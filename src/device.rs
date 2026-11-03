// SPDX-License-Identifier: Apache-2.0
//! USB HID transport for the YKUSH3 board.
//!
//! Talks to the board through async-hid, which reaches the device by way of
//! IOKit on macOS and through hidraw on Linux and FreeBSD. That
//! library is asynchronous because that is how macOS delivers HID reports - it
//! hands out callbacks on a run loop rather than a blocking read. This program
//! is not asynchronous and has no reason to be, so the awaits are resolved here
//! and everything above the [`Transport`] trait stays as synchronous as it was.
//!
//! The report exchange is the same on all systems. What differs is the way to
//! the device: Linux guards `/dev/hidraw*` with file permissions, which a udev
//! rule has to open up, and a hidraw node exists only while the kernel's HID
//! driver is bound to the board. Both failures are reported in words here
//! rather than as a bare errno or as a board that is not there.
//!
//! Opening also waits its turn when another invocation is using the board,
//! see [`crate::lock`].

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use async_hid::Access;
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

/// How long a write or a read may take before the board counts as not
/// answering.
const TIMEOUT: Duration = Duration::from_secs(5);

/// How long to wait, after what looks like the answer to the version query,
/// for another report to follow. Long enough to catch one already on its
/// way, short enough not to be felt.
const SETTLE_WAIT: Duration = Duration::from_millis(50);

/// Upper bound on the reports dropped while getting back in step, so a board
/// that streams cannot hold the program there.
const DISCARD_LIMIT: usize = 16;

