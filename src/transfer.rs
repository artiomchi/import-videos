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

use jiff::Timestamp;

use crate::error::{Error, Result};
use crate::plan::ImportPlan;
use crate::progress::Progress;
use crate::source::{Sidecar, Verdict};

const BUF_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferOutcome {
    Transferred,
    /// The file was cloned into place with a copy-on-write reflink
    /// rather than streamed. A successful clone shares the source's
    /// exact extents and is therefore verified by construction, not by
    /// a read-back hash (design D3) — it counts as content-verified and
    /// keeps source-deletion eligibility exactly like `Transferred`.
    Reflinked,
    SkippedIdentical,
    Suffixed(PathBuf),
    /// The file belongs to a `Quarantine` group whose profile has
    /// `copy_quarantine: false` — it was deliberately left in place
    /// on the source with no copy made. Because no transfer occurred,
    /// this outcome MUST NOT count as a success and MUST NOT make the
    /// group a source-deletion candidate.
    SkippedQuarantineDisabled,
    /// The destination file's name, size, and mtime matched the source
    /// within the 0.1 s tolerance — accepted as already-imported
    /// without content hashing. Because the content was **not** verified,
    /// this outcome MUST NOT make the group a source-deletion candidate
    /// (design D1, D2, ADR 0009).
    SkippedQuickMatch,
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
    /// Outcome of writing the group's sidecar, if it had one (design
    /// D6). Kept separate from `files` because a sidecar isn't a
    /// source file: it must never be treated as a deletion candidate.
    pub sidecar_outcome: Option<TransferOutcome>,
    pub deleted_from_source: bool,
    /// Where this group's files landed (`Keep`'s destination or
    /// `Quarantine`'s quarantine path); `None` for an `Ignore` verdict
    /// or a `Quarantine` group whose `copy_quarantine: false` left it
    /// on the source. Carried so the human report's verbose grouping
    /// (improve-console-output design D7) can header each group with
    /// where it went, without re-deriving it from the plan.
    pub destination: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ExecuteReport {
    pub groups: Vec<GroupResult>,
    pub deletion_skipped_reason: Option<String>,
    /// The effective `delete_source` this run executed with — carried
    /// so the human report can tell "no group was deleted because
    /// deletion wasn't requested" apart from "... because deletion was
    /// requested but every group was ineligible" (design D7).
    pub delete_source: bool,
}

/// Gates sidecar-writing and "handled" reporting: the file is in
/// place at the destination, whether by full verified transfer,
/// identity skip, suffix rename, or quick-match heuristic.
/// `SkippedQuarantineDisabled` is excluded (no copy was made).
fn in_place_at_destination(outcome: &TransferOutcome) -> bool {
    matches!(
        outcome,
        TransferOutcome::Transferred
            | TransferOutcome::Reflinked
            | TransferOutcome::SkippedIdentical
            | TransferOutcome::Suffixed(_)
            | TransferOutcome::SkippedQuickMatch
    )
}

/// Gates source deletion: the file’s content was actually verified at
/// the destination. Excludes `SkippedQuickMatch` (heuristic only —
/// unverified) and `SkippedQuarantineDisabled` (no copy at all).
/// Design D1; preserves ADR 0003’s safety invariant.
fn content_verified(outcome: &TransferOutcome) -> bool {
    matches!(
        outcome,
        TransferOutcome::Transferred
            | TransferOutcome::Reflinked
            | TransferOutcome::SkippedIdentical
            | TransferOutcome::Suffixed(_)
    )
}

/// A group with no sidecar has nothing to fail; one that wrote its
/// sidecar successfully is fine too — only an explicit write failure
/// blocks deletion (spec: "Sidecar failure blocks source deletion").
fn sidecar_ok(outcome: &Option<TransferOutcome>) -> bool {
    !matches!(outcome, Some(TransferOutcome::Failed(_)))
}

/// Executes every planned action: transfers `Keep` groups to their
/// destination and `Quarantine` groups to their quarantine path
/// (identical safety mechanism, different target directory); `Ignore`
/// groups are left untouched. Source deletion — gated on
/// `delete_source` and a confirmation prompt — only ever considers
/// groups whose files all transferred or were confirmed
/// already-imported (spec: "Source deletion only after verification").
/// `delete_source` is the effective value: the caller has already
/// folded in any `--delete-source`/`--no-delete-source` override at
/// profile resolution (design D5), so this function stays
/// override-unaware.
pub fn execute(
    plan: &ImportPlan,
    delete_source: bool,
    assume_yes: bool,
    quick_match: bool,
    reflink: bool,
    progress: &Progress,
) -> Result<ExecuteReport> {
    // The one place we consult the ambient terminal: whether stdin is
    // interactive is what decides if a missing `--yes` prompts or
    // safely skips deletion. Threading it into `execute_inner` as a
    // plain `bool` keeps that global dependency at the edge, so the
    // deletion gate stays deterministic to test without a real tty —
    // and an in-process test can never hang waiting on stdin.
    execute_inner(
        plan,
        delete_source,
        assume_yes,
        quick_match,
        reflink,
        io::stdin().is_terminal(),
        progress,
    )
}

fn execute_inner(
    plan: &ImportPlan,
    delete_source: bool,
    assume_yes: bool,
    quick_match: bool,
    reflink: bool,
    stdin_is_terminal: bool,
    progress: &Progress,
) -> Result<ExecuteReport> {
    let total_bytes: u64 = plan
        .actions
        .iter()
        .filter(|a| {
            matches!(a.verdict, Verdict::Keep)
                || (matches!(a.verdict, Verdict::Quarantine) && a.quarantine_path.is_some())
        })
        .flat_map(|a| &a.group.files)
        .map(|f| f.size)
        .sum();
    progress.set_length(total_bytes);

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
                if let Some(name) = media_file.path.file_name() {
                    progress.set_message(format!("copying {}", name.to_string_lossy()));
                }
                let outcome = transfer_file(
                    &media_file.path,
                    dir,
                    media_file.recorded_at,
                    quick_match,
                    reflink,
                    progress,
                )?;
                // `copy_and_hash` only ticks bytes for files it actually
                // streams; a quick-match or identical-content skip never
                // reaches it, even though its bytes are counted in
                // `total_bytes` above. A reflinked file likewise moves no
                // bytes through `copy_and_hash` (design D7). Without
                // this, a re-run that skips most files — or an import
                // that reflinks — would leave the bar stalled near 0%
                // instead of reflecting real completion.
                if matches!(
                    outcome,
                    TransferOutcome::SkippedQuickMatch
                        | TransferOutcome::SkippedIdentical
                        | TransferOutcome::Reflinked
                ) {
                    progress.inc(media_file.size);
                }
                files.push(FileResult {
                    src: media_file.path.clone(),
                    outcome,
                });
            }
        } else if matches!(action.verdict, Verdict::Quarantine) {
            // copy_quarantine: false — record each file as left in
            // place; no filesystem access whatsoever.
            for media_file in &action.group.files {
                files.push(FileResult {
                    src: media_file.path.clone(),
                    outcome: TransferOutcome::SkippedQuarantineDisabled,
                });
            }
        }

        // Sidecar is written only once every file in the group has
        // transferred and verified (spec: "written ... only after all
        // of the session's files transferred and verified").
        let all_files_ok =
            !files.is_empty() && files.iter().all(|f| in_place_at_destination(&f.outcome));
        let sidecar_outcome = match (target_dir, &action.group.sidecar) {
            (Some(dir), Some(sidecar)) if all_files_ok => Some(write_sidecar(dir, sidecar)),
            _ => None,
        };

        groups.push(GroupResult {
            group_name: action.group.name.clone(),
            verdict: action.verdict.clone(),
            files,
            sidecar_outcome,
            deleted_from_source: false,
            destination: target_dir.map(Path::to_path_buf),
        });
    }

    // Every transfer is done, so clear the bar — and, crucially, stop
    // its steady-tick thread — before any confirmation prompt. Left
    // running, that background redraw repaints the bar over the
    // `[y/N]` prompt and the user's keystrokes.
    progress.finish();

    let mut deletion_skipped_reason = None;
    if delete_source {
        let any_eligible = groups.iter().any(|g| {
            !g.files.is_empty()
                && g.files.iter().all(|f| content_verified(&f.outcome))
                && sidecar_ok(&g.sidecar_outcome)
        });

        if any_eligible {
            let decision = confirm(
                "Delete source files now that they are safely imported? [y/N]",
                assume_yes,
                stdin_is_terminal,
            )?;
            tracing::info!(
                decision = confirmation_label(&decision),
                "deletion decision"
            );
            match decision {
                Confirmation::Confirmed => {
                    for group in &mut groups {
                        let all_ok = !group.files.is_empty()
                            && group.files.iter().all(|f| content_verified(&f.outcome))
                            && sidecar_ok(&group.sidecar_outcome);
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
        delete_source,
    })
}

pub enum Confirmation {
    Confirmed,
    DeclinedInteractive,
    SkippedNonInteractive,
}

fn confirmation_label(decision: &Confirmation) -> &'static str {
    match decision {
        Confirmation::Confirmed => "confirmed",
        Confirmation::DeclinedInteractive => "declined",
        Confirmation::SkippedNonInteractive => "skipped_non_interactive",
    }
}

/// Destructive steps prompt on stdin unless `--yes` is passed;
/// non-interactive stdin without `--yes` aborts rather than assumes
/// (design D8, spec: "Destructive steps require confirmation").
/// Shared by `import`'s source deletion and `cleanup`'s purge (design
/// D7) so the two never drift on confirmation semantics.
pub fn confirm(prompt: &str, assume_yes: bool, stdin_is_terminal: bool) -> Result<Confirmation> {
    if assume_yes {
        return Ok(Confirmation::Confirmed);
    }
    if !stdin_is_terminal {
        return Ok(Confirmation::SkippedNonInteractive);
    }
    print!("{prompt} ");
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
/// declines. Split out from `confirm` so the accept/decline
/// decision is unit-testable without a real terminal on stdin.
fn parse_confirmation(line: &str) -> Confirmation {
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" => Confirmation::Confirmed,
        _ => Confirmation::DeclinedInteractive,
    }
}

/// Writes a group's sidecar into its already-transferred target
/// directory. Failure never propagates — it becomes a `Failed`
/// outcome so the caller can keep processing the rest of the plan and
/// so it participates in `sidecar_ok`'s deletion gate.
fn write_sidecar(dir: &Path, sidecar: &Sidecar) -> TransferOutcome {
    match write_sidecar_inner(dir, sidecar) {
        Ok(()) => TransferOutcome::Transferred,
        Err(e) => TransferOutcome::Failed(e.to_string()),
    }
}

fn write_sidecar_inner(dir: &Path, sidecar: &Sidecar) -> Result<()> {
    let path = dir.join(&sidecar.filename);
    // `Sidecar::content` is always built from our own String-keyed,
    // finite-valued structures, so serialization cannot fail in
    // practice.
    let bytes = serde_json::to_vec_pretty(&sidecar.content)
        .expect("sidecar content is always representable as JSON");
    fs::write(&path, bytes).map_err(|e| Error::io(&path, e))
}

/// Streams `src` to `<final>.part` under `dest_dir`, hashing it once
/// while writing, then re-reads and re-hashes the written file and
/// only on a match renames it to its final name (design D5, and
/// single-pass-verified-transfer D1-D3: this re-hash of the *written*
/// file, not a second read of the source, is what makes verification
/// real). Any failure along the way — copy error, read-back error, or
/// hash mismatch — removes the `.part` file and leaves `src`
/// untouched, reported as `TransferOutcome::Failed` rather than
/// propagated, so the caller can keep processing the rest of the plan.
/// When the final destination name is already occupied, the source is
/// hashed once up front to resolve the collision (identical content
/// skips the copy entirely) before the same single-pass copy and
/// read-back runs at the suffixed name — so the source is read twice
/// only on this rare path. The existence check that decides which
/// branch to take and the eventual rename are not atomic (TOCTOU): this
/// tool's model assumes the destination library isn't concurrently
/// mutated by another process.
/// `recorded_at`, when given, stamps the destination's mtime after the
/// verified rename (gopro-telemetry design D8) — never for a file
/// that's skipped as already-imported.
/// When `quick_match` is `true` and `recorded_at` is `Some`, the fast
/// path in `transfer_inner` may return `SkippedQuickMatch` without
/// hashing; see design D3.
/// When `reflink` is `true`, the copy site first attempts a
/// copy-on-write clone of `src` into `<final>.part`; any failure falls
/// through to the stream-copy-and-verify path unchanged (design D1-D4
/// of add-reflink-transfer).
pub fn transfer_file(
    src: &Path,
    dest_dir: &Path,
    recorded_at: Option<Timestamp>,
    quick_match: bool,
    reflink: bool,
    progress: &Progress,
) -> Result<TransferOutcome> {
    fs::create_dir_all(dest_dir).map_err(|e| Error::io(dest_dir, e))?;

    match transfer_inner(src, dest_dir, recorded_at, quick_match, reflink, progress) {
        Ok(outcome) => Ok(outcome),
        Err(e) => Ok(TransferOutcome::Failed(e.to_string())),
    }
}

fn transfer_inner(
    src: &Path,
    dest_dir: &Path,
    recorded_at: Option<Timestamp>,
    quick_match: bool,
    reflink: bool,
    progress: &Progress,
) -> Result<TransferOutcome> {
    let file_name = src.file_name().ok_or_else(|| {
        Error::io(
            src,
            io::Error::new(io::ErrorKind::InvalidInput, "source path has no file name"),
        )
    })?;

    // Quick-match fast path (design D3, ADR 0009): before hashing,
    // check if the canonical destination file already exists with
    // matching size and mtime within 0.1 s of `recorded_at`.
    // On any miss — file absent, size differs, mtime outside tolerance,
    // or any I/O error — fall through to the full verified path.
    if quick_match && let Some(ref_ts) = recorded_at {
        let dest_candidate = dest_dir.join(file_name);
        if let Ok(dest_meta) = fs::metadata(&dest_candidate) {
            let src_size = fs::metadata(src).map(|m| m.len()).unwrap_or(0);
            if dest_meta.len() == src_size
                && let Ok(dest_mtime) = dest_meta.modified()
            {
                let dest_ts = systemtime_to_timestamp(dest_mtime);
                let diff_ms = (dest_ts - ref_ts).get_milliseconds().unsigned_abs();
                if diff_ms <= 100 {
                    tracing::debug!(
                        file = %dest_candidate.display(),
                        size = src_size,
                        diff_ms,
                        "quick-match hit"
                    );
                    return Ok(TransferOutcome::SkippedQuickMatch);
                }
                tracing::debug!(
                    file = %dest_candidate.display(),
                    diff_ms,
                    "quick-match miss: mtime outside tolerance"
                );
            } else {
                tracing::debug!(
                    file = %dest_candidate.display(),
                    dest_size = dest_meta.len(),
                    src_size,
                    "quick-match miss: size differs"
                );
            }
        }
    }

    // Branch on destination-name occupancy (design D1) rather than
    // hashing the source up front: in the common unoccupied case, the
    // final path is the plain name by construction — no pre-hash, no
    // `resolve_destination` call, so the source is read exactly once
    // below by `copy_and_hash`. Only when the name is already taken do
    // we pay for a separate source hash, because knowing it before
    // copying is what lets identical-content collisions skip the copy
    // entirely.
    let dest_candidate = dest_dir.join(file_name);
    let (final_path, suffixed) = if dest_candidate.exists() {
        // The name is taken, so we hash the source (a full read off the
        // slow source medium) to tell an already-imported file from a
        // genuine collision. That read ticks no bytes — the copy phase
        // owns the byte budget — so announce the phase explicitly;
        // otherwise the bar sits under a stale "copying" message with
        // no visible movement while a multi-GB clip is hashed.
        progress.set_message(format!("checking {}", file_name.to_string_lossy()));
        let src_hash = hash_file(src)?;
        match resolve_destination(dest_dir, file_name, &src_hash)? {
            None => return Ok(TransferOutcome::SkippedIdentical),
            Some(path) => {
                let suffixed = path != dest_candidate;
                (path, suffixed)
            }
        }
    } else {
        (dest_candidate, false)
    };

    let part_path = {
        let mut name = final_path.clone().into_os_string();
        name.push(".part");
        PathBuf::from(name)
    };

    // Reflink fast path (design D1-D5 of add-reflink-transfer): try a
    // copy-on-write clone before falling back to the stream-copy path
    // below. The attempt itself is the probe — no `st_dev` pre-check —
    // so any failure (cross-device, non-CoW filesystem, or any other
    // I/O error) simply falls through unchanged (design D2). A
    // successful clone shares the source's exact extents and is
    // byte-identical by construction, so it skips the read-back hash
    // entirely (design D3).
    if reflink {
        // The strict `reflink()` expects the target not to exist, so a
        // stale `.part` from a previous aborted run must be cleared
        // first (design D5) — the stream-copy path tolerates this via
        // `File::create`'s truncate, but a clone would otherwise fail
        // with `AlreadyExists` even when cloning is available.
        let _ = fs::remove_file(&part_path);
        match reflink_copy::reflink(src, &part_path) {
            Ok(()) => {
                fs::rename(&part_path, &final_path).map_err(|e| Error::io(&final_path, e))?;
                if let Some(recorded_at) = recorded_at {
                    stamp_mtime(&final_path, recorded_at);
                }
                return Ok(TransferOutcome::Reflinked);
            }
            Err(error) => {
                tracing::debug!(
                    file = %src.display(),
                    %error,
                    "reflink unavailable, falling back to stream copy"
                );
                let _ = fs::remove_file(&part_path);
            }
        }
    }

    let stream_hash = match copy_and_hash(src, &part_path, progress) {
        Ok(hash) => hash,
        Err(e) => {
            let _ = fs::remove_file(&part_path);
            return Err(e);
        }
    };

    // The read-back step announces itself separately from the copy
    // that preceded it (design D2) — the bar sits near 100% here since
    // verification targets the fast destination disk, not the slow
    // source medium; the message explains the brief tail instead.
    progress.set_message(format!("verifying {}", file_name.to_string_lossy()));
    if let Err(e) = verify_part(&part_path, src, &final_path, &stream_hash) {
        let _ = fs::remove_file(&part_path);
        return Err(e);
    }

    fs::rename(&part_path, &final_path).map_err(|e| Error::io(&final_path, e))?;

    if let Some(recorded_at) = recorded_at {
        stamp_mtime(&final_path, recorded_at);
    }

    Ok(if suffixed {
        TransferOutcome::Suffixed(final_path)
    } else {
        TransferOutcome::Transferred
    })
}

/// Sets `path`'s modification time to `recorded_at` (design D8). The
/// verified copy is already complete and correct at this point, so a
/// failure here is metadata-only: log and move on rather than fail the
/// transfer (spec: "mtime failure does not fail the import").
fn stamp_mtime(path: &Path, recorded_at: Timestamp) {
    let result = File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(std::time::SystemTime::from(recorded_at)));
    if let Err(error) = result {
        tracing::warn!(
            file = %path.display(),
            %error,
            "could not set destination file's modification time"
        );
    }
}

