use std::path::PathBuf;

/// Library error type. Every variant that touches the filesystem or a
/// user-authored file carries the path involved, so error messages can
/// name exactly what failed without the caller re-deriving it.
///
/// `main.rs` is the only place this gets converted to an exit code; the
/// rest of the crate just returns `Result<_, Error>` (thiserror, not
/// anyhow — a library should give callers a typed error to match on).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("config error: {0}")]
    Config(String),

    #[error("template error: {0}")]
    Template(String),

    #[error("verification failed for {src} -> {dest}: hashes did not match")]
    VerifyMismatch { src: PathBuf, dest: PathBuf },
}

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Exit codes per design D7: 0 success (including "nothing to import"),
/// 1 failure during import, 2 configuration/usage error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success = 0,
    Failure = 1,
    UsageOrConfig = 2,
}

impl Error {
    /// Classifies this error for the process exit code. Config/Template
    /// errors are usage mistakes (exit 2); everything else is a runtime
    /// failure during scan/import (exit 1).
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Error::Config(_) | Error::Template(_) => ExitCode::UsageOrConfig,
            Error::Io { .. } | Error::VerifyMismatch { .. } => ExitCode::Failure,
        }
    }
}
