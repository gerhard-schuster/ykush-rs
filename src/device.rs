// SPDX-License-Identifier: Apache-2.0
//! USB HID transport for the YKUSH3 board.
//!
//! Talks to the board through async-hid, which reaches the device by way of
//! IOKit on macOS and through the kernel's hidraw interface on Linux. That
//! library is asynchronous because that is how macOS delivers HID reports - it
//! hands out callbacks on a run loop rather than a blocking read. This program
//! is not asynchronous and has no reason to be, so the awaits are resolved here
//! and everything above the [`Transport`] trait stays as synchronous as it was.
//!
//! The report exchange is the same on both systems. What differs is the way to
//! the device: Linux guards `/dev/hidraw*` with file permissions, which a udev
//! rule has to open up, and a hidraw node exists only while the kernel's HID
//! driver is bound to the board. Both failures are reported in words here
//! rather than as a bare errno or as a board that is not there.
//!
//! Opening also waits its turn when another invocation is using the board,
//! see [`crate::lock`].

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::time::Duration;

use async_hid::{AsyncHidRead, AsyncHidWrite, Device, DeviceReaderWriter, HidBackend};
use futures_lite::{future, StreamExt};

use crate::error::{Error, Result};
use crate::lock::DEFAULT_WAIT;
use crate::sanitize::sanitize;

/// Microchip vendor id used by all Yepkit boards.
pub const VENDOR_ID: u16 = 0x04D8;
/// Product id of the YKUSH3 board.
pub const PRODUCT_ID: u16 = 0xF11B;

/// Every YKUSH3 report is a fixed size block.
pub const REPORT_SIZE: usize = 64;

const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// How long to wait for a late answer when emptying the queue after a read
/// timed out. Long enough to catch one already on its way, short enough not to
/// be felt on the next command.
const DISCARD_WAIT: Duration = Duration::from_millis(50);

/// Upper bound on the reports dropped in one go, so a board that streams cannot
/// hold the program in the drain.
const DISCARD_LIMIT: usize = 16;

pub type Report = [u8; REPORT_SIZE];

/// Report exchange with a board.
///
/// The protocol layer is written against this trait so it can be driven by a
/// test double instead of real hardware.
pub trait Transport {
    /// Sends a report and returns the report the board answers with.
    fn transfer(&self, out: &Report) -> Result<Report>;

    /// Sends a report without waiting for an answer. Used for the commands that
    /// make the board reboot, which therefore never answer.
    fn send(&self, out: &Report) -> Result<()>;
}

/// Pads a command payload to a full report.
///
/// Crate-internal on purpose: the debug assertion below is the only length
/// guard, which is fine for the fixed payloads of this crate but would be an
/// implicit panic in a public interface.
pub(crate) fn report(payload: &[u8]) -> Report {
    debug_assert!(payload.len() <= REPORT_SIZE);

    let mut buf: Report = [0; REPORT_SIZE];
    buf[..payload.len()].copy_from_slice(payload);
    buf
}

/// An opened YKUSH3 board.
pub struct Board {
    /// Every exchange needs the handle mutably while [`Transport`] hands out a
    /// shared reference. A cell bridges the two, which costs nothing here: a
    /// board is driven from one thread at a time. Neither macOS nor Linux
    /// refuses a second open, see the README on concurrent use.
    dev: RefCell<DeviceReaderWriter>,

    /// Set when a read timed out. The answer may still arrive afterwards and
    /// would then be queued, so the next exchange empties the queue first.
    stale: Cell<bool>,

    /// This process's turn at the board, see [`crate::lock`]. Declared last
    /// so that it is dropped last: the next invocation must not get the turn
    /// while this one still has the device open.
    #[cfg(target_os = "linux")]
    _turn: crate::lock::NodeLock,
}

impl Board {
    /// Opens the board with the given serial number, or the first board in the
    /// enumeration order when no serial number is provided.
    ///
    /// When another invocation is using the board, waits up to
    /// [`DEFAULT_WAIT`](crate::DEFAULT_WAIT) for it to finish.
    pub fn open(serial: Option<&str>) -> Result<Self> {
        Board::open_within(serial, DEFAULT_WAIT)
    }

