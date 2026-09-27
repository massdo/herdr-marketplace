use std::fmt;
use std::io;

/// Pane and socket errors. Transport whose effect is unknown is `Uncertain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppError {
    OriginMissing,
    SnapshotUnreadable {
        detail: String,
    },
    OriginChanged,
    NoWorkingTarget,
    SeveralSidebars,
    Herdr {
        method: String,
        code: String,
        message: String,
    },
    Uncertain {
        method: String,
        message: String,
    },
    Io {
        message: String,
    },
}

impl AppError {
    pub fn herdr(
        method: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::Herdr {
            method: method.into(),
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn uncertain(method: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Uncertain {
            method: method.into(),
            message: message.into(),
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Uncertain { .. } => 2,
            _ => 1,
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OriginMissing => write!(f, "origin workspace, tab or pane is missing"),
            Self::SnapshotUnreadable { detail } => {
                write!(f, "pane snapshot cannot be interpreted: {detail}")
            }
            Self::OriginChanged => {
                write!(
                    f,
                    "captured origin pane is no longer present in the pane list"
                )
            }
            Self::NoWorkingTarget => {
                write!(f, "no working pane is available as a split target")
            }
            Self::SeveralSidebars => {
                write!(
                    f,
                    "several marketplace sidebars are recognised in the target tab"
                )
            }
            Self::Herdr {
                method,
                code,
                message,
            } => write!(f, "{method} failed ({code}): {message}"),
            Self::Uncertain { method, message } => write!(
                f,
                "{method} did not confirm: {message}. Inspect the layout before another attempt."
            ),
            Self::Io { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<io::Error> for AppError {
    fn from(error: io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
        }
    }
}
