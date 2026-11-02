// SPDX-License-Identifier: Apache-2.0
//! End-to-end tests that run the built binary.
//!
//! These cover the process entry point: exit codes, and which of stdout and
//! stderr a message lands on. Only commands that do not switch anything are
//! used, so the tests are safe to run with a board attached.

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ykushctrl"))
        .args(args)
        .output()
        .expect("the binary should run")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn without_arguments_the_help_goes_to_stdout_and_succeeds() {
    let output = run(&[]);

    assert!(output.status.success());
    assert!(stdout(&output).contains("ykushctrl "));
    assert!(stdout(&output).contains("--i2c-read"));
    assert!(stderr(&output).is_empty());
}

#[test]
fn the_help_option_prints_the_same_help() {
    for flag in ["-h", "--help"] {
        let output = run(&[flag]);

        assert!(output.status.success(), "{flag}");
        assert!(
            stdout(&output).contains("Switch the ports of a YKUSH3 hub"),
            "{flag}"
        );
    }
}

#[test]
fn the_version_option_prints_only_the_version() {
    let output = run(&["-v"]);

    assert!(output.status.success());
    assert!(stdout(&output).starts_with("ykushctrl "));
    assert_eq!(stdout(&output).lines().count(), 1);
}

#[test]
fn an_unknown_option_fails_with_a_usage_message_on_stderr() {
    let output = run(&["-x"]);

    assert_eq!(output.status.code(), Some(64));
    assert!(stderr(&output).contains("Unknown option -x"));
    assert!(stderr(&output).contains("Usage:"));
    assert!(stdout(&output).is_empty());
}

#[test]
fn an_invalid_value_fails_with_a_usage_message() {
    let output = run(&["-u", "9"]);

    assert_eq!(output.status.code(), Some(64));
    assert!(stderr(&output).contains("Invalid port number '9'"));
}

#[test]
fn an_unknown_serial_number_fails_without_a_usage_message() {
    let output = run(&["-s", "no-such-board", "-g", "1"]);

    assert_eq!(output.status.code(), Some(69));
    assert!(stderr(&output).contains("No YKUSH3 board with serial number no-such-board found"));
    // A board that is not there is not a usage error, so no usage text.
    assert!(!stderr(&output).contains("Usage:"));
}

#[test]
fn an_argument_that_is_not_unicode_is_a_usage_error_not_a_crash() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    // A byte sequence no String can hold. `env::args()` would panic on it;
    // the lossy conversion turns it into an ordinary unknown option.
    let output = Command::new(env!("CARGO_BIN_EXE_ykushctrl"))
        .arg(OsStr::from_bytes(b"-\xff"))
        .output()
        .expect("the binary should run");

    assert_eq!(output.status.code(), Some(64));
    assert!(stderr(&output).contains("Unknown option"));
    assert!(!stderr(&output).contains("panicked"));
}

#[test]
fn control_characters_in_an_argument_are_not_echoed_to_the_terminal() {
    // An unknown option is echoed in the error message. A caller must not be
    // able to smuggle an ANSI escape sequence onto stderr through it.
    let output = run(&["-\x1b[31mboo"]);

    assert_eq!(output.status.code(), Some(64));
    assert!(stderr(&output).contains("Unknown option -?[31mboo"));
}

#[test]
fn the_board_name_prefix_of_the_cpp_application_is_accepted() {
    let output = run(&["ykush3", "-h"]);

    assert!(output.status.success());
    assert!(stdout(&output).contains("Switch the ports of a YKUSH3 hub"));
}

#[test]
fn no_wait_changes_nothing_about_a_board_that_is_not_there() {
    let output = run(&["--no-wait", "-s", "no-such-board", "-g", "1"]);

    assert_eq!(output.status.code(), Some(69));
    assert!(stderr(&output).contains("No YKUSH3 board with serial number no-such-board found"));
}

