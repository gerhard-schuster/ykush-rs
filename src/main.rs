// SPDX-License-Identifier: Apache-2.0
//! Command line front end for the Yepkit YKUSH3 switchable USB hub.
//!
//! The board handling itself lives in the `ykush` library target of this
//! package; what remains here is parsing the command line and printing.

#![forbid(unsafe_code)]

mod cli;
mod help;

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use cli::Command;
use ykush::{Error, Result, Transport, Ykush3};

fn main() -> ExitCode {
    // Not `env::args()`: that panics on an argument that is not valid Unicode,
    // where a lossy conversion turns it into a readable usage error. Sanitizing
    // right here keeps control characters a caller put into argv from reaching
    // the terminal when an error message echoes the argument, the same filter
    // the device serial numbers go through. An empty argv, which `execve`
    // permits, must not panic either.
    let args: Vec<String> = std::env::args_os()
        .map(|arg| ykush::sanitize(&arg.to_string_lossy()))
        .collect();
    let program = program_name(&args);
    let mut out = io::stdout().lock();

    finish(
        &program,
        run(&program, args.get(1..).unwrap_or_default(), &mut out),
        &mut io::stderr(),
    )
}

fn finish(program: &str, result: Result<()>, err_out: &mut impl Write) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        // A downstream pipe reader that closed early (e.g. `| head -n 1`) is
        // normal shell usage, not an I/O error to report.
        Err(Error::Io(err)) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(err) => {
            if let Error::Usage(msg) = &err {
                let _ = writeln!(err_out, "{msg}\n");
                let _ = help::print_usage(err_out, program);
                let _ = writeln!(err_out, "\nRun '{program} -h' for the list of options.");
            } else {
                let _ = writeln!(err_out, "{err}");
            }
            ExitCode::from(err.exit_code())
        }
    }
}

fn run(program: &str, args: &[String], out: &mut impl Write) -> Result<()> {
    let invocation = cli::parse(args)?;

    match &invocation.command {
        // Commands that do not address a single board.
        Command::Help => help::print_all(out, program),
        Command::Version => help::print_version(out),
        Command::List => list_boards(
            out,
            &mut io::stderr(),
            &ykush::list()?,
            ykush::detached_hint,
        ),

        command => {
            let wait = if invocation.no_wait {
                Duration::ZERO
            } else {
                ykush::DEFAULT_WAIT
            };
            let board = Ykush3::open_within(invocation.serial.as_deref(), wait)?;
            execute(&board, command, out)
        }
    }
}

/// Runs a command against an opened board and prints what it returns.
fn execute<T: Transport>(board: &Ykush3<T>, command: &Command, out: &mut impl Write) -> Result<()> {
    match command {
        Command::PortUp(port) => board.port_up(*port)?,
        Command::PortDown(port) => board.port_down(*port)?,
        Command::PortStatus(port) => {
            for &p in port.ports() {
                writeln!(out, "{}", board.port_status(p)?)?;
            }
        }

        Command::Config(port, state) => board.config_port(*port, *state)?,

        Command::ReadIo(gpio) => writeln!(out, "{}", board.read_io(*gpio)?)?,
        Command::WriteIo(gpio, high) => board.write_io(*gpio, *high)?,
        Command::GpioControl(enable) => board.gpio_control(*enable)?,

        Command::Reset => board.reset()?,
        Command::Bootloader => board.enter_bootloader()?,

        Command::FirmwareVersion => writeln!(out, "Firmware {}", board.firmware_version()?)?,
        Command::BootloaderVersion => writeln!(out, "Bootloader {}", board.bootloader_version()?)?,

        Command::I2cSlave(enable) => board.i2c_slave(*enable)?,
        Command::I2cMaster(enable) => board.i2c_master(*enable)?,
        Command::I2cSetAddress(address) => board.i2c_set_address(*address)?,
        Command::I2cWrite(address, data) => board.i2c_write(*address, data)?,
        Command::I2cRead(address, len) => {
            for byte in board.i2c_read(*address, *len)? {
                writeln!(out, "{byte}")?;
            }
        }

        // Handled by run() before a board is opened.
        Command::Help | Command::Version | Command::List => {
            unreachable!("command does not address a board")
        }
    }

    Ok(())
}

/// Prints the boards found. With none found, a board on the bus that cannot
/// be reached is explained on stderr; the listing itself still succeeds, as an
/// empty one always has. The hint is fetched only then, which costs a scan of
/// sysfs that a non-empty list does not need.
fn list_boards(
    out: &mut impl Write,
    err: &mut impl Write,
    serials: &[String],
    hint: impl FnOnce() -> Option<String>,
) -> Result<()> {
    print_boards(out, serials)?;
    if serials.is_empty() {
        if let Some(hint) = hint() {
            writeln!(err, "{hint}")?;
        }
    }
    Ok(())
}

