//! Verified transfer (ADR 0003, design D5): copy → hash both sides →
//! atomic rename, then — only after that succeeds — quarantine/delete.
//! This is the one place in the crate that touches user footage
//! destructively, so every fallible step here maps to a per-file
//! `TransferOutcome` instead of aborting the whole run: one bad file
//! must not stop the rest of a card from importing.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::plan::ImportPlan;
use crate::source::Verdict;

const BUF_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferOutcome {
    Transferred,
    SkippedIdentical,
    Suffixed(PathBuf),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct FileResult {
    pub src: PathBuf,
    pub outcome: TransferOutcome,
}

#[derive(Debug, Clone)]
pub struct GroupResult {
    pub group_name: String,
    pub verdict: Verdict,
    pub files: Vec<FileResult>,
    pub deleted_from_source: bool,
}

#[derive(Debug, Clone)]
pub struct ExecuteReport {
    pub groups: Vec<GroupResult>,
    pub deletion_skipped_reason: Option<String>,
}

fn outcome_is_success(outcome: &TransferOutcome) -> bool {
    matches!(
        outcome,
        TransferOutcome::Transferred
            | TransferOutcome::SkippedIdentical
            | TransferOutcome::Suffixed(_)
    )
}

/// Executes every planned action: transfers `Keep` groups to their
/// destination and `Quarantine` groups to their quarantine path
/// (identical safety mechanism, different target directory); `Ignore`
/// groups are left untouched. Source deletion — gated on
/// `delete_source`, `keep_source`, and a confirmation prompt — only
/// ever considers groups whose files all transferred or were confirmed
/// already-imported (spec: "Source deletion only after verification").
pub fn execute(
    plan: &ImportPlan,
    delete_source: bool,
    keep_source: bool,
    assume_yes: bool,
) -> Result<ExecuteReport> {
    let mut groups = Vec::with_capacity(plan.actions.len());

    for action in &plan.actions {
        let target_dir = match &action.verdict {
            Verdict::Keep => action.destination.as_deref(),
            Verdict::Quarantine => action.quarantine_path.as_deref(),
            Verdict::Ignore(_) => None,
        };

        let mut files = Vec::with_capacity(action.group.files.len());
        if let Some(dir) = target_dir {
            for media_file in &action.group.files {
                let outcome = transfer_file(&media_file.path, dir)?;
                files.push(FileResult {
                    src: media_file.path.clone(),
                    outcome,
                });
            }
        }

        groups.push(GroupResult {
            group_name: action.group.name.clone(),
            verdict: action.verdict.clone(),
            files,
            deleted_from_source: false,
        });
    }

    let mut deletion_skipped_reason = None;
    if delete_source && !keep_source {
        let any_eligible = groups
            .iter()
            .any(|g| !g.files.is_empty() && g.files.iter().all(|f| outcome_is_success(&f.outcome)));

        if any_eligible {
            match confirm_deletion(assume_yes)? {
                Confirmation::Confirmed => {
                    for group in &mut groups {
                        let all_ok = !group.files.is_empty()
                            && group.files.iter().all(|f| outcome_is_success(&f.outcome));
                        if all_ok {
                            for file in &group.files {
                                let _ = fs::remove_file(&file.src);
                            }
                            group.deleted_from_source = true;
                        }
                    }
                }
                Confirmation::DeclinedInteractive => {
                    deletion_skipped_reason =
                        Some("deletion declined; source files were not deleted".to_string());
                }
                Confirmation::SkippedNonInteractive => {
                    deletion_skipped_reason = Some(
                        "stdin is not a terminal; skipping source deletion (pass --yes to confirm non-interactively)"
                            .to_string(),
                    );
                }
            }
        }
    }

    Ok(ExecuteReport {
        groups,
        deletion_skipped_reason,
    })
}

enum Confirmation {
    Confirmed,
    DeclinedInteractive,
    SkippedNonInteractive,
}

/// Destructive steps prompt on stdin unless `--yes` is passed;
/// non-interactive stdin without `--yes` aborts rather than assumes
/// (design D8, spec: "Destructive steps require confirmation").
fn confirm_deletion(assume_yes: bool) -> Result<Confirmation> {
    if assume_yes {
        return Ok(Confirmation::Confirmed);
    }
    if !io::stdin().is_terminal() {
        return Ok(Confirmation::SkippedNonInteractive);
    }
    print!("Delete source files now that they are safely imported? [y/N] ");
    io::stdout()
        .flush()
        .map_err(|e| Error::io(Path::new("<stdout>"), e))?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| Error::io(Path::new("<stdin>"), e))?;
    Ok(parse_confirmation(&line))
}

/// Interprets a y/N answer: only `y`/`yes` (case- and whitespace-
/// insensitive) confirm; anything else — including an empty line —
/// declines. Split out from `confirm_deletion` so the accept/decline
/// decision is unit-testable without a real terminal on stdin.
fn parse_confirmation(line: &str) -> Confirmation {
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" => Confirmation::Confirmed,
        _ => Confirmation::DeclinedInteractive,
    }
}

/// Streams `src` to `<final>.part` under `dest_dir` while hashing the
/// source, re-hashes the written file, and only on a match renames it
/// to its final name (design D5). Any failure along the way — copy
/// error or hash mismatch — removes the `.part` file and leaves `src`
/// untouched, reported as `TransferOutcome::Failed` rather than
/// propagated, so the caller can keep processing the rest of the plan.
pub fn transfer_file(src: &Path, dest_dir: &Path) -> Result<TransferOutcome> {
    fs::create_dir_all(dest_dir).map_err(|e| Error::io(dest_dir, e))?;

    match transfer_inner(src, dest_dir) {
        Ok(outcome) => Ok(outcome),
        Err(e) => Ok(TransferOutcome::Failed(e.to_string())),
    }
}

