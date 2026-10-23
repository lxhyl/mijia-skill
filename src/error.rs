use std::fmt;

/// Exit codes are part of the CLI contract: agents branch on them (see SKILL.md).
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_NEEDS_CONFIRMATION: i32 = 3;
pub const EXIT_LOGIN_REQUIRED: i32 = 4;

#[derive(Debug)]
pub enum Error {
    /// No usable login cache. Only the account owner can fix this, by running `mijia login`.
    LoginRequired(String),
    /// A write to a high-risk device was attempted without `--confirmed`.
    NeedsConfirmation(String),
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn fail<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Other(msg.into()))
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::LoginRequired(_) => EXIT_LOGIN_REQUIRED,
            Error::NeedsConfirmation(_) => EXIT_NEEDS_CONFIRMATION,
            Error::Other(_) => EXIT_ERROR,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::LoginRequired(m) | Error::NeedsConfirmation(m) | Error::Other(m) => f.write_str(m),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Other(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Other(format!("unexpected JSON: {e}"))
    }
}

impl From<ureq::Error> for Error {
    fn from(e: ureq::Error) -> Self {
        Error::Other(format!("network error: {e}"))
    }
}