/// Converts a `std::time::SystemTime` to a `jiff::Timestamp` for
/// mtime comparisons. `SystemTime` can represent times before the
/// Unix epoch (negative duration); `jiff::Timestamp::from_second`
/// handles negative values, so the conversion is lossless for any
/// real filesystem mtime. Design D3 / task 3.5.
fn systemtime_to_timestamp(t: std::time::SystemTime) -> Timestamp {
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => Timestamp::new(d.as_secs() as i64, d.subsec_nanos() as i32)
            .unwrap_or(Timestamp::UNIX_EPOCH),
        Err(e) => {
            // Before epoch — negate the duration.
            let d = e.duration();
            Timestamp::new(-(d.as_secs() as i64), -(d.subsec_nanos() as i32))
                .unwrap_or(Timestamp::UNIX_EPOCH)
        }
    }
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
            if suffix > 0 {
                tracing::debug!(
                    file = %candidate.display(),
                    collisions = suffix,
                    "collision resolved: content differs, suffixed"
                );
            }
            return Ok(Some(candidate));
        }
        if &hash_file(&candidate)? == src_hash {
            tracing::debug!(
                file = %candidate.display(),
                "collision resolved: identical content, skipping copy"
            );
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

fn copy_and_hash(src: &Path, dest: &Path, progress: &Progress) -> Result<blake3::Hash> {
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
        progress.inc(n as u64);
    }
    writer.flush().map_err(|e| Error::io(dest, e))?;
    Ok(hasher.finalize())
}

