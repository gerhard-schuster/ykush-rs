// SPDX-License-Identifier: Apache-2.0
//! Command line parsing.
//!
//! The grammar of the C++ application is kept as it is: a token starting with a
//! dash opens an option, every following token that does not start with a dash
//! is a parameter of that option. Options such as `-on` or `-off` are therefore
//! single options and not a group of short flags.
//!
//! Parsing produces an [`Invocation`] and touches no hardware, so the whole
//! command line surface can be tested on its own.

use ykush::{Error, Gpio, Port, PowerOnState, Result};

/// What the user asked the application to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    List,
    PortUp(Port),
    PortDown(Port),
    PortStatus(Port),
    Config(Port, PowerOnState),
    ReadIo(Gpio),
    WriteIo(Gpio, bool),
    GpioControl(bool),
    Reset,
    Bootloader,
    FirmwareVersion,
    BootloaderVersion,
    I2cSlave(bool),
    I2cMaster(bool),
    I2cSetAddress(u8),
    I2cWrite(u8, Vec<u8>),
    I2cRead(u8, u8),
}

/// A command together with the board it is addressed to.
#[derive(Debug, PartialEq, Eq)]
pub struct Invocation {
    pub serial: Option<String>,
    /// `--no-wait`: fail at once when another invocation is using the board,
    /// rather than waiting for it.
    pub no_wait: bool,
    pub command: Command,
}

/// Options that modify how a command runs rather than being one.
fn is_modifier(opt: &Opt) -> bool {
    opt.name == "-s" || opt.name == NO_WAIT
}

const NO_WAIT: &str = "--no-wait";

/// One option of the command line together with its parameters.
#[derive(Debug, PartialEq, Eq)]
struct Opt {
    name: String,
    params: Vec<String>,
}

impl Opt {
    /// Parameter at `index`, or a usage error naming what was expected.
    fn param(&self, index: usize, expected: &str) -> Result<&str> {
        self.params
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| Error::Usage(format!("Option {} expects {expected}", self.name)))
    }
}

/// Interprets the arguments, without the program name.
pub fn parse(args: &[String]) -> Result<Invocation> {
    // The C++ application is driven as `ykushcmd ykush3 ...`, so that one
    // leading board name is accepted and skipped. Only that word: any other
    // free-standing token is a mistake worth reporting, not ignoring.
    let args = match args.first() {
        Some(name) if name == "ykush3" => &args[1..],
        _ => args,
    };
    let opts = split(args)?;

    if opts.is_empty() {
        return Ok(Invocation {
            serial: None,
            no_wait: false,
            command: Command::Help,
        });
    }

    // The serial number selects the board for whichever command follows, so it
    // is picked up before the command itself. Two serial numbers on one line
    // have no meaning - silently taking the first could address the wrong
    // board.
    let mut serials = opts.iter().filter(|o| o.name == "-s");
    let serial = match serials.next() {
        Some(opt) => Some(opt.param(0, "a serial number")?.to_owned()),
        None => None,
    };
    if serials.next().is_some() {
        return Err(Error::Usage(
            "The serial number can only be given once".into(),
        ));
    }

    // Saying it twice says nothing new, so that is accepted. A value is not:
    // the option takes none, and a stray word after it is a mistake.
    let mut no_wait = false;
    for opt in opts.iter().filter(|o| o.name == NO_WAIT) {
        if let Some(value) = opt.params.first() {
            return Err(Error::Usage(format!(
                "Option {NO_WAIT} takes no value, got '{value}'"
            )));
        }
        no_wait = true;
    }

    let opt = opts
        .iter()
        .find(|opt| !is_modifier(opt))
        .ok_or_else(|| Error::Usage("No command given".into()))?;

    Ok(Invocation {
        serial,
        no_wait,
        command: command(opt)?,
    })
}

