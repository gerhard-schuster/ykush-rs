// SPDX-License-Identifier: Apache-2.0
//! Control library for the Yepkit YKUSH3 switchable USB hub.
//!
//! An independent implementation of the YKUSH3 control protocol, written
//! against Yepkit's published documentation rather than derived from their
//! `ykushcmd`. The `ykushctrl` binary in this package is a command line front
//! end to this library; a Rust program can skip it and drive the board
//! directly:
//!
//! ```no_run
//! use ykush::{Port, Ykush3};
//!
//! # fn main() -> ykush::Result<()> {
//! let board = Ykush3::open(None)?;
//! board.port_down(Port::Downstream(2))?;
//! println!("{}", board.port_status(Port::Downstream(2))?);
//! # Ok(())
//! # }
//! ```
//!
//! Code built on this library can be tested without hardware: behind the
//! `fake` feature, `fake::FakeBoard` stands in for a board, answers from a
//! prepared queue and records what would have been sent. The feature is off
//! by default, so the plain library carries no test gear.

// Built and run against a board on macOS on Apple silicon and on Linux and
// FreeBSD on aarch64; anywhere else it refuses to compile rather than produce
// a binary nobody has tried. x86_64 stays out only because no board has been
// driven from it.
#[cfg(not(all(
    target_arch = "aarch64",
    any(target_os = "macos", target_os = "linux", target_os = "freebsd")
)))]
compile_error!("ykush is for macOS on Apple silicon and for Linux and FreeBSD on aarch64");

mod device;
mod error;
mod lock;
mod sanitize;
mod ykush3;

#[cfg(any(test, feature = "fake"))]
pub mod fake;

pub use crate::device::{
    detached_hint, list, Board, Report, Transport, PRODUCT_ID, REPORT_SIZE, VENDOR_ID,
};
pub use crate::error::{Error, Result};
pub use crate::lock::DEFAULT_WAIT;
pub use crate::sanitize::sanitize;
pub use crate::ykush3::{Port, PortStatus, PowerOnState, Version, Ykush3, I2C_MAX_BYTES};