/// The firmware version query, opcode `0x61` with selector `0x02`. It changes
/// nothing on the board, and its answer, `01 61` followed by the version, is
/// what marks where the answers to earlier commands end. Firmware before 1.1.0
/// answers it with an empty report instead.
const VERSION_QUERY: [u8; 2] = [0x61, 0x02];

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

    /// Called when an answer did not fit its request: it may have been meant
    /// for an earlier one, so the next transfer must not trust the queue.
    fn desync(&self) {}
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
    /// board is driven from one thread at a time. On macOS the handle holds
    /// the board exclusively; on Linux the turn below keeps other
    /// invocations out, see the README on concurrent use.
    dev: RefCell<DeviceReaderWriter>,

    /// Whether the next report to arrive is the answer to the next command.
    /// Not so on a fresh handle, which may find an answer an earlier process
    /// left behind, nor after a failed or unfitting exchange.
    in_step: Cell<bool>,

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

        // macOS: the handle itself is the turn. The board is opened
        // exclusively (seized), so every other open fails with Busy until
        // this handle is dropped, and such an open is repeated. See
        // crate::lock.
        #[cfg(target_os = "macos")]
        let dev = {
            use crate::lock::Attempt;

            let (found, device) = future::block_on(choose(serial))?;
            // None while the board re-enumerates, e.g. reset by the holder.
            let mut device = Some(device);
            // Once it was held elsewhere, the holder may have reset it.
            let mut may_reset = false;
            crate::lock::wait_for(&found, wait, || {
                let current = match device.take() {
                    Some(current) => current,
                    None => match future::block_on(choose(Some(&found))) {
                        Ok((_, again)) => again,
                        Err(error @ Error::NotFound { .. }) => return Attempt::Gone(error),
                        Err(error) => return Attempt::Failed(error),
                    },
                };
                match future::block_on(current.open_options().access(Access::Exclusive).open()) {
                    Ok(dev) => Attempt::Got(dev),
                    Err(error) if refused_as_busy(&error) => {
                        device = Some(current);
                        may_reset = true;
                        Attempt::Busy(crate::lock::HELD_EXCLUSIVELY.to_owned())
                    }
                    // IOKit names a vanished or returning board in several ways.
                    Err(error)
                        if may_reset || matches!(error, async_hid::HidError::NotConnected) =>
                    {
                        Attempt::Gone(open_failed(error))
                    }
                    Err(error) => Attempt::Failed(open_failed(error)),
                }
            })?
        };

        // FreeBSD: hidraw takes one open at a time, so the handle is the turn
        // as on macOS. Listing opens the node too, so a held board is missing
        // from the list rather than refused.
        #[cfg(target_os = "freebsd")]
        let dev = {
            use crate::lock::{Attempt, HELD_OPEN};

            // Without a serial number, the first board by node, held or not.
            let first = serial
                .is_none()
                .then(|| held_board(None))
                .flatten()
                .map(|(found, _)| found);
            let serial = serial.or(first.as_deref());

            let open = |device: Device| match future::block_on(device.open()) {
                Ok(dev) => Attempt::Got(dev),
                Err(error) if refused_as_busy(&error) => Attempt::Busy(HELD_OPEN.to_owned()),
                Err(error @ async_hid::HidError::NotConnected) => Attempt::Gone(error.into()),
                Err(error) => Attempt::Failed(open_failed(error)),
            };
            // Once the board is missing from the list, only its node is tried.
            let mut held: Option<(String, std::path::PathBuf)> = None;
            // Once it was gone, e.g. reset by the holder, it may be coming back.
            let mut gone = false;
            crate::lock::wait_for(serial.unwrap_or_default(), wait, || {
                let outcome = 'attempt: {
                    let (found, node) = match held.take() {
                        Some(board) => board,
                        None => match future::block_on(choose(serial)) {
                            Ok((_, device)) => break 'attempt open(device),
                            Err(error @ Error::NotFound { .. }) => match held_board(serial) {
                                Some(board) => board,
                                None if gone => break 'attempt Attempt::Gone(error),
                                None => break 'attempt Attempt::Failed(error),
                            },
                            Err(error) => break 'attempt Attempt::Failed(error),
                        },
                    };
                    let device = future::block_on(node_device(&node, &found));
                    held = Some((found, node));
                    match device {
                        Ok(device) => open(device),
                        Err(error) if refused_as_busy(&error) => {
                            Attempt::Busy(HELD_OPEN.to_owned())
                        }
                        Err(error @ async_hid::HidError::NotConnected) => {
                            Attempt::Gone(error.into())
                        }
                        Err(error) => Attempt::Failed(open_failed(error)),
                    }
                };
                if let Attempt::Gone(_) = outcome {
                    gone = true;
                    held = None;
                }
                outcome
            })
            .map_err(|error| match error {
                Error::Busy { waited, cause, .. } => Error::Busy {
                    serial: held_board(serial)
                        .map(|(found, _)| found)
                        .or_else(|| serial.map(str::to_owned))
                        .unwrap_or_default(),
                    waited,
                    cause,
                },
                error => error,
            })?
        };

        Ok(Board {
            dev: RefCell::new(dev),
            in_step: Cell::new(false),
            #[cfg(target_os = "linux")]
            _turn: turn,
        })
    }
}

impl Transport for Board {
    fn transfer(&self, out: &Report) -> Result<Report> {
        exchange(&mut *self.dev.borrow_mut(), &self.in_step, out)
    }

    fn send(&self, out: &Report) -> Result<()> {
        // The board reboots and leaves nothing to get in step with.
        self.in_step.set(false);
        self.dev
            .borrow_mut()
            .write(out, TIMEOUT)
            .ok_or(Error::NoResponse)?
    }

    fn desync(&self) {
        self.in_step.set(false);
    }
}

/// One report in each direction, as the transport moves them; `None` when
/// the time ran out. The exchange below is written against this so that it
/// can be tested without a board.
trait Link {
    fn write(&mut self, out: &Report, timeout: Duration) -> Option<Result<()>>;
    fn read(&mut self, into: &mut Report, timeout: Duration) -> Option<Result<usize>>;
}

impl Link for DeviceReaderWriter {
    fn write(&mut self, out: &Report, timeout: Duration) -> Option<Result<()>> {
        // The first byte is the report id, which the library strips when it
        // is zero. The board uses unnumbered reports, so a leading zero is
        // what it wants: passing the bare report instead would send it under
        // whatever its first byte happens to be.
        let mut buf = [0u8; REPORT_SIZE + 1];
        buf[1..].copy_from_slice(out);
        // The library has no timeouts, so each call races a timer: a board
        // that stalls must not hang the program. Dropping the losing call
        // leaves the handle usable.
        future::block_on(future::or(
            async { Some(self.write_output_report(&buf).await.map_err(Error::from)) },
            async {
                async_io::Timer::after(timeout).await;
                None
            },
        ))
    }