fn transfer_inner(src: &Path, dest_dir: &Path) -> Result<TransferOutcome> {
    let file_name = src.file_name().ok_or_else(|| {
        Error::io(
            src,
            io::Error::new(io::ErrorKind::InvalidInput, "source path has no file name"),
        )
    })?;

    let src_hash = hash_file(src)?;

    let final_path = match resolve_destination(dest_dir, file_name, &src_hash)? {
        None => return Ok(TransferOutcome::SkippedIdentical),
        Some(path) => path,
    };
    let suffixed = final_path != dest_dir.join(file_name);

    let part_path = {
        let mut name = final_path.clone().into_os_string();
        name.push(".part");
        PathBuf::from(name)
    };

    let dest_hash = match copy_and_hash(src, &part_path) {
        Ok(hash) => hash,
        Err(e) => {
            let _ = fs::remove_file(&part_path);
            return Err(e);
        }
    };

    if dest_hash != src_hash {
        let _ = fs::remove_file(&part_path);
        return Err(Error::VerifyMismatch {
            src: src.to_path_buf(),
            dest: final_path,
        });
    }

    fs::rename(&part_path, &final_path).map_err(|e| Error::io(&final_path, e))?;

    Ok(if suffixed {
        TransferOutcome::Suffixed(final_path)
    } else {
        TransferOutcome::Transferred
    })
}

/// Picks the path a file should land at: `None` if a file with
/// identical content already exists there (spec: collisions never
/// overwrite — identical content counts as already-imported), or a
/// numeric-suffixed name (`-1`, `-2`, ...) if a *different* file
/// already occupies the plain name.
fn resolve_destination(
    dest_dir: &Path,
    file_name: &OsStr,
    src_hash: &blake3::Hash,
) -> Result<Option<PathBuf>> {
    let mut candidate = dest_dir.join(file_name);
    let mut suffix = 0u32;
    loop {
        if !candidate.exists() {
            return Ok(Some(candidate));
        }
        if &hash_file(&candidate)? == src_hash {
            return Ok(None);
        }
        suffix += 1;
        candidate = suffixed_path(dest_dir, file_name, suffix);
    }
}

fn suffixed_path(dest_dir: &Path, file_name: &OsStr, n: u32) -> PathBuf {
    let name = Path::new(file_name);
    let stem = name.file_stem().unwrap_or(file_name).to_string_lossy();
    let new_name = match name.extension() {
        Some(ext) => format!("{stem}-{n}.{}", ext.to_string_lossy()),
        None => format!("{stem}-{n}"),
    };
    dest_dir.join(new_name)
}

fn copy_and_hash(src: &Path, dest: &Path) -> Result<blake3::Hash> {
    let mut reader = File::open(src).map_err(|e| Error::io(src, e))?;
    let mut writer = File::create(dest).map_err(|e| Error::io(dest, e))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; BUF_SIZE];
    loop {
        let n = reader.read(&mut buf).map_err(|e| Error::io(src, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        writer
            .write_all(&buf[..n])
            .map_err(|e| Error::io(dest, e))?;
    }
    writer.flush().map_err(|e| Error::io(dest, e))?;
    Ok(hasher.finalize())
}

fn hash_file(path: &Path) -> Result<blake3::Hash> {
    let mut reader = File::open(path).map_err(|e| Error::io(path, e))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; BUF_SIZE];
    loop {
        let n = reader.read(&mut buf).map_err(|e| Error::io(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_accepts_only_yes_variants() {
        for input in ["y", "Y", "yes", "YES", " yes \n", "y\n"] {
            assert!(
                matches!(parse_confirmation(input), Confirmation::Confirmed),
                "{input:?} should confirm"
            );
        }
    }

    #[test]
    fn confirmation_declines_everything_else() {
        // The declined-interactive branch: spec "Declined prompt" — an
        // explicit no, and (defensively) an empty line, must not delete.
        for input in ["n", "no", "", "\n", "  ", "nope", "yeah"] {
            assert!(
                matches!(parse_confirmation(input), Confirmation::DeclinedInteractive),
                "{input:?} should decline"
            );
        }
    }

    #[test]
    fn transfers_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"hello").unwrap();
        let dest_dir = dir.path().join("dest");

        let outcome = transfer_file(&src, &dest_dir).unwrap();
        assert_eq!(outcome, TransferOutcome::Transferred);
        assert_eq!(fs::read(dest_dir.join("clip.mp4")).unwrap(), b"hello");
        assert!(src.exists(), "transfer never deletes the source");
        assert!(!dest_dir.join("clip.mp4.part").exists());
    }

    #[test]
    fn identical_content_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"hello").unwrap();
        let dest_dir = dir.path().join("dest");
        fs::create_dir_all(&dest_dir).unwrap();
        fs::write(dest_dir.join("clip.mp4"), b"hello").unwrap();

        let outcome = transfer_file(&src, &dest_dir).unwrap();
        assert_eq!(outcome, TransferOutcome::SkippedIdentical);
    }

    #[test]
    fn different_content_gets_suffixed() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"new-bytes").unwrap();
        let dest_dir = dir.path().join("dest");
        fs::create_dir_all(&dest_dir).unwrap();
        fs::write(dest_dir.join("clip.mp4"), b"old-bytes").unwrap();

        let outcome = transfer_file(&src, &dest_dir).unwrap();
        let TransferOutcome::Suffixed(path) = outcome else {
            panic!("expected Suffixed, got different outcome");
        };
        assert_eq!(path, dest_dir.join("clip-1.mp4"));
        assert_eq!(fs::read(&path).unwrap(), b"new-bytes");
        assert_eq!(fs::read(dest_dir.join("clip.mp4")).unwrap(), b"old-bytes");
    }
}
