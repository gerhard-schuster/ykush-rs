// SPDX-License-Identifier: Apache-2.0
//! Error type shared by the command line, transport and protocol layers.

use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

/// Not exhaustive: a new way to fail is not a breaking change for a caller
/// that already handles the ones it knows and passes the rest on.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The command line could not be interpreted.
    Usage(String),
    /// No board (or no board with the requested serial number) is attached.
    NotFound { serial: Option<String> },
    /// The HID stack refused the operation. Carries the message rather than
    /// the error type of the HID library, which would otherwise become part of
    /// this library's public interface and tie it to that library's major
    /// version.
    Hid(String),
    /// The board did not answer within the read timeout.
    NoResponse,
    /// The board could not be taken because another invocation had it, and
    /// it did not come free in time. On macOS the reason is a guess: the
    /// system refused the open without saying why.
    Busy {
        /// The board, by its serial number.
        serial: String,
        /// How long was waited; zero when waiting was not asked for.
        waited: std::time::Duration,
        /// Who has the board, as far as can be told.
        cause: String,
    },
    /// The board answered, but reported a failure or an unexpected report.
    Device(String),
    /// Writing the output failed.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Usage(msg) => write!(f, "{msg}"),
            Error::NotFound { serial: Some(s) } => {
                write!(f, "No YKUSH3 board with serial number {s} found")
            }
            Error::NotFound { serial: None } => write!(f, "No YKUSH3 board found"),
            Error::Hid(e) => write!(f, "USB HID error: {e}"),
            Error::NoResponse => write!(f, "No response from the board"),
            Error::Busy {
                serial,
                waited,
                cause,
            } if waited.is_zero() => write!(f, "The YKUSH3 {serial} is not available: {cause}"),
            Error::Busy {
                serial,
                waited,
                cause,
            } => write!(
                f,
                "The YKUSH3 {serial} is still not available after {:.1} s: {cause}",
                waited.as_secs_f64()
            ),
            Error::Device(msg) => write!(f, "{msg}"),
            Error::Io(e) => write!(f, "Output error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    /// The underlying error where one is kept. `Hid` deliberately carries
    /// only the message, see the variant.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<async_hid::HidError> for Error {
    fn from(e: async_hid::HidError) -> Self {
        Error::Hid(e.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hid_error() -> async_hid::HidError {
        async_hid::HidError::Message("boom".into())
    }

    fn io_error() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "boom")
    }

    #[test]
    fn every_error_says_what_went_wrong() {
        let cases: Vec<(Error, &str)> = vec![
            (Error::Usage("bad option".into()), "bad option"),
            (
                Error::NotFound {
                    serial: Some("YK00001".into()),
                },
                "serial number YK00001",
            ),
            (Error::NotFound { serial: None }, "No YKUSH3 board found"),
            (Error::NoResponse, "No response from the board"),
            (
                Error::Busy {
                    serial: "Y3N13808".into(),
                    waited: std::time::Duration::from_secs(5),
                    cause: "another ykush3cmd is using it".into(),
                },
                "The YKUSH3 Y3N13808 is still not available after 5.0 s: another ykush3cmd is using it",
            ),
            (
                Error::Busy {
                    serial: "Y3N13808".into(),
                    waited: std::time::Duration::ZERO,
                    cause: "another ykush3cmd is using it".into(),
                },
                "The YKUSH3 Y3N13808 is not available: another ykush3cmd is using it",
            ),
            (
                Error::Device("the board said no".into()),
                "the board said no",
            ),
            (Error::Hid("boom".into()), "USB HID error"),
            (Error::Io(io_error()), "Output error"),
        ];

        for (error, expected) in cases {
            let message = error.to_string();
            assert!(message.contains(expected), "{message:?} lacks {expected:?}");
        }
    }

    #[test]
    fn transport_and_output_failures_convert_into_an_error() {
        let hid: Error = hid_error().into();
        let io: Error = io_error().into();

        assert!(matches!(hid, Error::Hid(_)));
        assert!(matches!(io, Error::Io(_)));
    }

    #[test]
    fn an_output_error_exposes_its_source() {
        use std::error::Error as _;

        let io: Error = io_error().into();
        let device = Error::Device("the board said no".into());

        assert!(io.source().is_some(), "Io keeps the underlying error");
        assert!(device.source().is_none(), "Device carries only a message");
    }
}