#[test]
fn no_wait_with_a_value_is_a_usage_error() {
    let output = run(&["--no-wait", "5", "-g", "1"]);

    assert_eq!(output.status.code(), Some(64));
    assert!(stderr(&output).contains("Option --no-wait takes no value"));
    assert!(stderr(&output).contains("Usage:"));
}

/// Another process holding the board's lock, the way a second invocation
/// would. Linux only, and only with a board attached: the binary has to find
/// the board before it gets to the lock. Read-only, like everything here.
///
/// The holder is util-linux `flock`, a real second process on the same
/// hidraw node, which the kernel treats exactly like another `ykushctrl`.
#[cfg(target_os = "linux")]
mod held_by_another_process {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    /// The hidraw node of the first YKUSH3, found the way sysfs lists it.
    fn ykush3_node() -> String {
        let mut nodes: Vec<String> = std::fs::read_dir("/sys/class/hidraw")
            .expect("hidraw class in sysfs")
            .filter_map(|e| e.ok())
            .filter(|e| {
                std::fs::read_to_string(e.path().join("device/uevent"))
                    .is_ok_and(|u| u.contains("HID_ID=0003:000004D8:0000F11B"))
            })
            .map(|e| format!("/dev/{}", e.file_name().to_string_lossy()))
            .collect();
        nodes.sort();
        nodes.into_iter().next().expect("no YKUSH3 attached")
    }

    /// Holds the lock on the board's node for `seconds`, and returns once it
    /// is held.
    ///
    /// The shell opens the node itself, locks it through util-linux `flock`
    /// on that descriptor, and then becomes `sleep`. So the spawned process
    /// is the one holding the lock, and killing it releases the lock at once;
    /// with `flock NODE sleep` a killed `flock` would leave `sleep` holding
    /// it until the time was up.
    fn hold(seconds: u32) -> Child {
        let script = format!(
            "exec 9<'{}' && flock 9 && echo held && exec sleep {seconds}",
            ykush3_node()
        );
        let mut child = Command::new("sh")
            .args(["-c", &script])
            .stdout(Stdio::piped())
            .spawn()
            .expect("sh should run");
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "held");
        child
    }

    fn timed(args: &[&str]) -> (Output, Duration) {
        let started = Instant::now();
        let output = run(args);
        (output, started.elapsed())
    }

    #[test]
    #[ignore = "needs a YKUSH3 attached"]
    fn with_no_wait_a_held_board_fails_at_once() {
        let mut holder = hold(10);

        let (output, took) = timed(&["--no-wait", "-g", "1"]);
        let _ = holder.kill();
        let _ = holder.wait();

        assert_eq!(output.status.code(), Some(75));
        assert!(
            stderr(&output).contains("is not available: another ykushctrl is using it"),
            "{}",
            stderr(&output)
        );
        assert!(stdout(&output).is_empty());
        // Generous: the point is that it did not wait the five seconds.
        assert!(took < Duration::from_secs(3), "took {took:?}");
    }

    #[test]
    #[ignore = "needs a YKUSH3 attached"]
    fn a_board_held_too_long_fails_after_the_wait() {
        let mut holder = hold(10);

        let (output, took) = timed(&["-g", "1"]);
        let _ = holder.kill();
        let _ = holder.wait();

        assert_eq!(output.status.code(), Some(75));
        assert!(
            stderr(&output).contains("still not available after 5.0 s"),
            "{}",
            stderr(&output)
        );
        assert!(took >= Duration::from_secs(5), "took {took:?}");
    }

    #[test]
    #[ignore = "needs a YKUSH3 attached"]
    fn a_board_that_comes_free_is_waited_for() {
        let mut holder = hold(1);

        let (output, _) = timed(&["-g", "1"]);
        let _ = holder.wait();

        assert!(output.status.success(), "{}", stderr(&output));
        assert!(stdout(&output).starts_with("Port 1: "));
    }
}