/// Maps one option to a command.
fn command(opt: &Opt) -> Result<Command> {
    let command = match opt.name.as_str() {
        "-h" | "--help" => Command::Help,
        "-v" | "--version" => Command::Version,
        "-l" => Command::List,

        "-u" => Command::PortUp(port(opt.param(0, "a port number")?)?),
        "-d" => Command::PortDown(port(opt.param(0, "a port number")?)?),
        "-g" => Command::PortStatus(port(opt.param(0, "a port number")?)?),
        "-on" => Command::PortUp(Port::External),
        "-off" => Command::PortDown(Port::External),

        "-c" => Command::Config(
            port(opt.param(0, "a port number")?)?,
            power_on_state(opt.param(1, "a configuration value")?)?,
        ),

        "-r" => Command::ReadIo(gpio(opt.param(0, "a GPIO number")?)?),
        "-w" => Command::WriteIo(
            gpio(opt.param(0, "a GPIO number")?)?,
            level(opt.param(1, "a value of 0 or 1")?)?,
        ),
        "--gpio" => Command::GpioControl(enable_disable(
            opt.param(0, "enable or disable")?,
            "--gpio",
        )?),

        "--reset" => Command::Reset,
        "--boot" => Command::Bootloader,
        "--firmware-version" => Command::FirmwareVersion,
        "--bootloader-version" => Command::BootloaderVersion,

        "--i2c-slave" => Command::I2cSlave(enable_disable(
            opt.param(0, "enable or disable")?,
            "--i2c-slave",
        )?),
        "--i2c-master" => Command::I2cMaster(enable_disable(
            opt.param(0, "enable or disable")?,
            "--i2c-master",
        )?),
        "--i2c-set-address" => Command::I2cSetAddress(hex_byte(opt.param(0, "an I2C address")?)?),
        "--i2c-write" => {
            let address = hex_byte(opt.param(0, "an I2C address")?)?;
            if opt.params.len() < 2 {
                return Err(Error::Usage(
                    "Option --i2c-write expects at least one data byte".into(),
                ));
            }
            let data = opt.params[1..]
                .iter()
                .map(|b| hex_byte(b))
                .collect::<Result<Vec<u8>>>()?;
            Command::I2cWrite(address, data)
        }
        "--i2c-read" => Command::I2cRead(
            hex_byte(opt.param(0, "an I2C address")?)?,
            count(opt.param(1, "a number of bytes")?)?,
        ),

        other => return Err(Error::Usage(format!("Unknown option {other}"))),
    };

    Ok(command)
}

/// Splits the arguments into options. A token that belongs to no option is
/// rejected - the only free-standing token with a meaning is the leading
/// board name, and `parse()` takes that off beforehand.
fn split(args: &[String]) -> Result<Vec<Opt>> {
    let mut opts: Vec<Opt> = Vec::new();

    for arg in args {
        if arg.starts_with('-') && arg.len() > 1 {
            opts.push(Opt {
                name: arg.clone(),
                params: Vec::new(),
            });
        } else if let Some(last) = opts.last_mut() {
            last.params.push(arg.clone());
        } else {
            return Err(Error::Usage(format!("Unexpected argument '{arg}'")));
        }
    }

    Ok(opts)
}

/// Port of a switching or status command, `1|2|3|4|e|a`.
fn port(value: &str) -> Result<Port> {
    match value {
        "1" => Ok(Port::Downstream1),
        "2" => Ok(Port::Downstream2),
        "3" => Ok(Port::Downstream3),
        // The C++ application spells the external port `4` for switching and
        // `e` for configuration. Both spellings are accepted everywhere.
        "4" | "e" => Ok(Port::External),
        "a" => Ok(Port::All),
        _ => Err(Error::Usage(format!(
            "Invalid port number '{value}', expected 1, 2, 3, 4 (external 5V), e or a (all)"
        ))),
    }
}

/// GPIO pin number, `1|2|3`.
fn gpio(value: &str) -> Result<Gpio> {
    match value {
        "1" => Ok(Gpio::Gpio1),
        "2" => Ok(Gpio::Gpio2),
        "3" => Ok(Gpio::Gpio3),
        _ => Err(Error::Usage(format!(
            "Invalid GPIO number '{value}', expected 1, 2 or 3"
        ))),
    }
}

/// Logic level of a GPIO pin, `0|1`.
fn level(value: &str) -> Result<bool> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(Error::Usage(format!(
            "Invalid GPIO value '{value}', expected 0 or 1"
        ))),
    }
}