    /// Like [`Board::open`], waiting at most `wait` for a board another
    /// invocation is using. With `wait` zero, a board in use fails at once
    /// with [`Error::Busy`].
    pub fn open_within(serial: Option<&str>, wait: Duration) -> Result<Self> {
        // Linux: the turn first, then the handle, so this process never has
        // the board open while another one is in the middle of an exchange.
        #[cfg(target_os = "linux")]
        let (turn, dev) = {
            let (_, device, turn) = crate::lock::take_stable(
                wait,
                || future::block_on(choose(serial)),
                hidraw_node_of,
                |node, found, left| crate::lock::NodeLock::take(node, found, left),
            )?;
            (turn, future::block_on(device.open()).map_err(open_failed)?)
        };

        // macOS: no lock; a refused open is repeated. With this transport the
        // system does not refuse a second open, so this protects nothing yet,
        // see crate::lock.
        #[cfg(target_os = "macos")]
        let dev = {
            use crate::lock::Attempt;

            let (found, device) = future::block_on(choose(serial))?;
            crate::lock::wait_for(&found, wait, || match future::block_on(device.open()) {
                Ok(dev) => Attempt::Got(dev),
                Err(error) if refused_as_busy(&error) => Attempt::Busy(format!(
                    "the system refused to open it, possibly because another program \
                     has it open ({error})"
                )),
                Err(error) => Attempt::Failed(open_failed(error)),
            })?
        };

        Ok(Board {
            dev: RefCell::new(dev),
            stale: Cell::new(false),
            #[cfg(target_os = "linux")]
            _turn: turn,
        })
    }

    /// Drops reports the board sent after a read had already given up on them.
    ///
    /// Bounded twice: by a short wait, so a board with nothing left to say
    /// costs almost nothing, and by a count, so one that streams cannot keep
    /// the program here. Errors are swallowed - the exchange that follows
    /// raises them properly.
    fn discard_queued(&self) {
        let mut dev = self.dev.borrow_mut();
        let mut scratch: Report = [0; REPORT_SIZE];

        for _ in 0..DISCARD_LIMIT {
            let got = future::block_on(future::or(
                async { Some(dev.read_input_report(&mut scratch).await) },
                async {
                    async_io::Timer::after(DISCARD_WAIT).await;
                    None
                },
            ));

            if !matches!(got, Some(Ok(_))) {
                break;
            }
        }
    }
}

impl Transport for Board {
    fn transfer(&self, out: &Report) -> Result<Report> {
        // An answer to a read that timed out may arrive afterwards and wait in
        // the queue of the transport. Taken for the answer to this command it
        // would pass unnoticed: the switching and configuration commands carry
        // nothing beyond the status byte to check it against. Emptying the
        // queue first is what keeps one timeout from shifting every later
        // answer by one.
        if self.stale.replace(false) {
            self.discard_queued();
        }

        self.send(out)?;

        let mut resp: Report = [0; REPORT_SIZE];
        let mut dev = self.dev.borrow_mut();

        // The library has no timed read, so the read races a timer: a board
        // that says nothing must not hang the program. Dropping the losing
        // read leaves the handle usable - the next command goes through as if
        // nothing had happened.
        let read = future::block_on(future::or(
            async { Some(dev.read_input_report(&mut resp).await) },
            async {
                async_io::Timer::after(READ_TIMEOUT).await;
                None
            },
        ));
        drop(dev);

        match read {
            None => {
                self.stale.set(true);
                Err(Error::NoResponse)
            }
            Some(Err(e)) => Err(e.into()),
            // The board always answers with a full report. Anything shorter
            // would leave the zero padding of the buffer to be read as answer
            // bytes, so it is rejected rather than interpreted.
            Some(Ok(read)) if read != REPORT_SIZE => Err(Error::Device(format!(
                "Truncated answer from the board: {read} of {REPORT_SIZE} bytes"
            ))),
            Some(Ok(_)) => Ok(resp),
        }
    }

    fn send(&self, out: &Report) -> Result<()> {
        // The first byte is the report id, which the library strips when it is
        // zero. The board uses unnumbered reports, so a leading zero is what it
        // wants: passing the bare report instead would send it under whatever
        // its first byte happens to be.
        //
        // There is no timeout here. The library offers none for writing, so a
        // device that stalls its endpoint blocks for as long as the operating
        // system lets it.
        let mut buf = [0u8; REPORT_SIZE + 1];
        buf[1..].copy_from_slice(out);

        let mut dev = self.dev.borrow_mut();
        future::block_on(dev.write_output_report(&buf))?;
        Ok(())
    }
}

