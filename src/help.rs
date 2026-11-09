// SPDX-License-Identifier: Apache-2.0
//! Help output.

use std::io::Write;

use ykush::Result;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn print_version(out: &mut impl Write) -> Result<()> {
    writeln!(out, "ykushctrl {VERSION}")?;
    Ok(())
}

pub fn print_usage(out: &mut impl Write, program: &str) -> Result<()> {
    writeln!(out, "Usage: {program} [-s SERIAL] [--no-wait] COMMAND")?;
    writeln!(
        out,
        "Switch the ports of a YKUSH3 hub and read back its state."
    )?;
    Ok(())
}

pub fn print_options(out: &mut impl Write) -> Result<()> {
    write!(
        out,
        "
Ports. The number 4 addresses the separate 5V output, the letter a every
downstream port at once.

  -u PORT            power the port up            PORT = 1|2|3|4|a
  -d PORT            power the port down          PORT = 1|2|3|4|a
  -g PORT            report whether it is on      PORT = 1|2|3|4|a
  -on                shorthand for -u 4
  -off               shorthand for -d 4
  -c PORT STATE      what the port does after a reset or a power cut:
                     0 stay off, 1 come up, 2 return to where it was.
                     PORT = 1|2|3|e, where e is the 5V output

GPIO. Three pins, driven from the host once the control interface is off.

  -r PIN             read the pin and print 0 or 1     PIN = 1|2|3
  -w PIN LEVEL       drive the pin high or low         LEVEL = 0|1
  --gpio enable      hand the pins over to the board, which then switches
                     the like-numbered port according to the level it sees
  --gpio disable     take the pins back for -r and -w

I2C. Enabling either mode makes the board re-enumerate; it stays enabled
across resets and power cuts. Addresses are hexadecimal, 7 bit, aligned to
the top of the byte: write 0xA2 for address 0x51.

  --i2c-slave enable|disable    let an outside master switch the ports
  --i2c-master enable|disable   let this host reach devices on the bus
  --i2c-set-address ADDR        the address the board answers to as a slave
  --i2c-write ADDR BYTE...      put bytes on the bus, at most 60
  --i2c-read ADDR COUNT         take bytes off the bus, at most 60,
                                one per line as a decimal number

Board.

  -l                 list the serial number of every attached board
  -s SERIAL          send the command to that board rather than to the
                     first one the system enumerates
  --no-wait          fail at once when the board is in use, instead of
                     waiting up to 5 seconds for it
  --firmware-version, --bootloader-version
  --reset            restart the board
  --boot             stop in the bootloader, ready for a firmware image
  -v, --version      print the version of this program
  -h, --help         print this text

Exit status: 0 on success, or a sysexits(3) code (64 to 77) saying what to do next.
"
    )?;
    Ok(())
}

pub fn print_all(out: &mut impl Write, program: &str) -> Result<()> {
    print_version(out)?;
    print_usage(out, program)?;
    print_options(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ykush::Error;

    /// A writer whose every write fails, to exercise the output error paths.
    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "writer closed",
            ))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_help_names_the_version_the_usage_and_the_options() {
        let mut out = Vec::new();

        print_all(&mut out, "ykushctrl").unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(&format!("ykushctrl {VERSION}")));
        assert!(text.contains("Usage: ykushctrl"));
        assert!(text.contains("--i2c-write"));
        assert!(text.contains("--bootloader-version"));
        assert!(text.contains("--no-wait"));
        // The text names the figure; this keeps the two together.
        assert!(text.contains("up to 5 seconds"));
        assert_eq!(ykush::DEFAULT_WAIT, std::time::Duration::from_secs(5));
    }

    #[test]
    fn a_failing_writer_is_reported_instead_of_panicking() {
        // Every part on its own, so no early failure hides a later one.
        assert!(matches!(
            print_version(&mut FailingWriter),
            Err(Error::Io(_))
        ));
        assert!(matches!(
            print_usage(&mut FailingWriter, "ykushctrl"),
            Err(Error::Io(_))
        ));
        assert!(matches!(
            print_options(&mut FailingWriter),
            Err(Error::Io(_))
        ));
        assert!(matches!(
            print_all(&mut FailingWriter, "ykushctrl"),
            Err(Error::Io(_))
        ));

        // Only the writes fail; a flush goes through.
        assert!(FailingWriter.flush().is_ok());
    }
}