/// Power-on default of a port, `0|1|2`.
fn power_on_state(value: &str) -> Result<PowerOnState> {
    match value {
        "0" => Ok(PowerOnState::Off),
        "1" => Ok(PowerOnState::On),
        "2" => Ok(PowerOnState::Persist),
        _ => Err(Error::Usage(format!(
            "Invalid configuration value '{value}', expected 0 (off), 1 (on) or 2 (persistent)"
        ))),
    }
}

/// `enable` or `disable`.
fn enable_disable(value: &str, option: &str) -> Result<bool> {
    match value {
        "enable" => Ok(true),
        "disable" => Ok(false),
        _ => Err(Error::Usage(format!(
            "Invalid value '{value}' for {option}, expected enable or disable"
        ))),
    }
}

/// A byte written in hexadecimal, with or without the `0x` prefix.
fn hex_byte(value: &str) -> Result<u8> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);

    u8::from_str_radix(digits, 16).map_err(|_| {
        Error::Usage(format!(
            "Invalid hexadecimal byte '{value}', expected for example 0x2a"
        ))
    })
}

/// A decimal byte count.
fn count(value: &str) -> Result<u8> {
    value
        .parse()
        .map_err(|_| Error::Usage(format!("Invalid number of bytes '{value}'")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    /// Parses a command line and returns the command.
    fn cmd(line: &str) -> Command {
        parse(&args(line))
            .expect("command line should parse")
            .command
    }

    /// Message of a command line that is expected to be rejected. A usage error
    /// displays as its bare message, so the caller can match on the text.
    fn usage_error(line: &str) -> String {
        parse(&args(line))
            .expect_err("command line should be rejected")
            .to_string()
    }

    // -- grammar ----------------------------------------------------------

    #[test]
    fn options_collect_their_parameters() {
        let opts = split(&args("-s YK00001 -c 1 2")).unwrap();

        assert_eq!(
            opts,
            vec![
                Opt {
                    name: "-s".into(),
                    params: vec!["YK00001".into()]
                },
                Opt {
                    name: "-c".into(),
                    params: vec!["1".into(), "2".into()]
                },
            ]
        );
    }

    #[test]
    fn a_leading_board_name_is_ignored() {
        assert_eq!(cmd("ykush3 -u 1"), Command::PortUp(Port::Downstream1));
    }

    #[test]
    fn any_other_leading_word_is_rejected() {
        // Only the board name of the C++ invocation is accepted; anything
        // else in that position is a mistake, not something to skip.
        assert_eq!(usage_error("garbage -u 1"), "Unexpected argument 'garbage'");
        assert_eq!(usage_error("ykush2 -u 1"), "Unexpected argument 'ykush2'");
    }

    #[test]
    fn extra_parameters_after_a_command_are_tolerated() {
        // C++ parity: the original reads the parameters it needs and ignores
        // the rest of the line.
        assert_eq!(cmd("-u 1 2"), Command::PortUp(Port::Downstream1));
    }

    #[test]
    fn multi_character_short_options_stay_whole() {
        assert_eq!(cmd("-on"), Command::PortUp(Port::External));
        assert_eq!(cmd("-off"), Command::PortDown(Port::External));
    }

    #[test]
    fn no_arguments_show_the_help() {
        assert_eq!(cmd(""), Command::Help);
    }

    #[test]
    fn an_unknown_option_is_rejected() {
        assert_eq!(usage_error("-x"), "Unknown option -x");
    }

    #[test]
    fn a_serial_number_alone_is_not_a_command() {
        assert_eq!(usage_error("-s YK00001"), "No command given");
    }

    // -- board selection --------------------------------------------------

    #[test]
    fn the_serial_number_selects_the_board() {
        let inv = parse(&args("-s YK00001 -d 2")).unwrap();

        assert_eq!(inv.serial.as_deref(), Some("YK00001"));
        assert_eq!(inv.command, Command::PortDown(Port::Downstream2));
    }

    #[test]
    fn the_serial_number_may_follow_the_command() {
        // The C++ application executes the first command it sees and would miss
        // a serial number given afterwards.
        let inv = parse(&args("-d 2 -s YK00001")).unwrap();

        assert_eq!(inv.serial.as_deref(), Some("YK00001"));
        assert_eq!(inv.command, Command::PortDown(Port::Downstream2));
    }

    #[test]
    fn without_a_serial_number_no_board_is_selected() {
        assert_eq!(parse(&args("-l")).unwrap().serial, None);
    }

    #[test]
    fn a_serial_number_option_needs_a_value() {
        assert_eq!(usage_error("-s -d 1"), "Option -s expects a serial number");
    }

    #[test]
    fn a_second_serial_number_is_rejected() {
        // Silently taking the first of two would address the wrong board
        // half the time.
        assert_eq!(
            usage_error("-s A -s B -d 1"),
            "The serial number can only be given once"
        );
    }

    // -- waiting for the board --------------------------------------------

    #[test]
    fn by_default_a_busy_board_is_waited_for() {
        assert!(!parse(&args("-g 1")).unwrap().no_wait);
    }

    #[test]
    fn no_wait_may_stand_anywhere_on_the_line() {
        for line in ["--no-wait -g 1", "-g 1 --no-wait", "-s A --no-wait -g 1"] {
            let inv = parse(&args(line)).unwrap();

            assert!(inv.no_wait, "{line}");
            assert_eq!(
                inv.command,
                Command::PortStatus(Port::Downstream1),
                "{line}"
            );
        }
    }

    #[test]
    fn no_wait_given_twice_is_still_no_wait() {
        assert!(parse(&args("--no-wait --no-wait -g 1")).unwrap().no_wait);
    }

    #[test]
    fn no_wait_takes_no_value() {
        assert_eq!(
            usage_error("--no-wait 5 -g 1"),
            "Option --no-wait takes no value, got '5'"
        );
    }

    #[test]
    fn no_wait_alone_is_not_a_command() {
        assert_eq!(usage_error("--no-wait"), "No command given");
        assert_eq!(usage_error("-s A --no-wait"), "No command given");
    }

    #[test]
    fn an_unknown_option_is_named_even_after_a_modifier() {
        assert_eq!(usage_error("--no-wait -x"), "Unknown option -x");
        assert_eq!(usage_error("-s A -x"), "Unknown option -x");
    }

    // -- port commands ----------------------------------------------------

    #[test]
    fn port_numbers_map_to_ports() {
        assert_eq!(cmd("-u 1"), Command::PortUp(Port::Downstream1));
        assert_eq!(cmd("-u 3"), Command::PortUp(Port::Downstream3));
        assert_eq!(cmd("-u a"), Command::PortUp(Port::All));
        assert_eq!(cmd("-d 2"), Command::PortDown(Port::Downstream2));
        assert_eq!(cmd("-g 1"), Command::PortStatus(Port::Downstream1));
        assert_eq!(cmd("-g a"), Command::PortStatus(Port::All));
    }

    #[test]
    fn the_external_port_has_two_spellings() {
        assert_eq!(cmd("-u 4"), Command::PortUp(Port::External));
        assert_eq!(cmd("-u e"), Command::PortUp(Port::External));
        assert_eq!(
            cmd("-c e 1"),
            Command::Config(Port::External, PowerOnState::On)
        );
        assert_eq!(
            cmd("-c 4 1"),
            Command::Config(Port::External, PowerOnState::On)
        );
    }

    #[test]
    fn an_invalid_port_number_is_rejected() {
        assert!(usage_error("-u 9").starts_with("Invalid port number '9'"));
        assert!(usage_error("-u x").starts_with("Invalid port number 'x'"));
    }

    #[test]
    fn a_port_command_needs_a_port_number() {
        assert_eq!(usage_error("-u"), "Option -u expects a port number");
    }

    // -- configuration ----------------------------------------------------

    #[test]
    fn the_power_on_state_is_parsed() {
        assert_eq!(
            cmd("-c 1 0"),
            Command::Config(Port::Downstream1, PowerOnState::Off)
        );
        assert_eq!(
            cmd("-c 2 1"),
            Command::Config(Port::Downstream2, PowerOnState::On)
        );
        assert_eq!(
            cmd("-c 3 2"),
            Command::Config(Port::Downstream3, PowerOnState::Persist)
        );
    }

    #[test]
    fn an_invalid_power_on_state_is_rejected() {
        assert!(usage_error("-c 1 5").starts_with("Invalid configuration value '5'"));
        assert_eq!(
            usage_error("-c 1"),
            "Option -c expects a configuration value"
        );
    }

    // -- gpio -------------------------------------------------------------

    #[test]
    fn gpio_commands_are_parsed() {
        assert_eq!(cmd("-r 2"), Command::ReadIo(Gpio::Gpio2));
        assert_eq!(cmd("-w 1 1"), Command::WriteIo(Gpio::Gpio1, true));
        assert_eq!(cmd("-w 3 0"), Command::WriteIo(Gpio::Gpio3, false));
        assert_eq!(cmd("--gpio enable"), Command::GpioControl(true));
        assert_eq!(cmd("--gpio disable"), Command::GpioControl(false));
    }

    #[test]
    fn invalid_gpio_arguments_are_rejected() {
        assert!(usage_error("-r 4").starts_with("Invalid GPIO number '4'"));
        assert!(usage_error("-w 1 2").starts_with("Invalid GPIO value '2'"));
        assert!(usage_error("--gpio on").starts_with("Invalid value 'on' for --gpio"));
    }

    // -- i2c --------------------------------------------------------------

    #[test]
    fn i2c_mode_commands_are_parsed() {
        assert_eq!(cmd("--i2c-slave enable"), Command::I2cSlave(true));
        assert_eq!(cmd("--i2c-master disable"), Command::I2cMaster(false));
        assert_eq!(cmd("--i2c-set-address 0x2a"), Command::I2cSetAddress(0x2a));
    }

    #[test]
    fn an_invalid_i2c_mode_value_is_rejected() {
        assert!(usage_error("--i2c-slave on").starts_with("Invalid value 'on' for --i2c-slave"));
        assert!(usage_error("--i2c-master 1").starts_with("Invalid value '1' for --i2c-master"));
        assert_eq!(
            usage_error("--i2c-slave"),
            "Option --i2c-slave expects enable or disable"
        );
    }

    #[test]
    fn i2c_write_takes_a_variable_number_of_bytes() {
        assert_eq!(
            cmd("--i2c-write 0x20 0x01 0xff"),
            Command::I2cWrite(0x20, vec![0x01, 0xff])
        );
    }

    #[test]
    fn i2c_write_needs_at_least_one_data_byte() {
        assert_eq!(
            usage_error("--i2c-write 0x20"),
            "Option --i2c-write expects at least one data byte"
        );
    }

    #[test]
    fn i2c_read_takes_a_decimal_length() {
        assert_eq!(cmd("--i2c-read 0x20 12"), Command::I2cRead(0x20, 12));
    }

    #[test]
    fn hex_bytes_parse_with_and_without_prefix() {
        assert_eq!(cmd("--i2c-set-address 2a"), Command::I2cSetAddress(0x2a));
        assert_eq!(cmd("--i2c-set-address 0X2A"), Command::I2cSetAddress(0x2a));
        assert!(usage_error("--i2c-set-address zz").starts_with("Invalid hexadecimal byte"));
        assert!(usage_error("--i2c-set-address 0x1ff").starts_with("Invalid hexadecimal byte"));
    }

    #[test]
    fn an_invalid_byte_count_is_rejected() {
        assert!(usage_error("--i2c-read 0x20 x").starts_with("Invalid number of bytes"));
    }

    // -- remaining commands -----------------------------------------------

    #[test]
    fn the_plain_commands_are_parsed() {
        assert_eq!(cmd("-l"), Command::List);
        assert_eq!(cmd("--reset"), Command::Reset);
        assert_eq!(cmd("--boot"), Command::Bootloader);
        assert_eq!(cmd("--firmware-version"), Command::FirmwareVersion);
        assert_eq!(cmd("--bootloader-version"), Command::BootloaderVersion);
        assert_eq!(cmd("-h"), Command::Help);
        assert_eq!(cmd("--help"), Command::Help);
        assert_eq!(cmd("-v"), Command::Version);
        assert_eq!(cmd("--version"), Command::Version);
    }

    #[test]
    fn the_first_command_on_the_line_wins() {
        // Same rule as in the C++ application, which returns after the first
        // command it recognises.
        assert_eq!(cmd("-u 1 -d 2"), Command::PortUp(Port::Downstream1));
    }
}