    fn read(&mut self, into: &mut Report, timeout: Duration) -> Option<Result<usize>> {
        future::block_on(future::or(
            async { Some(self.read_input_report(into).await.map_err(Error::from)) },
            async {
                async_io::Timer::after(timeout).await;
                None
            },
        ))
    }
}

/// Sends `out` and returns the answer, after getting back in step first when
/// `in_step` says the queue may hold answers to earlier commands. `in_step`
/// is set again only by a complete answer.
///
/// Taken for the answer to this command, an answer meant for an earlier one
/// would mostly pass unnoticed: the switching and configuration commands
/// carry nothing beyond the status byte to check it against.
fn exchange(link: &mut impl Link, in_step: &Cell<bool>, out: &Report) -> Result<Report> {
    if !in_step.replace(false) {
        resync(link)?;
    }
    link.write(out, TIMEOUT).ok_or(Error::NoResponse)??;

    let mut answer: Report = [0; REPORT_SIZE];
    let read = link.read(&mut answer, TIMEOUT).ok_or(Error::NoResponse)??;
    // The board always answers with a full report. Anything shorter would
    // leave the zero padding of the buffer to be read as answer bytes.
    full_report(read)?;
    in_step.set(true);
    Ok(answer)
}

/// Asks for the firmware version and drops every report up to its answer.
/// The board answers in order, so whatever was queued before - a late
/// answer, or one an earlier process left behind - comes first.
///
/// What looks like the answer may still be one left behind: an earlier
/// process may have been killed during this very query, and firmware before
/// 1.1.0 answers it with an empty report, the same as a refusal. Taken too
/// early, the real answer would wait in the queue and pass for the
/// acknowledgement of the next command. So the queue has to stay quiet for
/// a moment after it; another report means the wait goes on.
fn resync(link: &mut impl Link) -> Result<()> {
    link.write(&report(&VERSION_QUERY), TIMEOUT)
        .ok_or(Error::NoResponse)??;

    let deadline = Instant::now() + TIMEOUT;
    let mut answer: Report = [0; REPORT_SIZE];
    let mut settling = false;
    // The reports dropped, the answer, and the quiet after it.
    for _ in 0..DISCARD_LIMIT + 2 {
        let wait = if settling {
            SETTLE_WAIT
        } else {
            deadline.saturating_duration_since(Instant::now())
        };
        let read = match link.read(&mut answer, wait) {
            None if settling => return Ok(()),
            None => return Err(Error::NoResponse),
            Some(read) => read?,
        };
        settling = read == REPORT_SIZE
            && (answer[..2] == [0x01, VERSION_QUERY[0]] || answer.iter().all(|&b| b == 0));
    }
    Err(Error::Device(format!(
        "The board sent more than {DISCARD_LIMIT} reports before answering the version query"
    )))
}

/// Serial numbers of all attached YKUSH3 boards, in enumeration order.
pub fn list() -> Result<Vec<String>> {
    let attached = future::block_on(attached_boards())?
        .into_iter()
        .map(|(serial, _)| serial);
    // A board another program has open is missing from the device list.
    #[cfg(target_os = "freebsd")]
    let attached = hidraw_boards()
        .into_iter()
        .map(|(serial, _)| serial)
        .chain(attached);

    let mut serials: Vec<String> = Vec::new();
    for serial in attached {
        if !serials.contains(&serial) {
            serials.push(serial);
        }
    }
    Ok(serials)
}

