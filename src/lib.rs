// SPDX-License-Identifier: Apache-2.0
//! Control library for the Yepkit YKUSH3 switchable USB hub.
//!
//! An independent implementation of the YKUSH3 control protocol, written
//! against Yepkit's published documentation rather than derived from their
//! `ykushcmd`. The `ykush3cmd` binary in this package is a command line front
//! end to this library; a Rust program can skip it and drive the board
//! directly:
//!
//! ```no_run
//! use ykush3::{Port, Ykush3};
//!
//! # fn main() -> ykush3::Result<()> {
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

// This library is built and run against a board on two systems: macOS on
// Apple silicon, and Linux on 64 bit Arm (a Raspberry Pi 4 under Debian).
// Anywhere else it refuses to compile rather than produce a binary nobody has
// tried. Nothing in the code is tied to the processor; Linux on x86_64 stays
// out only because no board has been driven from it, and taking it in is this
// line plus a hardware run.
#[cfg(not(all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux"))))]
compile_error!("ykush3 is for macOS on Apple silicon and for Linux on aarch64");

mod device;
mod error;
mod sanitize;
mod ykush3;

#[cfg(any(test, feature = "fake"))]
pub mod fake;

pub use crate::device::{
    detached_hint, list, Board, Report, Transport, PRODUCT_ID, REPORT_SIZE, VENDOR_ID,
};
pub use crate::error::{Error, Result};
pub use crate::sanitize::sanitize;
pub use crate::ykush3::{Port, PortStatus, PowerOnState, Version, Ykush3, I2C_MAX_BYTES};