/// Serial numbers of all attached YKUSH3 boards, in enumeration order.
pub fn list() -> Result<Vec<String>> {
    future::block_on(async {
        let attached = attached_boards().await?;
        Ok(attached.into_iter().map(|(serial, _)| serial).collect())
    })
}

/// What to tell a user whose operating system refused to open the board.
#[cfg(target_os = "linux")]
pub(crate) const PERMISSION_DENIED: &str = "Permission denied on the hidraw device of the \
YKUSH3. Allow access with a udev rule, /etc/udev/rules.d/60-ykush3.rules as \
shown in the README, make this user a member of the group the rule names, and \
replug the board";
#[cfg(not(target_os = "linux"))]
pub(crate) const PERMISSION_DENIED: &str = "Permission denied on the YKUSH3";

/// Turns a refused open into an error that says what to do about it.
///
/// On Linux the board is a file, `/dev/hidrawN`, owned by root until a udev
/// rule says otherwise. The library reports that as a plain `EACCES`, which
/// on its own does not lead anyone to the rule.
fn open_failed(error: async_hid::HidError) -> Error {
    let denied = match &error {
        async_hid::HidError::Other(inner) => inner
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::PermissionDenied),
        _ => false,
    };

    if denied {
        Error::Hid(PERMISSION_DENIED.to_owned())
    } else {
        error.into()
    }
}

/// Explains a board that is on the bus but has no HID device to open.
///
/// Only Linux can get into this state: a libusb based program - `ykushcmd`
/// is one - detaches the kernel's HID driver to talk to the board itself, and
/// the hidraw node goes with it until the driver is bound again. The board is
/// then listed by `lsusb` and still invisible here, which a plain "not found"
/// would leave unexplained.
fn detached(serial: Option<&str>) -> Option<Error> {
    detached_message(&detached_on_this_host(serial)).map(Error::Device)
}

/// Why boards that `lsusb` shows are missing from the list, if any are.
///
/// Listing only reads the device list and fails for none of this, so the
/// command line prints this beside an empty list rather than instead of it.
/// Always `None` outside Linux, where the state cannot arise.
pub fn detached_hint() -> Option<String> {
    detached_message(&detached_on_this_host(None))
}

#[cfg(target_os = "linux")]
fn detached_on_this_host(serial: Option<&str>) -> Vec<String> {
    detached_interfaces(std::path::Path::new(SYSFS_USB_DEVICES), serial)
}

#[cfg(not(target_os = "linux"))]
fn detached_on_this_host(_serial: Option<&str>) -> Vec<String> {
    Vec::new()
}

/// The explanation for HID interfaces without a driver, naming every one of
/// them: with several boards detached, naming one would leave the others
/// unexplained.
fn detached_message(interfaces: &[String]) -> Option<String> {
    let bind =
        |name: &String| format!("echo -n {name} | sudo tee /sys/bus/usb/drivers/usbhid/bind");

    match interfaces {
        [] => None,
        [one] => Some(format!(
            "A YKUSH3 is on the USB bus, but no HID driver is bound to its interface \
             {one}, so it has no hidraw device. A program that drives the board \
             through libusb, such as ykushcmd, detaches the driver. Replug the board, \
             or bind the driver again: {}",
            bind(one)
        )),
        several => Some(format!(
            "{} YKUSH3 boards are on the USB bus, but no HID driver is bound to their \
             interfaces {}, so they have no hidraw device. A program that drives the \
             board through libusb, such as ykushcmd, detaches the driver. Replug the \
             boards, or bind the driver again: {}",
            several.len(),
            several.join(", "),
            several.iter().map(bind).collect::<Vec<_>>().join("; ")
        )),
    }
}

/// Where Linux lists USB devices and their interfaces side by side.
#[cfg(target_os = "linux")]
const SYSFS_USB_DEVICES: &str = "/sys/bus/usb/devices";

