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
    /// The connection broke after a write was sent, so it may or may not have reached the device.
    Interrupted(String),
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
            Error::Interrupted(_) | Error::Other(_) => EXIT_ERROR,
        }
    }

    /// What `--json` prints on stdout for a failed command, so scripts can always parse stdout.
    pub fn to_json(&self) -> serde_json::Value {
        let kind = match self {
            Error::LoginRequired(_) => "login_required",
            Error::NeedsConfirmation(_) => "needs_confirmation",
            Error::Interrupted(_) | Error::Other(_) => "error",
        };
        serde_json::json!({"error": self.to_string(), "kind": kind, "exit_code": self.exit_code()})
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::LoginRequired(m) | Error::NeedsConfirmation(m) | Error::Interrupted(m) | Error::Other(m) => {
                f.write_str(m)
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_errors_carry_kind_and_exit_code() {
        let e = Error::NeedsConfirmation("ask first".into()).to_json();
        assert_eq!(e, serde_json::json!({"error": "ask first", "kind": "needs_confirmation", "exit_code": 3}));
        assert_eq!(Error::LoginRequired("x".into()).to_json()["exit_code"], 4);
        assert_eq!(Error::Other("x".into()).to_json()["kind"], "error");
    }
}