fn print_boards(out: &mut impl Write, serials: &[String]) -> Result<()> {
    writeln!(out, "YKUSH3 boards on this host:")?;
    if serials.is_empty() {
        writeln!(out, "  none")?;
    }
    for (i, serial) in serials.iter().enumerate() {
        writeln!(out, "  {:>2}  {serial}", i + 1)?;
    }
    writeln!(out)?;

    Ok(())
}

fn program_name(args: &[String]) -> String {
    args.first()
        .map(|p| {
            Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.clone())
        })
        .unwrap_or_else(|| env!("CARGO_PKG_NAME").to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ykush::fake::FakeBoard;
    use ykush::{Gpio, Port};

    /// Runs a command against a board answering with `answer` and returns what
    /// was printed.
    fn output(answer: &[u8], command: Command) -> String {
        let board = Ykush3::with_transport(FakeBoard::answering(answer));
        let mut out = Vec::new();

        execute(&board, &command, &mut out).expect("command should succeed");

        String::from_utf8(out).expect("output should be utf-8")
    }

    #[test]
    fn the_port_state_is_printed_like_in_the_cpp_application() {
        assert_eq!(
            output(&[0x01, 0x11], Command::PortStatus(Port::Downstream1)),
            "Port 1: on\n"
        );
        assert_eq!(
            output(&[0x01, 0x02], Command::PortStatus(Port::Downstream2)),
            "Port 2: off\n"
        );
    }

    #[test]
    fn the_state_of_all_ports_lists_each_downstream_port() {
        let fake = FakeBoard::answering(&[0x01, 0x11])
            .then(&[0x01, 0x02])
            .then(&[0x01, 0x13]);
        let board = Ykush3::with_transport(fake);
        let mut out = Vec::new();

        execute(&board, &Command::PortStatus(Port::All), &mut out).unwrap();

        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Port 1: on\nPort 2: off\nPort 3: on\n"
        );
    }

    #[test]
    fn a_gpio_read_prints_the_pin_level() {
        // The C++ application only puts the level into the exit code.
        assert_eq!(
            output(&[0x01, 0x30, 0x01, 0x01], Command::ReadIo(Gpio::Gpio1)),
            "1\n"
        );
        assert_eq!(
            output(&[0x01, 0x30, 0x01, 0x00], Command::ReadIo(Gpio::Gpio1)),
            "0\n"
        );
    }

    #[test]
    fn versions_are_printed_with_the_original_wording() {
        assert_eq!(
            output(&[0x01, 0x61, 1, 4, 0], Command::FirmwareVersion),
            "Firmware 1.4.0\n"
        );
        assert_eq!(
            output(&[0x01, 0x61, 0, 10, 0], Command::BootloaderVersion),
            "Bootloader 0.10.0\n"
        );
    }

    #[test]
    fn an_i2c_read_prints_one_decimal_byte_per_line() {
        assert_eq!(
            output(
                &[0x01, 0x52, 0x03, 0x01, 0x0a, 0xff],
                Command::I2cRead(0x20, 3)
            ),
            "1\n10\n255\n"
        );
    }

    #[test]
    fn switching_commands_print_nothing() {
        assert_eq!(output(&[0x01], Command::PortUp(Port::Downstream1)), "");
        assert_eq!(output(&[0x01], Command::WriteIo(Gpio::Gpio1, true)), "");
        assert_eq!(output(&[0x01, 0x51], Command::I2cSlave(true)), "");
    }

    #[test]
    fn a_failing_command_is_reported_and_prints_nothing() {
        let board = Ykush3::with_transport(FakeBoard::failing(Error::NoResponse));
        let mut out = Vec::new();

        let result = execute(&board, &Command::PortStatus(Port::Downstream1), &mut out);

        assert!(matches!(result, Err(Error::NoResponse)));
        assert!(out.is_empty());
    }

    #[test]
    fn attached_boards_are_listed_with_their_serial_numbers() {
        let mut out = Vec::new();

        print_boards(&mut out, &["YK00001".to_owned(), "YK00002".to_owned()]).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("YKUSH3 boards on this host:"));
        assert!(text.contains("1  YK00001"));
        assert!(text.contains("2  YK00002"));
    }

    #[test]
    fn an_empty_board_list_says_so() {
        let mut out = Vec::new();

        print_boards(&mut out, &[]).unwrap();

        assert!(String::from_utf8(out).unwrap().contains("none"));
    }

    #[test]
    fn a_detached_board_is_explained_on_stderr_and_the_listing_succeeds() {
        let (mut out, mut err) = (Vec::new(), Vec::new());

        list_boards(&mut out, &mut err, &[], || {
            Some("no driver on 1-1.4.4:1.0".into())
        })
        .expect("an empty listing is still a listing");

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("YKUSH3 boards on this host:") && text.contains("none"));
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "no driver on 1-1.4.4:1.0\n"
        );
    }

    #[test]
    fn a_non_empty_listing_does_not_look_for_detached_boards() {
        let (mut out, mut err) = (Vec::new(), Vec::new());

        list_boards(&mut out, &mut err, &["YK00001".to_owned()], || {
            panic!("the hint is only needed for an empty list")
        })
        .unwrap();

        assert!(err.is_empty());
    }

    #[test]
    fn the_help_names_the_program_it_was_called_as() {
        let mut out = Vec::new();

        run("ykushcmd", &[], &mut out).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Usage: ykushcmd"));
        assert!(text.contains("--i2c-read"));
    }

    #[test]
    fn the_program_name_is_taken_from_the_invocation_path() {
        assert_eq!(
            program_name(&["/usr/bin/ykushctrl".to_owned()]),
            "ykushctrl"
        );
        assert_eq!(program_name(&[]), "ykushctrl");
    }

    #[test]
    fn every_command_that_addresses_the_board_is_dispatched() {
        use ykush::PowerOnState;

        let answered: Vec<(Vec<u8>, Command)> = vec![
            (vec![0x01], Command::PortUp(Port::All)),
            (vec![0x01], Command::PortDown(Port::Downstream2)),
            (
                vec![0x01],
                Command::Config(Port::Downstream1, PowerOnState::Persist),
            ),
            (vec![0x01], Command::WriteIo(Gpio::Gpio2, false)),
            (vec![0x01], Command::GpioControl(false)),
            (vec![0x01, 0x51], Command::I2cSlave(true)),
            (vec![0x01, 0x51], Command::I2cMaster(false)),
            (vec![0x01, 0x51], Command::I2cSetAddress(0xa6)),
            (vec![0x01, 0x52], Command::I2cWrite(0x20, vec![0x01])),
            (vec![0x01, 0x61, 1, 2, 0], Command::BootloaderVersion),
        ];

        for (answer, command) in answered {
            let board = Ykush3::with_transport(FakeBoard::answering(&answer));

            execute(&board, &command, &mut Vec::new())
                .unwrap_or_else(|err| panic!("{command:?} failed: {err}"));

            assert_eq!(board.transport().sent_count(), 1, "{command:?}");
        }

        // These two make the board reboot, so it never answers.
        for command in [Command::Reset, Command::Bootloader] {
            let board = Ykush3::with_transport(FakeBoard::mute());

            execute(&board, &command, &mut Vec::new())
                .unwrap_or_else(|err| panic!("{command:?} failed: {err}"));

            assert_eq!(board.transport().sent_count(), 1, "{command:?}");
        }
    }

    #[test]
    fn the_version_is_printed_without_addressing_a_board() {
        let mut out = Vec::new();

        run("ykushctrl", &["-v".to_owned()], &mut out).unwrap();

        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("ykushctrl {}\n", help::VERSION)
        );
    }

    /// Needs the HID stack of the operating system, but no board.
    #[test]
    #[ignore = "needs the HID stack of the operating system"]
    fn listing_boards_reaches_the_hid_stack() {
        let mut out = Vec::new();

        run("ykushctrl", &["-l".to_owned()], &mut out).expect("listing should work");

        assert!(String::from_utf8(out)
            .unwrap()
            .contains("YKUSH3 boards on this host"));
    }

    #[test]
    fn an_unparsable_command_line_never_reaches_a_board() {
        let result = run(
            "ykushctrl",
            &["-u".to_owned(), "9".to_owned()],
            &mut Vec::new(),
        );

        assert!(matches!(result, Err(Error::Usage(_))));
    }

    #[test]
    fn a_broken_pipe_exits_cleanly_without_an_error_message() {
        let pipe = Error::Io(io::Error::from(io::ErrorKind::BrokenPipe));
        let mut err = Vec::new();
        assert_eq!(finish("ykushctrl", Err(pipe), &mut err), ExitCode::SUCCESS);
        assert!(err.is_empty());
    }

    #[test]
    fn another_output_error_is_reported_with_its_exit_code() {
        let failed = Error::Io(io::Error::other("disk full"));
        let mut err = Vec::new();
        assert_eq!(
            finish("ykushctrl", Err(failed), &mut err),
            ExitCode::from(74)
        );
        assert!(String::from_utf8(err).unwrap().contains("disk full"));
    }

    /// Needs a YKUSH3 attached.
    /// Run with `cargo test -- --ignored --test-threads=1`.
    #[test]
    #[ignore = "needs a YKUSH3 attached"]
    fn a_read_only_command_reaches_a_real_board() {
        let mut out = Vec::new();

        run("ykushctrl", &["--firmware-version".to_owned()], &mut out).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("Firmware "), "{text:?}");
    }
}