/// The HID interfaces of YKUSH3 boards that no driver is bound to, by their
/// sysfs name (`1-1.4.4:1.0`), optionally only those of one serial number.
///
/// Takes the directory as a parameter so the tests can lay out a fake one.
/// Anything unreadable counts as not matching: this only refines an error
/// that is already being reported.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn detached_interfaces(usb_devices: &std::path::Path, serial: Option<&str>) -> Vec<String> {
    let read = |path: std::path::PathBuf| {
        std::fs::read_to_string(path)
            .map(|s| s.trim().to_owned())
            .unwrap_or_default()
    };

    let Ok(entries) = std::fs::read_dir(usb_devices) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();

    let boards: Vec<&String> = names
        .iter()
        .filter(|name| {
            let dir = usb_devices.join(name);
            u16::from_str_radix(&read(dir.join("idVendor")), 16) == Ok(VENDOR_ID)
                && u16::from_str_radix(&read(dir.join("idProduct")), 16) == Ok(PRODUCT_ID)
                && serial.is_none_or(|wanted| sanitize(&read(dir.join("serial"))) == wanted)
        })
        .collect();

    names
        .iter()
        .filter(|name| {
            boards.iter().any(|board| {
                name.strip_prefix(board.as_str())
                    .is_some_and(|rest| rest.starts_with(':'))
            })
        })
        .filter(|name| {
            let dir = usb_devices.join(name);
            read(dir.join("bInterfaceClass")) == "03"
                && std::fs::symlink_metadata(dir.join("driver")).is_err()
        })
        .cloned()
        .collect()
}

/// The board a command is meant for: the one with `serial`, or the first one
/// in enumeration order. Returns its sanitised serial number with it.
async fn choose(serial: Option<&str>) -> Result<(String, Device)> {
    // Probing the device list first turns "nothing attached" into a clear
    // message instead of a generic open failure.
    let attached = attached_boards().await?;
    let chosen = match serial {
        Some(wanted) => attached.into_iter().find(|(known, _)| known == wanted),
        None => attached.into_iter().next(),
    };

    chosen.ok_or_else(|| {
        detached(serial).unwrap_or(Error::NotFound {
            serial: serial.map(str::to_owned),
        })
    })
}

/// Whether macOS refused an open because the device is held elsewhere, which
/// waiting may cure.
///
/// IOKit says so with `kIOReturnExclusiveAccess`, but the transport discards
/// the return code and reports every refused open with the same message. So
/// any refused open counts: a refusal that has another cause costs the wait
/// and is then reported with the transport's own words.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn refused_as_busy(error: &async_hid::HidError) -> bool {
    match error {
        async_hid::HidError::Message(msg) => {
            msg == "Failed to open device" || msg.contains("exclusive access")
        }
        _ => false,
    }
}

/// The hidraw node of a board found by the transport.
///
/// On Linux the transport identifies a device by its directory in sysfs,
/// `.../hidraw/hidrawN`, and opens the node named in the `uevent` file there.
/// The lock goes on the same node.
#[cfg(target_os = "linux")]
fn hidraw_node_of(device: &Device) -> Result<std::path::PathBuf> {
    let async_hid::DeviceId::DevPath(sysfs) = &device.id else {
        return Err(Error::Device("The YKUSH3 has no hidraw device".into()));
    };

    hidraw_node(sysfs).ok_or_else(|| {
        Error::Device(format!(
            "No hidraw device node found for the YKUSH3 at {}",
            sysfs.display()
        ))
    })
}

/// Reads `DEVNAME` from the `uevent` file in a hidraw directory of sysfs and
/// turns it into the path under `/dev`. Only a plain `hidrawN` is taken, so a
/// file that says something else cannot point the lock anywhere.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn hidraw_node(sysfs: &std::path::Path) -> Option<std::path::PathBuf> {
    let uevent = std::fs::read_to_string(sysfs.join("uevent")).ok()?;
    let name = uevent
        .lines()
        .find_map(|line| line.strip_prefix("DEVNAME="))?
        .trim();
    let number = name.strip_prefix("hidraw")?;

    (!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
        .then(|| std::path::Path::new("/dev").join(name))
}