/// What to tell a user whose operating system refused to open the board.
#[cfg(target_os = "linux")]
pub(crate) const PERMISSION_DENIED: &str = "Permission denied on the hidraw device of the \
YKUSH3. Allow access with a udev rule, /etc/udev/rules.d/60-ykush.rules as \
shown in the README, make this user a member of the group the rule names, and \
replug the board";
#[cfg(target_os = "freebsd")]
pub(crate) const PERMISSION_DENIED: &str = "Permission denied on the hidraw device of the \
YKUSH3. Allow access with a devfs.rules entry as shown in the README and make this \
user a member of the group it names";
#[cfg(target_os = "macos")]
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
        Error::PermissionDenied
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
    detached_message(&detached_on_this_host(serial)).map(Error::Unavailable)
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

/// The serial number of a YKUSH3 in the `%pnpinfo` of a FreeBSD USB device,
/// or `None` for any other device.
#[cfg_attr(not(target_os = "freebsd"), allow(dead_code))]
fn ykush3_serial(pnpinfo: &str) -> Option<String> {
    let field = |key: &str| {
        pnpinfo
            .split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
    };
    let hex = |key: &str| u16::from_str_radix(field(key)?.strip_prefix("0x")?, 16).ok();

    (hex("vendor")? == VENDOR_ID && hex("product")? == PRODUCT_ID).then(|| {
        match field("sernum").unwrap_or("").trim_matches('"') {
            // Named as the device list names a board without one.
            "" => "<unknown>".to_owned(),
            sernum => sanitize(sernum),
        }
    })
}

/// The serial numbers and nodes of all YKUSH3 boards with a hidraw node,
/// read through sysctl: listing opens the node, and FreeBSD lets one handle
/// open it at a time, so a board another program has open is missing from the
/// list.
#[cfg(target_os = "freebsd")]
fn hidraw_boards() -> Vec<(String, std::path::PathBuf)> {
    let sysctl = |name: String| {
        let out = std::process::Command::new("/sbin/sysctl")
            .args(["-n", &name])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    // "hidbus3" names the sysctl node "dev.hidbus.3".
    let node = |dev: &str| {
        let unit = dev.trim_start_matches(|c: char| !c.is_ascii_digit());
        format!("dev.{}.{unit}", &dev[..dev.len() - unit.len()])
    };

    let Ok(dev) = std::fs::read_dir("/dev") else {
        return Vec::new();
    };
    let mut boards: Vec<_> = dev
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with("hidraw"))
        .filter_map(|hidraw| {
            // hidraw sits on hidbus, which sits on the USB driver.
            let hidbus = sysctl(format!("{}.%parent", node(&hidraw)))?;
            let usb = sysctl(format!("{}.%parent", node(&hidbus)))?;
            let serial = ykush3_serial(&sysctl(format!("{}.%pnpinfo", node(&usb)))?)?;
            Some((serial, std::path::Path::new("/dev").join(hidraw)))
        })
        .collect();
    // In the node order of the device list.
    boards.sort_by(|a, b| a.1.cmp(&b.1));
    boards
}

/// The board `serial` on `node`, read without listing the other nodes.
#[cfg(target_os = "freebsd")]
async fn node_device(node: &std::path::Path, serial: &str) -> async_hid::HidResult<Device> {
    let id = async_hid::DeviceId::DevPath(node.to_path_buf());
    backend()
        .query_devices(&id)
        .await?
        .find(|dev| {
            dev.vendor_id == VENDOR_ID
                && dev.product_id == PRODUCT_ID
                && sanitize(dev.serial_number.as_deref().unwrap_or("<unknown>")) == serial
        })
        .ok_or(async_hid::HidError::NotConnected)
}

/// The board with `serial`, or any board, among those with a hidraw node.
#[cfg(target_os = "freebsd")]
fn held_board(serial: Option<&str>) -> Option<(String, std::path::PathBuf)> {
    hidraw_boards()
        .into_iter()
        .find(|(found, _)| serial.is_none_or(|wanted| found == wanted))
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

/// Whether an open failed because another handle holds the board
/// exclusively, which waiting may cure. The transport reports exactly that
/// as `Busy` (`kIOReturnExclusiveAccess` on macOS); every other failure is
/// reported at once.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn refused_as_busy(error: &async_hid::HidError) -> bool {
    matches!(error, async_hid::HidError::Busy)
}

