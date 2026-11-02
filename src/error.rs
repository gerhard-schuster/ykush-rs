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
    /// The board, or the HID stack, cannot be reached: the board is no longer
    /// connected, its HID driver is detached, it has no hidraw node, its node
    /// keeps changing, or the HID stack cannot list its devices.
    Unavailable(String),
    /// The system denied access to the board; on Linux the udev rule is
    /// missing or the user is not in its group.
    PermissionDenied,
    /// The HID transport failed: the stack refused the operation, or a report
    /// came back short. Carries the message rather than the error type of the
    /// HID library, which would otherwise become part of this library's public
    /// interface and tie it to that library's major version.
    Hid(String),
    /// The board did not answer within the read timeout.
    NoResponse,
    /// The board could not be taken because another invocation had it, and
    /// it did not come free in time. On macOS the system reports the board as
    /// held exclusively, but not by whom.
    Busy {
        /// The board, by its serial number.
        serial: String,
        /// How long was waited; zero when waiting was not asked for.
        waited: std::time::Duration,
        /// Who has the board, as far as can be told.
        cause: String,
    },
    /// The board said no: it answered with status `0x00` instead of an ACK,
    /// or an I2C transfer failed (not in master mode, transmission error).
    Rejected(String),
    /// The board's answer does not fit the request: wrong echo, wrong length,
    /// an unknown status. Usually an answer meant for another exchange.
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
            Error::Unavailable(msg) => write!(f, "{msg}"),
            Error::PermissionDenied => write!(f, "{}", crate::device::PERMISSION_DENIED),
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
            Error::Rejected(msg) => write!(f, "{msg}"),
            Error::Device(msg) => write!(f, "{msg}"),
            Error::Io(e) => write!(f, "Output error: {e}"),
        }
    }
}

impl Error {
    /// The exit status for this error, after sysexits(3), chosen by what
    /// the caller should do next:
    ///
    /// | Code | Error | What to do |
    /// |---|---|---|
    /// | 64 `EX_USAGE` | [`Error::Usage`] | correct the call |
    /// | 69 `EX_UNAVAILABLE` | [`Error::NotFound`], [`Error::Unavailable`], [`Error::Rejected`] | stop, check the board and its connection |
    /// | 74 `EX_IOERR` | [`Error::Hid`], [`Error::NoResponse`], [`Error::Io`] | retry if the command may be repeated |
    /// | 75 `EX_TEMPFAIL` | [`Error::Busy`] | retry later |
    /// | 76 `EX_PROTOCOL` | [`Error::Device`] | retry if the command may be repeated; if it recurs, check the board |
    /// | 77 `EX_NOPERM` | [`Error::PermissionDenied`] | fix the setup |
    ///
    /// sysexits(3) has no code for a device that refuses a command; such a
    /// refusal is a valid answer, not a protocol violation, so it gets the
    /// catch-all `EX_UNAVAILABLE`. `EX_NOPERM` is meant for permissions
    /// above the file system; a denied device node is one, but the cause is
    /// the setup, and that is what the code says.
    ///
    /// The match has no catch-all arm, so a new variant does not compile
    /// until it has a code.
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::Usage(_) => 64,
            Error::NotFound { .. } | Error::Unavailable(_) | Error::Rejected(_) => 69,
            Error::Hid(_) | Error::NoResponse | Error::Io(_) => 74,
            Error::Busy { .. } => 75,
            Error::Device(_) => 76,
            Error::PermissionDenied => 77,
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
    /// A board that is no longer connected is unavailable, not an I/O error
    /// that a retry would cure.
    fn from(e: async_hid::HidError) -> Self {
        match e {
            async_hid::HidError::NotConnected => {
                Error::Unavailable("The YKUSH3 is no longer connected".into())
            }
            e => Error::Hid(e.to_string()),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_board_that_is_gone_is_unavailable_not_an_io_error() {
        let error = Error::from(async_hid::HidError::NotConnected);
        assert!(matches!(error, Error::Unavailable(_)), "{error:?}");
        assert_eq!(error.exit_code(), 69);
        assert!(matches!(
            Error::from(async_hid::HidError::message("x")),
            Error::Hid(_)
        ));
    }

    #[test]
    fn every_error_maps_to_the_exit_code_of_what_the_caller_should_do() {
        let busy = Error::Busy {
            serial: "Y3TEST".into(),
            waited: std::time::Duration::ZERO,
            cause: "held".into(),
        };
        let cases = [
            (Error::Usage("x".into()), 64),
            (Error::NotFound { serial: None }, 69),
            (Error::Unavailable("x".into()), 69),
            (Error::PermissionDenied, 77),
            (busy, 75),
            (Error::Hid("x".into()), 74),
            (Error::NoResponse, 74),
            (Error::Device("x".into()), 76),
            (Error::Io(std::io::Error::other("x")), 74),
            (Error::Rejected("x".into()), 69),
        ];
        for (error, code) in cases {
            assert_eq!(error.exit_code(), code, "{error:?}");
        }
    }

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
                    cause: "another ykushctrl is using it".into(),
                },
                "The YKUSH3 Y3N13808 is still not available after 5.0 s: another ykushctrl is using it",
            ),
            (
                Error::Busy {
                    serial: "Y3N13808".into(),
                    waited: std::time::Duration::ZERO,
                    cause: "another ykushctrl is using it".into(),
                },
                "The YKUSH3 Y3N13808 is not available: another ykushctrl is using it",
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