/// Re-reads and hashes the written `.part` file, comparing it against
/// the hash captured while streaming the copy (design D2, D3). This is
/// the seam that makes verification real: it is the only place that
/// reads back what was actually persisted, rather than re-reading the
/// source. `src` and `final_path` are only used to shape the
/// `VerifyMismatch` error message.
fn verify_part(
    part_path: &Path,
    src: &Path,
    final_path: &Path,
    expected: &blake3::Hash,
) -> Result<()> {
    let actual = hash_file(part_path)?;
    if actual != *expected {
        return Err(Error::VerifyMismatch {
            src: src.to_path_buf(),
            dest: final_path.to_path_buf(),
        });
    }
    Ok(())
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
    use crate::plan::PlannedAction;
    use crate::source::{MediaFile, MediaGroup};
    use std::collections::HashMap;

    fn ts(secs: i64) -> jiff::Timestamp {
        jiff::Timestamp::from_second(secs).unwrap()
    }

    /// True when a plain-file reflink clone actually succeeds inside
    /// `dir` — used to runtime-skip the reflink success-path tests on a
    /// non-CoW filesystem (tmpfs/ext4 CI runners) rather than silently
    /// never running them (design Open Questions, task 6.3).
    fn cow_supported(dir: &Path) -> bool {
        let src = dir.join("__cow_probe_src");
        let dst = dir.join("__cow_probe_dst");
        fs::write(&src, b"probe").unwrap();
        let supported = reflink_copy::reflink(&src, &dst).is_ok();
        let _ = fs::remove_file(&src);
        let _ = fs::remove_file(&dst);
        supported
    }

    fn group_with_sidecar(path: &Path, sidecar: Sidecar) -> MediaGroup {
        MediaGroup {
            name: "session".to_string(),
            files: vec![MediaFile {
                path: path.to_path_buf(),
                size: 7,
                recorded_at: None,
            }],
            timestamp: ts(0),
            markers: vec![],
            geo: None,
            context: HashMap::new(),
            sidecar: Some(sidecar),
        }
    }

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
    fn non_interactive_without_yes_skips_deletion() {
        // spec: "Destructive steps require confirmation" — with
        // `delete_source` but no `--yes` and a non-interactive stdin,
        // deletion is skipped rather than assumed. `stdin_is_terminal`
        // is injected as `false` so the behaviour is deterministic
        // regardless of how the test runner wires stdin (calling the
        // public `execute` here would read the real terminal and block
        // on the `[y/N]` prompt).
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"footage").unwrap();
        let dest_dir = dir.path().join("dest");

        let group = MediaGroup {
            name: "a".to_string(),
            files: vec![MediaFile {
                path: src.clone(),
                size: 7,
                recorded_at: None,
            }],
            timestamp: ts(0),
            markers: vec![],
            geo: None,
            context: HashMap::new(),
            sidecar: None,
        };
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group,
                verdict: Verdict::Keep,
                destination: Some(dest_dir.clone()),
                quarantine_path: None,
            }],
        };

        let report =
            execute_inner(&plan, true, false, false, false, false, &Progress::hidden()).unwrap();

        assert!(src.exists(), "deletion must be skipped, not assumed");
        assert!(!report.groups[0].deleted_from_source);
        assert!(report.deletion_skipped_reason.is_some());
    }

    #[test]
    fn transfers_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"hello").unwrap();
        let dest_dir = dir.path().join("dest");

        let outcome =
            transfer_file(&src, &dest_dir, None, false, false, &Progress::hidden()).unwrap();
        assert_eq!(outcome, TransferOutcome::Transferred);
        assert_eq!(fs::read(dest_dir.join("clip.mp4")).unwrap(), b"hello");
        assert!(src.exists(), "transfer never deletes the source");
        assert!(!dest_dir.join("clip.mp4.part").exists());
    }

    // --- phase messages (design D2, task 4.2) ---

    #[test]
    fn message_sequence_over_one_transfer_is_copying_then_verifying() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"hello").unwrap();
        let dest_dir = dir.path().join("dest");

        let group = MediaGroup {
            name: "session".to_string(),
            files: vec![MediaFile {
                path: src.clone(),
                size: 5,
                recorded_at: None,
            }],
            timestamp: ts(0),
            markers: vec![],
            geo: None,
            context: HashMap::new(),
            sidecar: None,
        };
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group,
                verdict: Verdict::Keep,
                destination: Some(dest_dir.clone()),
                quarantine_path: None,
            }],
        };

        let progress = Progress::hidden();
        let report = execute(&plan, false, false, false, false, &progress).unwrap();
        assert!(matches!(
            report.groups[0].files[0].outcome,
            TransferOutcome::Transferred
        ));
        assert_eq!(
            progress.message_history(),
            vec![
                "copying clip.mp4".to_string(),
                "verifying clip.mp4".to_string()
            ],
            "one file should announce copying, then verifying, in that order"
        );
    }

    // spec scenario "Successful verified copy" / "Source is read once
    // when no collision exists" (single-pass-verified-transfer): the
    // unoccupied-name path lands the file at its final name with no
    // `.part` remnant, the mtime stamped, and the group eligible for
    // source deletion — the full end state the read-back verification
    // is supposed to unlock. The "read exactly once" half of the
    // scenario is enforced structurally rather than measured here: the
    // unoccupied branch in `transfer_inner` contains no `hash_file(src)`
    // call at all (design D3), so there is no source-read count to
    // assert against.
    #[test]
    fn happy_path_verified_copy_lands_at_final_name_and_group_is_deletable() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"hello world").unwrap();
        let dest_dir = dir.path().join("dest");

        let recorded_at: Timestamp = "2026-07-09T07:41:03Z".parse().unwrap();
        let group = MediaGroup {
            name: "session".to_string(),
            files: vec![MediaFile {
                path: src.clone(),
                size: 11,
                recorded_at: Some(recorded_at),
            }],
            timestamp: ts(0),
            markers: vec![],
            geo: None,
            context: HashMap::new(),
            sidecar: None,
        };
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group,
                verdict: Verdict::Keep,
                destination: Some(dest_dir.clone()),
                quarantine_path: None,
            }],
        };

        let report =
            execute_inner(&plan, true, true, false, false, false, &Progress::hidden()).unwrap();

        let dest_path = dest_dir.join("clip.mp4");
        assert_eq!(fs::read(&dest_path).unwrap(), b"hello world");
        assert!(!dest_dir.join("clip.mp4.part").exists());
        let mtime = fs::metadata(&dest_path).unwrap().modified().unwrap();
        assert_eq!(Timestamp::try_from(mtime).unwrap(), recorded_at);

        assert!(matches!(
            report.groups[0].files[0].outcome,
            TransferOutcome::Transferred
        ));
        assert!(
            report.groups[0].deleted_from_source,
            "verified transfer must make the group eligible for source deletion"
        );
        assert!(!src.exists(), "source should be deleted once verified");
    }

    #[test]
    fn identical_content_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"hello").unwrap();
        let dest_dir = dir.path().join("dest");
        fs::create_dir_all(&dest_dir).unwrap();
        fs::write(dest_dir.join("clip.mp4"), b"hello").unwrap();

        let outcome =
            transfer_file(&src, &dest_dir, None, false, false, &Progress::hidden()).unwrap();
        assert_eq!(outcome, TransferOutcome::SkippedIdentical);
    }

    // --- mtime stamping (gopro-telemetry design D8) ---

    #[test]
    fn mtime_stamped_to_recorded_time_after_transfer() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"hello").unwrap();
        let dest_dir = dir.path().join("dest");

        let recorded_at: Timestamp = "2026-07-09T07:41:03Z".parse().unwrap();
        let outcome = transfer_file(
            &src,
            &dest_dir,
            Some(recorded_at),
            false,
            false,
            &Progress::hidden(),
        )
        .unwrap();
        assert_eq!(outcome, TransferOutcome::Transferred);

        let dest_path = dest_dir.join("clip.mp4");
        let mtime = fs::metadata(&dest_path).unwrap().modified().unwrap();
        assert_eq!(Timestamp::try_from(mtime).unwrap(), recorded_at);
        assert_eq!(
            fs::read(&dest_path).unwrap(),
            b"hello",
            "content stays byte-identical"
        );
    }

    #[test]
    fn mtime_stamped_for_quarantine_transfer_too() {
        // design D8: "destination and quarantine transfers alike" —
        // routed through `execute` rather than calling `transfer_file`
        // directly, to exercise the real quarantine path.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"hello").unwrap();
        let quarantine_dir = dir.path().join("quarantine");

        let recorded_at: Timestamp = "2026-07-09T07:41:03Z".parse().unwrap();
        let group = MediaGroup {
            name: "session".to_string(),
            files: vec![MediaFile {
                path: src.clone(),
                size: 5,
                recorded_at: Some(recorded_at),
            }],
            timestamp: ts(0),
            markers: vec![],
            geo: None,
            context: HashMap::new(),
            sidecar: None,
        };
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group,
                verdict: Verdict::Quarantine,
                destination: None,
                quarantine_path: Some(quarantine_dir.clone()),
            }],
        };

        execute(&plan, false, false, false, false, &Progress::hidden()).unwrap();

        let mtime = fs::metadata(quarantine_dir.join("clip.mp4"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(Timestamp::try_from(mtime).unwrap(), recorded_at);
    }

    #[test]
    fn skipped_identical_file_mtime_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"hello").unwrap();
        let dest_dir = dir.path().join("dest");
        fs::create_dir_all(&dest_dir).unwrap();
        let dest_path = dest_dir.join("clip.mp4");
        fs::write(&dest_path, b"hello").unwrap();

        // A distinctive mtime far from "now" and from the recorded_at
        // below, so any accidental touch is detectable.
        let original_mtime =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        File::options()
            .write(true)
            .open(&dest_path)
            .unwrap()
            .set_modified(original_mtime)
            .unwrap();

        let recorded_at: Timestamp = "2026-07-09T07:41:03Z".parse().unwrap();
        let outcome = transfer_file(
            &src,
            &dest_dir,
            Some(recorded_at),
            false,
            false,
            &Progress::hidden(),
        )
        .unwrap();
        assert_eq!(outcome, TransferOutcome::SkippedIdentical);

        let mtime_after = fs::metadata(&dest_path).unwrap().modified().unwrap();
        assert_eq!(
            mtime_after, original_mtime,
            "skipped-identical file must not be touched"
        );
    }

    #[test]
    fn no_recorded_at_leaves_mtime_alone() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"hello").unwrap();
        let dest_dir = dir.path().join("dest");

        let outcome =
            transfer_file(&src, &dest_dir, None, false, false, &Progress::hidden()).unwrap();
        assert_eq!(outcome, TransferOutcome::Transferred);
        // No panic and a normal outcome is the whole point here: with
        // `recorded_at: None`, `transfer_inner` never calls
        // `stamp_mtime` at all.
    }

    #[test]
    fn mtime_stamp_failure_is_logged_not_propagated() {
        // Exercises `stamp_mtime` directly against a path that can't be
        // opened, standing in for a filesystem that rejects the mtime
        // change (spec: "mtime failure does not fail the import") —
        // the function has no `Result` to check; this test's only
        // assertion is that it returns instead of panicking.
        let missing = Path::new("/nonexistent-dir-for-import-videos-test/clip.mp4");
        stamp_mtime(missing, Timestamp::UNIX_EPOCH);
    }

    #[test]
    fn sidecar_written_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"footage").unwrap();
        let dest_dir = dir.path().join("dest");

        let sidecar = Sidecar {
            filename: "import.json".to_string(),
            content: serde_json::json!({"camera": "gopro-hero8"}),
        };
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group: group_with_sidecar(&src, sidecar),
                verdict: Verdict::Keep,
                destination: Some(dest_dir.clone()),
                quarantine_path: None,
            }],
        };

        let report = execute(&plan, false, false, false, false, &Progress::hidden()).unwrap();

        assert!(matches!(
            report.groups[0].sidecar_outcome,
            Some(TransferOutcome::Transferred)
        ));
        let content: serde_json::Value =
            serde_json::from_slice(&fs::read(dest_dir.join("import.json")).unwrap()).unwrap();
        assert_eq!(content, serde_json::json!({"camera": "gopro-hero8"}));
    }

    #[test]
    fn sidecar_failure_blocks_source_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"footage").unwrap();
        let dest_dir = dir.path().join("dest");
        // Occupy the sidecar's path with a directory so the write fails.
        fs::create_dir_all(dest_dir.join("import.json")).unwrap();

        let sidecar = Sidecar {
            filename: "import.json".to_string(),
            content: serde_json::json!({}),
        };
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group: group_with_sidecar(&src, sidecar),
                verdict: Verdict::Keep,
                destination: Some(dest_dir.clone()),
                quarantine_path: None,
            }],
        };

        let report = execute(&plan, true, true, false, false, &Progress::hidden()).unwrap();

        assert!(matches!(
            report.groups[0].sidecar_outcome,
            Some(TransferOutcome::Failed(_))
        ));
        assert!(!report.groups[0].deleted_from_source);
        assert!(
            src.exists(),
            "source must be retained when the sidecar fails to write"
        );
    }

    // --- verify_part seam (design D3, task 1.2) ---
    //
    // The corruption path can't be provoked end-to-end through the
    // public `transfer_file` API (no hook exists between "copy
    // finished" and "verify ran"), so these tests drive the extracted
    // seam directly: write a `.part` by hand, then check it against an
    // expected hash the way `transfer_inner` does.

    #[test]
    fn verify_part_passes_on_matching_content() {
        let dir = tempfile::tempdir().unwrap();
        let part_path = dir.path().join("clip.mp4.part");
        fs::write(&part_path, b"hello").unwrap();
        let expected = blake3::hash(b"hello");

        assert!(verify_part(&part_path, Path::new("src"), Path::new("dest"), &expected).is_ok());
    }

    #[test]
    fn verify_part_reports_mismatch_on_corrupted_content() {
        let dir = tempfile::tempdir().unwrap();
        let part_path = dir.path().join("clip.mp4.part");
        fs::write(&part_path, b"corrupted").unwrap();
        let expected = blake3::hash(b"hello");

        let src = Path::new("/source/clip.mp4");
        let dest = Path::new("/dest/clip.mp4");
        let err = verify_part(&part_path, src, dest, &expected).unwrap_err();
        assert!(matches!(
            err,
            Error::VerifyMismatch { src: s, dest: d } if s == src && d == dest
        ));
    }

    #[test]
    fn verify_part_reports_io_error_on_unreadable_part() {
        let dir = tempfile::tempdir().unwrap();
        let part_path = dir.path().join("missing.mp4.part");
        let expected = blake3::hash(b"hello");

        let err =
            verify_part(&part_path, Path::new("src"), Path::new("dest"), &expected).unwrap_err();
        assert!(matches!(err, Error::Io { .. }));
    }

    #[test]
    fn different_content_gets_suffixed() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"new-bytes").unwrap();
        let dest_dir = dir.path().join("dest");
        fs::create_dir_all(&dest_dir).unwrap();
        fs::write(dest_dir.join("clip.mp4"), b"old-bytes").unwrap();

        let outcome =
            transfer_file(&src, &dest_dir, None, false, false, &Progress::hidden()).unwrap();
        let TransferOutcome::Suffixed(path) = outcome else {
            panic!("expected Suffixed, got different outcome");
        };
        assert_eq!(path, dest_dir.join("clip-1.mp4"));
        assert_eq!(fs::read(&path).unwrap(), b"new-bytes");
        assert_eq!(fs::read(dest_dir.join("clip.mp4")).unwrap(), b"old-bytes");
    }

    // --- progress (design D6, task 5.4) ---
    //
    // `Progress`'s own construction/no-op behavior is unit-tested in
    // `src/progress.rs`; the tests below exercise it through real
    // transfers, where its bookkeeping actually matters.

    #[test]
    fn skipped_identical_still_advances_progress_by_full_size() {
        // `copy_and_hash` never runs for a content-identical skip, so
        // nothing ticks the bar unless `execute_inner` does it itself —
        // otherwise a re-run over already-imported footage would leave
        // the bar stalled near 0% despite doing exactly the "work" its
        // total already counted.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        let content = b"hello world";
        fs::write(&src, content).unwrap();
        let dest_dir = dir.path().join("dest");
        fs::create_dir_all(&dest_dir).unwrap();
        fs::write(dest_dir.join("clip.mp4"), content).unwrap();

        let group = MediaGroup {
            name: "a".to_string(),
            files: vec![MediaFile {
                path: src.clone(),
                size: content.len() as u64,
                recorded_at: None,
            }],
            timestamp: ts(0),
            markers: vec![],
            geo: None,
            context: HashMap::new(),
            sidecar: None,
        };
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group,
                verdict: Verdict::Keep,
                destination: Some(dest_dir.clone()),
                quarantine_path: None,
            }],
        };

        let progress = Progress::new(true, "Importing");
        let report = execute(&plan, false, false, false, false, &progress).unwrap();
        assert!(matches!(
            report.groups[0].files[0].outcome,
            TransferOutcome::SkippedIdentical
        ));
        assert_eq!(progress.position(), content.len() as u64);
    }

    #[test]
    fn skipped_quick_match_still_advances_progress_by_full_size() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        let content = b"hello world";
        fs::write(&src, content).unwrap();
        let dest_dir = dir.path().join("dest");
        fs::create_dir_all(&dest_dir).unwrap();
        let dest_path = dest_dir.join("clip.mp4");
        fs::write(&dest_path, content).unwrap();

        let recorded_at = ts(0);
        File::options()
            .write(true)
            .open(&dest_path)
            .unwrap()
            .set_modified(std::time::SystemTime::from(recorded_at))
            .unwrap();

        let group = MediaGroup {
            name: "a".to_string(),
            files: vec![MediaFile {
                path: src.clone(),
                size: content.len() as u64,
                recorded_at: Some(recorded_at),
            }],
            timestamp: ts(0),
            markers: vec![],
            geo: None,
            context: HashMap::new(),
            sidecar: None,
        };
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group,
                verdict: Verdict::Keep,
                destination: Some(dest_dir.clone()),
                quarantine_path: None,
            }],
        };

        let progress = Progress::new(true, "Importing");
        let report = execute(&plan, false, false, true, false, &progress).unwrap();
        assert!(matches!(
            report.groups[0].files[0].outcome,
            TransferOutcome::SkippedQuickMatch
        ));
        assert_eq!(progress.position(), content.len() as u64);
    }

    // --- reflink fast path (add-reflink-transfer) ---

    #[test]
    fn reflink_falls_back_to_stream_copy_when_unavailable() {
        // Spec: "Cross-device transfer falls back to verified copy"
        // (task 6.1). A default tempdir sits on a non-CoW filesystem on
        // every machine this crate's tests are expected to run on
        // (tmpfs/ext4), so a real reflink attempt here fails
        // deterministically and the engine must fall through to the
        // ordinary stream-copy-and-verify path with no visible
        // difference to the caller. Skipped, rather than false-failed,
        // on the rare CoW-everywhere box where it can't be exercised.
        let dir = tempfile::tempdir().unwrap();
        if cow_supported(dir.path()) {
            eprintln!(
                "skipping: filesystem supports reflink, cannot exercise the deterministic fallback path here"
            );
            return;
        }

        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"hello world").unwrap();
        let dest_dir = dir.path().join("dest");

        let outcome =
            transfer_file(&src, &dest_dir, None, false, true, &Progress::hidden()).unwrap();

        assert_eq!(outcome, TransferOutcome::Transferred);
        assert_eq!(fs::read(dest_dir.join("clip.mp4")).unwrap(), b"hello world");
        assert!(src.exists(), "fallback must never touch the source");
        assert!(!dest_dir.join("clip.mp4.part").exists());
    }

    #[test]
    fn reflink_disabled_never_attempts_a_clone() {
        // Spec: "Reflink disabled always stream-copies" / "Reflink
        // override forces cloning off" (task 6.2). Unlike the fallback
        // test above, this must hold regardless of the underlying
        // filesystem's CoW support: `reflink: false` short-circuits the
        // attempt entirely.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"hello world").unwrap();
        let dest_dir = dir.path().join("dest");

        let outcome =
            transfer_file(&src, &dest_dir, None, false, false, &Progress::hidden()).unwrap();

        assert_eq!(outcome, TransferOutcome::Transferred);
        assert_eq!(fs::read(dest_dir.join("clip.mp4")).unwrap(), b"hello world");
    }

    #[test]
    fn reflink_success_path_clones_when_cow_available() {
        // Spec: "Same-filesystem transfer is reflinked without hashing"
        // (task 6.3). Runtime-detects CoW support and skips otherwise
        // (design Open Questions) rather than assuming the test
        // environment can exercise it.
        let dir = tempfile::tempdir().unwrap();
        if !cow_supported(dir.path()) {
            eprintln!("skipping: filesystem does not support reflink/CoW");
            return;
        }

        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"hello world").unwrap();
        let dest_dir = dir.path().join("dest");

        let recorded_at: Timestamp = "2026-07-09T07:41:03Z".parse().unwrap();
        let outcome = transfer_file(
            &src,
            &dest_dir,
            Some(recorded_at),
            false,
            true,
            &Progress::hidden(),
        )
        .unwrap();

        assert_eq!(outcome, TransferOutcome::Reflinked);
        let dest_path = dest_dir.join("clip.mp4");
        assert_eq!(fs::read(&dest_path).unwrap(), b"hello world");
        assert!(!dest_dir.join("clip.mp4.part").exists());
        let mtime = fs::metadata(&dest_path).unwrap().modified().unwrap();
        assert_eq!(Timestamp::try_from(mtime).unwrap(), recorded_at);

        // Independent inode (design D6): a reflink clone is a distinct
        // inode sharing extents copy-on-write, so mutating the
        // destination must never affect the source — unlike a hard
        // link, which shares one inode under two names.
        fs::write(&dest_path, b"mutated").unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"hello world");
    }

    #[test]
    fn reflinked_group_is_deletion_eligible() {
        // Spec: "Reflinked files are deletion candidates" (task 6.4) —
        // a successful clone is verified by construction (design D3),
        // so it must make its group a source-deletion candidate exactly
        // as a stream-copied-and-verified file does. Runtime-skipped
        // like 6.3 when CoW is unavailable.
        let dir = tempfile::tempdir().unwrap();
        if !cow_supported(dir.path()) {
            eprintln!("skipping: filesystem does not support reflink/CoW");
            return;
        }

        let src = dir.path().join("clip.mp4");
        fs::write(&src, b"hello world").unwrap();
        let dest_dir = dir.path().join("dest");

        let group = MediaGroup {
            name: "session".to_string(),
            files: vec![MediaFile {
                path: src.clone(),
                size: 11,
                recorded_at: None,
            }],
            timestamp: ts(0),
            markers: vec![],
            geo: None,
            context: HashMap::new(),
            sidecar: None,
        };
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group,
                verdict: Verdict::Keep,
                destination: Some(dest_dir.clone()),
                quarantine_path: None,
            }],
        };

        let report =
            execute_inner(&plan, true, true, false, true, false, &Progress::hidden()).unwrap();

        assert!(matches!(
            report.groups[0].files[0].outcome,
            TransferOutcome::Reflinked
        ));
        assert!(
            report.groups[0].deleted_from_source,
            "a reflinked group must be deletion-eligible: verified by construction"
        );
        assert!(!src.exists(), "source should be deleted once cloned");
    }

    #[test]
    fn reflinked_outcome_advances_progress_by_full_size() {
        // Companion to `skipped_identical_still_advances_progress_by_full_size`
        // (design D7, task 3.5): a reflink moves no bytes through
        // `copy_and_hash`, so `execute_inner` must tick the bar itself
        // or a reflink-heavy run would leave it stalled near 0%.
        let dir = tempfile::tempdir().unwrap();
        if !cow_supported(dir.path()) {
            eprintln!("skipping: filesystem does not support reflink/CoW");
            return;
        }

        let src = dir.path().join("clip.mp4");
        let content = b"hello world";
        fs::write(&src, content).unwrap();
        let dest_dir = dir.path().join("dest");

        let group = MediaGroup {
            name: "a".to_string(),
            files: vec![MediaFile {
                path: src.clone(),
                size: content.len() as u64,
                recorded_at: None,
            }],
            timestamp: ts(0),
            markers: vec![],
            geo: None,
            context: HashMap::new(),
            sidecar: None,
        };
        let plan = ImportPlan {
            actions: vec![PlannedAction {
                group,
                verdict: Verdict::Keep,
                destination: Some(dest_dir.clone()),
                quarantine_path: None,
            }],
        };

        let progress = Progress::new(true, "Importing");
        let report = execute(&plan, false, false, false, true, &progress).unwrap();
        assert!(matches!(
            report.groups[0].files[0].outcome,
            TransferOutcome::Reflinked
        ));
        assert_eq!(progress.position(), content.len() as u64);
    }
}