/// The hidraw node of a board found by the transport.
///
/// On Linux the transport identifies a device by its directory in sysfs,
/// `.../hidraw/hidrawN`, and opens the node named in the `uevent` file there.
/// The lock goes on the same node.
#[cfg(target_os = "linux")]
fn hidraw_node_of(device: &Device) -> Result<std::path::PathBuf> {
    let async_hid::DeviceId::DevPath(sysfs) = &device.id else {
        return Err(Error::Unavailable("The YKUSH3 has no hidraw device".into()));
    };

    hidraw_node(sysfs).ok_or_else(|| {
        Error::Unavailable(format!(
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

/// One backend for the process. It is a handle rather than a resource, but
/// making it once keeps the enumeration from building it again per call.
fn backend() -> &'static HidBackend {
    static BACKEND: OnceLock<HidBackend> = OnceLock::new();
    BACKEND.get_or_init(HidBackend::default)
}

/// Every attached board, as its sanitised serial number and the handle it was
/// found under, in enumeration order.
async fn attached_boards() -> Result<Vec<(String, Device)>> {
    let mut found: Vec<(String, Device)> = Vec::new();

    // A HID stack that cannot even list its devices is an unavailable
    // service, not an I/O error that a retry would cure.
    let mut devices = backend()
        .enumerate()
        .await
        .map_err(|e| Error::Unavailable(format!("Cannot list the HID devices: {e}")))?;
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

/// A report shorter than 64 bytes is a failed transfer: the read came back
/// short, and reading the zero padding of the buffer as answer bytes would
/// turn it into a wrong answer. It is an I/O error, not an answer that does
/// not fit the request.
fn full_report(read: usize) -> Result<()> {
    if read == REPORT_SIZE {
        Ok(())
    } else {
        Err(Error::Hid(format!(
            "Truncated answer from the board: {read} of {REPORT_SIZE} bytes"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_board_is_recognised_in_the_pnp_info_of_freebsd() {
        let ykush3 =
            r#"vendor=0x04d8 product=0xf11b devclass=0x00 sernum="Y3N13808" release=0x0100"#;
        assert_eq!(ykush3_serial(ykush3), Some("Y3N13808".to_owned()));
        let mouse = r#"vendor=0x0e0f product=0x0003 sernum="" release=0x0102"#;
        assert_eq!(ykush3_serial(mouse), None);
        let unnamed = r#"vendor=0x04d8 product=0xf11b sernum="""#;
        assert_eq!(ykush3_serial(unnamed), Some("<unknown>".to_owned()));
        assert_eq!(ykush3_serial("pci0:0:0:0"), None);
    }

    #[test]
    fn a_truncated_report_is_an_io_error_not_an_unfitting_answer() {
        assert!(full_report(REPORT_SIZE).is_ok());
        for read in [0, 1, REPORT_SIZE - 1] {
            let error = full_report(read).unwrap_err();
            assert!(matches!(error, Error::Hid(_)), "{read}: {error:?}");
            assert_eq!(error.exit_code(), 74);
        }
    }

    #[test]
    fn payload_is_padded_to_a_full_report() {
        let r = report(&[0x52, 0x01, 0x20, 0x02, 0xaa, 0xbb]);

        assert_eq!(r.len(), REPORT_SIZE);
        assert_eq!(&r[..6], &[0x52, 0x01, 0x20, 0x02, 0xaa, 0xbb]);
        assert!(r[6..].iter().all(|&b| b == 0));
    }

    /// What a [`FakeLink`] read returns next.
    enum Incoming {
        Answer(Report),
        Short(usize),
        Fails,
        Silent,
    }

    /// A link that records what is written and reads from a prepared queue;
    /// an empty queue reads as silence.
    #[derive(Default)]
    struct FakeLink {
        written: Vec<Report>,
        incoming: std::collections::VecDeque<Incoming>,
        write_hangs: bool,
    }

    impl FakeLink {
        fn answers(payloads: &[&[u8]]) -> Self {
            let mut link = FakeLink::default();
            for payload in payloads {
                link.incoming.push_back(Incoming::Answer(report(payload)));
            }
            link
        }

        fn then(mut self, incoming: Incoming) -> Self {
            self.incoming.push_back(incoming);
            self
        }

        fn written(&self) -> Vec<Vec<u8>> {
            self.written.iter().map(|r| r[..2].to_vec()).collect()
        }
    }

    impl Link for FakeLink {
        fn write(&mut self, out: &Report, _: Duration) -> Option<Result<()>> {
            if self.write_hangs {
                return None;
            }
            self.written.push(*out);
            Some(Ok(()))
        }

        fn read(&mut self, into: &mut Report, _: Duration) -> Option<Result<usize>> {
            match self.incoming.pop_front().unwrap_or(Incoming::Silent) {
                Incoming::Answer(r) => {
                    *into = r;
                    Some(Ok(REPORT_SIZE))
                }
                Incoming::Short(n) => Some(Ok(n)),
                Incoming::Fails => Some(Err(Error::Hid("read failed".into()))),
                Incoming::Silent => None,
            }
        }
    }

    const STATUS_1: [u8; 1] = [0x21];
    const VERSION: &[u8] = &[0x01, 0x61, 1, 5, 0];
    const EMPTY: &[u8] = &[];

    #[test]
    fn a_fresh_handle_drops_an_answer_left_behind_by_an_earlier_process() {
        // A process ended between its write and its read; its answer comes
        // before anything this one asks.
        let mut link = FakeLink::answers(&[&[0x01, 0x11], VERSION]);
        link.incoming.push_back(Incoming::Silent);
        link.incoming
            .push_back(Incoming::Answer(report(&[0x01, 0x01])));
        let in_step = Cell::new(false);

        let answer = exchange(&mut link, &in_step, &report(&STATUS_1)).unwrap();

        assert_eq!(answer[..2], [0x01, 0x01]);
        assert_eq!(link.written(), [VERSION_QUERY.to_vec(), vec![0x21, 0x00]]);
        assert!(in_step.get());
    }

    #[test]
    fn a_handle_in_step_sends_the_command_alone() {
        let mut link = FakeLink::answers(&[&[0x01, 0x11]]);
        let in_step = Cell::new(true);

        exchange(&mut link, &in_step, &report(&STATUS_1)).unwrap();

        assert_eq!(link.written(), [vec![0x21, 0x00]]);
        assert!(in_step.get());
    }

    #[test]
    fn every_failed_exchange_leaves_the_handle_out_of_step() {
        for (what, incoming) in [
            ("timeout", Incoming::Silent),
            ("short report", Incoming::Short(10)),
            ("read error", Incoming::Fails),
        ] {
            let mut link = FakeLink::default().then(incoming);
            let in_step = Cell::new(true);

            assert!(
                exchange(&mut link, &in_step, &report(&STATUS_1)).is_err(),
                "{what}"
            );
            assert!(!in_step.get(), "{what}");
        }
    }

    #[test]
    fn a_late_answer_is_dropped_by_the_next_exchange() {
        let mut link = FakeLink::default()
            .then(Incoming::Silent)
            .then(Incoming::Answer(report(&[0x01, 0x11])))
            .then(Incoming::Answer(report(VERSION)))
            .then(Incoming::Silent)
            .then(Incoming::Answer(report(&[0x01, 0x02])));
        let in_step = Cell::new(true);

        assert!(matches!(
            exchange(&mut link, &in_step, &report(&STATUS_1)),
            Err(Error::NoResponse)
        ));
        let answer = exchange(&mut link, &in_step, &report(&[0x22])).unwrap();

        assert_eq!(answer[..2], [0x01, 0x02]);
    }

    #[test]
    fn a_version_answer_left_behind_does_not_end_the_resync_early() {
        // An earlier process was killed during its own version query. Taken
        // for the answer, the real one would pass for the acknowledgement of
        // the switching command that follows.
        let mut link = FakeLink::answers(&[VERSION, VERSION]);
        link.incoming.push_back(Incoming::Silent);
        link.incoming
            .push_back(Incoming::Answer(report(&[0x01, 0x11])));
        let in_step = Cell::new(false);

        let answer = exchange(&mut link, &in_step, &report(&[0x11])).unwrap();

        assert_eq!(answer[..2], [0x01, 0x11]);
        assert!(link.incoming.is_empty());
    }

    #[test]
    fn firmware_before_1_1_0_answers_the_query_with_an_empty_report() {
        let mut link = FakeLink::answers(&[EMPTY, &[0x01, 0x11]]);
        // The empty report is followed by silence, then the command answers.
        link.incoming.insert(1, Incoming::Silent);
        let in_step = Cell::new(false);

        let answer = exchange(&mut link, &in_step, &report(&STATUS_1)).unwrap();

        assert_eq!(answer[..2], [0x01, 0x11]);
    }

    #[test]
    fn an_empty_report_left_behind_does_not_end_the_resync_early() {
        // A newer board: the empty report was an earlier refusal, and the
        // real answer to the query follows at once.
        let mut link = FakeLink::answers(&[EMPTY, VERSION]);
        link.incoming.push_back(Incoming::Silent);
        link.incoming
            .push_back(Incoming::Answer(report(&[0x01, 0x11])));
        let in_step = Cell::new(false);

        let answer = exchange(&mut link, &in_step, &report(&STATUS_1)).unwrap();

        assert_eq!(answer[..2], [0x01, 0x11]);
    }

    #[test]
    fn an_old_board_with_an_empty_report_left_behind_gets_in_step() {
        let mut link = FakeLink::answers(&[EMPTY, EMPTY]);
        link.incoming.push_back(Incoming::Silent);
        link.incoming
            .push_back(Incoming::Answer(report(&[0x01, 0x11])));
        let in_step = Cell::new(false);

        let answer = exchange(&mut link, &in_step, &report(&STATUS_1)).unwrap();

        assert_eq!(answer[..2], [0x01, 0x11]);
    }

    #[test]
    fn a_short_report_while_getting_in_step_is_dropped() {
        let mut link = FakeLink::default()
            .then(Incoming::Short(2))
            .then(Incoming::Answer(report(VERSION)))
            .then(Incoming::Silent)
            .then(Incoming::Answer(report(&[0x01, 0x11])));
        let in_step = Cell::new(false);

        let answer = exchange(&mut link, &in_step, &report(&STATUS_1)).unwrap();

        assert_eq!(answer[..2], [0x01, 0x11]);
    }

    #[test]
    fn a_read_error_while_getting_in_step_is_reported() {
        let mut link = FakeLink::default().then(Incoming::Fails);
        let in_step = Cell::new(false);

        assert!(matches!(
            exchange(&mut link, &in_step, &report(&STATUS_1)),
            Err(Error::Hid(_))
        ));
        assert_eq!(link.written(), [VERSION_QUERY.to_vec()]);
        assert!(!in_step.get());
    }

    #[test]
    fn a_write_that_does_not_finish_in_time_is_no_response() {
        let mut link = FakeLink {
            write_hangs: true,
            ..FakeLink::default()
        };
        let in_step = Cell::new(true);

        assert!(matches!(
            exchange(&mut link, &in_step, &report(&STATUS_1)),
            Err(Error::NoResponse)
        ));
        assert!(!in_step.get());
    }

    #[test]
    fn a_board_that_does_not_answer_the_version_query_is_no_response() {
        let mut link = FakeLink::default();
        let in_step = Cell::new(false);

        assert!(matches!(
            exchange(&mut link, &in_step, &report(&STATUS_1)),
            Err(Error::NoResponse)
        ));
        assert_eq!(link.written(), [VERSION_QUERY.to_vec()]);
        assert!(!in_step.get());
    }

    #[test]
    fn as_many_reports_as_the_limit_allows_are_dropped() {
        let mut link = FakeLink::default();
        for _ in 0..DISCARD_LIMIT {
            link.incoming
                .push_back(Incoming::Answer(report(&[0x01, 0x11])));
        }
        link.incoming.push_back(Incoming::Answer(report(VERSION)));
        link.incoming.push_back(Incoming::Silent);
        link.incoming
            .push_back(Incoming::Answer(report(&[0x01, 0x10])));
        let in_step = Cell::new(false);

        let answer = exchange(&mut link, &in_step, &report(&STATUS_1)).unwrap();

        assert_eq!(answer[..2], [0x01, 0x10]);
    }

    #[test]
    fn a_board_that_keeps_sending_empty_reports_is_given_up_on() {
        let mut link = FakeLink::default();
        for _ in 0..DISCARD_LIMIT + 2 {
            link.incoming.push_back(Incoming::Answer(report(EMPTY)));
        }
        let in_step = Cell::new(false);

        assert!(matches!(
            exchange(&mut link, &in_step, &report(&STATUS_1)),
            Err(Error::Device(_))
        ));
        assert!(!in_step.get());
    }

    #[test]
    fn a_board_that_never_gets_to_the_version_answer_is_given_up_on() {
        let mut link = FakeLink::default();
        for _ in 0..=DISCARD_LIMIT {
            link.incoming
                .push_back(Incoming::Answer(report(&[0x01, 0x11])));
        }
        link.incoming.push_back(Incoming::Answer(report(VERSION)));
        let in_step = Cell::new(false);

        assert!(matches!(
            exchange(&mut link, &in_step, &report(&STATUS_1)),
            Err(Error::Device(_))
        ));
        assert!(!in_step.get());
    }

    fn os_error(kind: std::io::ErrorKind) -> async_hid::HidError {
        async_hid::HidError::from(std::io::Error::from(kind))
    }

    #[test]
    fn a_refused_open_says_what_to_do_about_it() {
        let error = open_failed(os_error(std::io::ErrorKind::PermissionDenied));

        assert!(matches!(&error, Error::PermissionDenied));
        assert_eq!(error.to_string(), PERMISSION_DENIED);
        #[cfg(target_os = "linux")]
        assert!(error
            .to_string()
            .contains("/etc/udev/rules.d/60-ykush.rules"));
    }

    #[test]
    fn any_other_open_failure_keeps_its_own_message() {
        let error = open_failed(os_error(std::io::ErrorKind::NotFound));

        assert!(matches!(&error, Error::Hid(msg) if msg != PERMISSION_DENIED));
        // A board that went away while being opened is unavailable.
        let error = open_failed(async_hid::HidError::NotConnected);
        assert!(matches!(&error, Error::Unavailable(_)), "{error:?}");
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
    fn only_busy_from_the_transport_counts_as_a_board_in_use() {
        use async_hid::HidError;

        assert!(refused_as_busy(&HidError::Busy));
        // The messages the earlier transport used for a refused open are
        // ordinary failures now; the transport reports a seized device as
        // Busy (sidit77/async-hid#55).
        assert!(!refused_as_busy(&HidError::message(
            "Failed to open device"
        )));
        assert!(!refused_as_busy(&HidError::message(
            "exclusive access and device already open"
        )));
        assert!(!refused_as_busy(&HidError::NotConnected));
        assert!(!refused_as_busy(&HidError::Unsupported));
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
    /// not optional. On Linux a second open is not refused and each answer
    /// reaches every open handle, so tests running side by side would read
    /// each other's answers; on macOS the exclusive open makes them wait for
    /// each other.
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

                assert!(took < TIMEOUT, "read {} of {count} took {took:?}", i + 1);
                slowest = slowest.max(took);
            }

            println!(
                "soak: {count} status reads in {:?}, slowest answer {slowest:?}",
                started.elapsed()
            );
        }

        /// A board this process holds is not handed out a second time. On
        /// Linux that is the flock, on macOS the exclusive open: the system
        /// refuses the second open with Busy. Passes on macOS since the
        /// exclusive open (2026-11-02).
        /// Read-only: nothing is sent to the board.
        #[test]
        #[ignore = "needs a YKUSH3 attached"]
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