/// Every attached board, as its sanitised serial number and the handle it was
/// found under, in enumeration order.
async fn attached_boards() -> Result<Vec<(String, Device)>> {
    let mut found: Vec<(String, Device)> = Vec::new();

    // One backend for the process. It is a handle rather than a resource, but
    // making it once keeps the enumeration from building it again per call.
    static BACKEND: OnceLock<HidBackend> = OnceLock::new();
    let backend = BACKEND.get_or_init(HidBackend::default);

    let mut devices = backend.enumerate().await?;
    while let Some(dev) = devices.next().await {
        if dev.vendor_id != VENDOR_ID || dev.product_id != PRODUCT_ID {
            continue;
        }
        let serial = sanitize(dev.serial_number.as_deref().unwrap_or("<unknown>"));
        // A board can expose several HID interfaces and would then show up
        // more than once in the device list.
        if !found.iter().any(|(known, _)| *known == serial) {
            found.push((serial, dev));
        }
    }

    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_is_padded_to_a_full_report() {
        let r = report(&[0x52, 0x01, 0x20, 0x02, 0xaa, 0xbb]);

        assert_eq!(r.len(), REPORT_SIZE);
        assert_eq!(&r[..6], &[0x52, 0x01, 0x20, 0x02, 0xaa, 0xbb]);
        assert!(r[6..].iter().all(|&b| b == 0));
    }

    fn os_error(kind: std::io::ErrorKind) -> async_hid::HidError {
        async_hid::HidError::from(std::io::Error::from(kind))
    }

    #[test]
    fn a_refused_open_says_what_to_do_about_it() {
        let error = open_failed(os_error(std::io::ErrorKind::PermissionDenied));

        assert!(matches!(&error, Error::Hid(msg) if msg == PERMISSION_DENIED));
        #[cfg(target_os = "linux")]
        assert!(error
            .to_string()
            .contains("/etc/udev/rules.d/60-ykush3.rules"));
    }

    #[test]
    fn any_other_open_failure_keeps_its_own_message() {
        let error = open_failed(os_error(std::io::ErrorKind::NotFound));

        assert!(matches!(&error, Error::Hid(msg) if msg != PERMISSION_DENIED));
        let error = open_failed(async_hid::HidError::NotConnected);
        assert!(matches!(&error, Error::Hid(msg) if msg != PERMISSION_DENIED));
    }

    /// A stand-in for `/sys/bus/usb/devices`, removed again when dropped.
    struct FakeSysfs(std::path::PathBuf);

    impl FakeSysfs {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("ykush3-sysfs-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            FakeSysfs(dir)
        }

        fn file(&self, path: &str, content: &str) -> &Self {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("{content}\n")).unwrap();
            self
        }

        fn device(&self, name: &str, vid: &str, pid: &str, serial: &str) -> &Self {
            self.file(&format!("{name}/idVendor"), vid)
                .file(&format!("{name}/idProduct"), pid)
                .file(&format!("{name}/serial"), serial)
        }

        fn interface(&self, name: &str, class: &str, bound: bool) -> &Self {
            self.file(&format!("{name}/bInterfaceClass"), class);
            if bound {
                std::fs::create_dir_all(self.0.join(name).join("driver")).unwrap();
            }
            self
        }
    }

    impl Drop for FakeSysfs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_board_without_a_hid_driver_is_found_on_the_bus() {
        let sysfs = FakeSysfs::new("detached");
        sysfs
            .device("1-1.4.4", "04d8", "f11b", "Y3N13808")
            .interface("1-1.4.4:1.0", "03", false)
            // Same vendor, other product, likewise without a driver.
            .device("1-1.2", "04d8", "f11c", "OTHER")
            .interface("1-1.2:1.0", "03", false)
            // An ST-Link, whose name shares a prefix with nothing here.
            .device("1-1.4.4.1", "0483", "374e", "005000213438510D34313939")
            .interface("1-1.4.4.1:1.0", "03", false);

        assert_eq!(detached_interfaces(&sysfs.0, None), ["1-1.4.4:1.0"]);
        assert_eq!(
            detached_interfaces(&sysfs.0, Some("Y3N13808")),
            ["1-1.4.4:1.0"]
        );
        assert!(detached_interfaces(&sysfs.0, Some("Y3N00000")).is_empty());
    }

    #[test]
    fn every_detached_board_is_named() {
        let sysfs = FakeSysfs::new("two");
        sysfs
            .device("1-1.3", "04d8", "f11b", "Y3N00001")
            .interface("1-1.3:1.0", "03", false)
            .device("1-1.4.4", "04d8", "f11b", "Y3N13808")
            .interface("1-1.4.4:1.0", "03", false);

        let interfaces = detached_interfaces(&sysfs.0, None);
        assert_eq!(interfaces, ["1-1.3:1.0", "1-1.4.4:1.0"]);
        assert_eq!(
            detached_interfaces(&sysfs.0, Some("Y3N00001")),
            ["1-1.3:1.0"]
        );

        let message = detached_message(&interfaces).expect("a message");
        assert!(message.starts_with("2 YKUSH3 boards"), "{message}");
        for name in &interfaces {
            assert!(
                message.contains(&format!("echo -n {name} | sudo tee")),
                "{message} lacks {name}"
            );
        }
    }

    #[test]
    fn one_detached_board_is_named_and_none_says_nothing() {
        let message = detached_message(&["1-1.4.4:1.0".to_owned()]).expect("a message");

        assert!(
            message.starts_with("A YKUSH3 is on the USB bus"),
            "{message}"
        );
        assert!(message.contains("echo -n 1-1.4.4:1.0 | sudo tee"));
        assert_eq!(detached_message(&[]), None);
    }

    #[test]
    fn a_board_with_its_hid_driver_bound_is_not_reported() {
        let sysfs = FakeSysfs::new("bound");
        sysfs
            .device("1-1.4.4", "04d8", "f11b", "Y3N13808")
            .interface("1-1.4.4:1.0", "03", true)
            // A non-HID interface without a driver is none of this check's business.
            .interface("1-1.4.4:1.1", "ff", false);

        assert!(detached_interfaces(&sysfs.0, None).is_empty());
    }

    #[test]
    fn the_hidraw_node_is_read_from_sysfs() {
        let sysfs = FakeSysfs::new("node");
        sysfs.file(
            "hidraw/hidraw3/uevent",
            "MAJOR=241\nMINOR=3\nDEVNAME=hidraw3",
        );

        assert_eq!(
            hidraw_node(&sysfs.0.join("hidraw/hidraw3")),
            Some(std::path::PathBuf::from("/dev/hidraw3"))
        );
    }

    #[test]
    fn a_node_name_that_is_not_hidraw_is_refused() {
        let sysfs = FakeSysfs::new("badnode");
        sysfs
            .file("a/uevent", "DEVNAME=../sda")
            .file("b/uevent", "DEVNAME=hidraw")
            .file("c/uevent", "DEVNAME=hidraw1/../../sda")
            .file("d/uevent", "MAJOR=241");

        for dir in ["a", "b", "c", "d", "missing"] {
            assert_eq!(hidraw_node(&sysfs.0.join(dir)), None, "{dir}");
        }
    }

    #[test]
    fn only_a_refused_open_counts_as_a_board_in_use() {
        use async_hid::HidError;

        assert!(refused_as_busy(&HidError::message("Failed to open device")));
        assert!(refused_as_busy(&HidError::message(
            "exclusive access and device already open"
        )));
        assert!(!refused_as_busy(&HidError::NotConnected));
        assert!(!refused_as_busy(&HidError::message(
            "Failed to read input report size"
        )));
        assert!(!refused_as_busy(&os_error(
            std::io::ErrorKind::PermissionDenied
        )));
    }

    #[test]
    fn an_unreadable_device_directory_reports_nothing() {
        let missing = std::env::temp_dir().join("ykush3-sysfs-does-not-exist");

        assert!(detached_interfaces(&missing, None).is_empty());
    }

    /// Everything below needs a YKUSH3 attached and is skipped by default.
    ///
    /// Run with `cargo test -- --ignored --test-threads=1`. The single thread is
    /// not optional. Neither macOS nor Linux refuses a second open, and each
    /// answer reaches every open handle, so tests running side by side would
    /// read each other's answers.
    mod hardware {
        use super::*;

        #[test]
        #[ignore = "needs the HID stack of the operating system"]
        fn enumeration_lists_the_attached_boards() {
            let serials = list().expect("enumeration should work");

            assert!(
                serials.iter().all(|s| !s.is_empty()),
                "a serial number should never be empty: {serials:?}"
            );
        }

        fn attached() -> Vec<String> {
            let serials = list().expect("enumeration should work");
            assert!(!serials.is_empty(), "no YKUSH3 attached");
            serials
        }

        #[test]
        #[ignore = "needs a YKUSH3 attached"]
        fn a_board_is_found_and_answers_a_report() {
            let serials = attached();

            let board = Board::open(None).expect("the first board should open");
            let answer = board
                .transfer(&report(&[0x61, 0x02]))
                .expect("the board should answer the firmware version");

            assert_eq!(answer[0], 0x01, "status byte");
            assert_eq!(answer[1], 0x61, "echoed opcode");
            assert_eq!(serials.len(), list().unwrap().len(), "stable enumeration");
        }

        #[test]
        #[ignore = "needs a YKUSH3 attached"]
        fn a_board_can_be_addressed_by_serial_number() {
            let serials = attached();

            let board = Board::open(Some(&serials[0])).expect("open by serial number");
            let answer = board.transfer(&report(&[0x61, 0x01])).expect("answer");

            assert_eq!(answer[1], 0x61);
        }

        /// Keeps one handle open and reads port states over and over, the way
        /// a test rig polls a board. Every answer has to arrive within the
        /// read timeout. Read-only: the ports power real boards, so nothing
        /// is switched.
        ///
        /// 2000 reads by default; `YKUSH3_SOAK_COUNT` sets another number.
        /// Run with `--nocapture` to see the count and the slowest answer.
        #[test]
        #[ignore = "needs a YKUSH3 attached"]
        fn a_held_handle_answers_every_status_read_in_time() {
            use crate::ykush3::{Port, Ykush3};
            use std::time::Instant;

            attached();
            let count: u32 = std::env::var("YKUSH3_SOAK_COUNT")
                .map(|n| n.parse().expect("YKUSH3_SOAK_COUNT should be a number"))
                .unwrap_or(2000);

            let board = Ykush3::open(None).expect("the first board should open");
            let mut slowest = Duration::ZERO;
            let started = Instant::now();

            for i in 0..count {
                let port = Port::Downstream(1 + (i % 3) as u8);
                let sent = Instant::now();
                board
                    .port_status(port)
                    .unwrap_or_else(|e| panic!("read {} of {count}, {port:?}: {e}", i + 1));
                let took = sent.elapsed();

                assert!(
                    took < READ_TIMEOUT,
                    "read {} of {count} took {took:?}",
                    i + 1
                );
                slowest = slowest.max(took);
            }

            println!(
                "soak: {count} status reads in {:?}, slowest answer {slowest:?}",
                started.elapsed()
            );
        }

        /// A board this process holds is not handed out a second time. On
        /// Linux that is the flock. On macOS it would be the system refusing
        /// the second open, which it does not do today: the transport opens
        /// without asking for exclusive access, so the second open goes
        /// through. The test is therefore expected to fail there until the
        /// planned seize-mode open in the transport, which is expected to
        /// change that (not yet measured). If it does, the expectation below
        /// makes the test fail, as a reminder to drop it.
        /// Read-only: nothing is sent to the board.
        #[test]
        #[ignore = "needs a YKUSH3 attached"]
        #[cfg_attr(
            target_os = "macos",
            should_panic(expected = "a second open should be refused as busy")
        )]
        fn a_held_board_is_not_opened_a_second_time() {
            attached();

            let first = Board::open(None).expect("the first board should open");
            let second = Board::open_within(None, Duration::ZERO);

            assert!(
                matches!(&second, Err(Error::Busy { .. })),
                "a second open should be refused as busy, got {:?}",
                second.as_ref().err()
            );
            drop(first);
            Board::open_within(None, Duration::ZERO)
                .expect("the board should open again once the first handle is gone");
        }

        #[test]
        #[ignore = "needs a YKUSH3 attached"]
        fn an_unknown_serial_number_is_reported_as_not_found() {
            attached();

            let result = Board::open(Some("no-such-board"));

            assert!(matches!(result, Err(Error::NotFound { serial: Some(_) })));
        }
    }
}
