# ykush-rs

An independent Rust implementation of the YKUSH3 control protocol, **for macOS on Apple
silicon and for Linux and FreeBSD on 64 bit Arm**. YKUSH3 only (VID `0x04D8`, PID `0xF11B`) - YKUSH,
YKUSH2 and YKUSHXS are not covered.

It was written against Yepkit's published protocol documentation, not derived from the
source of their C++ `ykushcmd`: the structure, the architecture and every line of code are
new work. That program serves here as a reference to compare against, and the
[differences table](#where-it-differs-from-the-c-original) records where the two part ways.

1. [Building](#building)
2. [Using it](#using-it)
3. [The hardware](#the-hardware)
4. [USB control protocol](#usb-control-protocol)
5. [How it is put together](#how-it-is-put-together)
6. [Tests](#tests)
7. [Checked against the hardware](#checked-against-the-hardware)
8. [Where it differs from the C++ original](#where-it-differs-from-the-c-original)
9. [Driving a YKUR relay board over I2C](#driving-a-ykur-relay-board-over-i2c)
10. [Documents and sources](#documents-and-sources)
11. [License](#license)

---

## Building

```
cargo build --release
```

The result is a single binary, `target/release/ykushctrl`.

With a Rust toolchain (1.86 or newer) installed, building and installing is one step,
no clone needed:

```
cargo install --locked --git https://github.com/gerhard-schuster/ykush-rs
```

This puts `ykushctrl` into `~/.cargo/bin`. The `--locked` matters: without it, cargo
resolves the dependencies afresh instead of taking the versions pinned in `Cargo.lock` -
the ones the tests and the dependency audit actually ran against. `--tag` picks a release;
up to v0.4.1 the program was called `ykush3cmd` and the repository `ykush3cmd`.

**For macOS on Apple silicon and for Linux and FreeBSD on aarch64.** Those are the systems it
has been built on and run against a board - Linux on a Raspberry Pi 4 under Debian 13, FreeBSD
15.1 in a VM - and the source refuses to compile anywhere else rather than produce a binary
nobody has tried. x86_64 is left out for that reason alone: nothing in the code depends on the
processor, but no board has been driven from such a machine yet.

The HID access goes through the `async-hid` crate, which reaches IOKit on macOS and the
kernel's hidraw interface on Linux and FreeBSD, all from Rust. No C is compiled and no library travels
alongside the binary, so a Rust toolchain is the whole requirement - no C compiler, no Xcode
command line tools, and on Linux neither `libudev` nor `pkg-config`: the devices are found
by reading `/sys`.

On macOS, reaching the device needs no special permission - the system hands it to any
process that asks. On Linux it takes a udev rule, on FreeBSD a devfs rule, see below. See
[SECURITY-REVIEW-0.4.0.md](SECURITY-REVIEW-0.4.0.md) for what that means, and
[SECURITY-REVIEW.md](SECURITY-REVIEW.md) for the earlier review of the `hidapi` transport.

Every push builds and tests on macOS and on Linux (arm64), see
[`.github/workflows/ci.yml`](.github/workflows/ci.yml).

### Linux

The board is a file, `/dev/hidrawN`, which belongs to root until a udev rule hands it to a
group. ykushctrl reaches the board only through that node; this rule gives it to `plugdev`:

```
# /etc/udev/rules.d/60-ykush.rules
KERNEL=="hidraw*", ATTRS{idVendor}=="04d8", ATTRS{idProduct}=="f11b", MODE="0660", GROUP="plugdev"
```

Then load it, make the user a member of the group, and replug the board:

```
sudo udevadm control --reload-rules
sudo usermod -aG plugdev "$USER"      # takes effect at the next login
```

Without the rule, every command that opens the board fails and names the rule file. `-l`
works without it, since listing reads `/sys` and opens nothing.

Three things to know on Linux, the last one applies on macOS in part (see
[Concurrent invocations](#concurrent-invocations)):

- **The kernel's HID driver has to be bound to the board.** A program that drives the
  YKUSH3 through libusb - Yepkit's `ykushcmd`, the Debian package `ykush-control` among
  them - detaches that driver and does not bind it again, and the hidraw node disappears with
  it. `lsusb` still lists the board, this program can no longer reach it, and says so,
  naming the interface; `-l` still prints its list and exits 0, with the hint on stderr.
  Replugging the board brings the driver back, and so does binding it by hand, for
  example `echo -n 1-1.4.4:1.0 | sudo tee /sys/bus/usb/drivers/usbhid/bind` with the
  interface name from the message.
- **The board is not opened exclusively.** Linux lets several programs open the hidraw node
  at once and delivers every answer to all of them (the kernel copies each report into the
  buffer of every open file, `hidraw_report_event` in `drivers/hid/hidraw.c`), so two
  programs talking to the board at the same time can read each other's answers.
  Invocations of `ykushctrl` take turns: each one takes an advisory `flock` on the board's
  `/dev/hidrawN` before it opens the board, and the next one waits for it, see
  [Concurrent invocations](#concurrent-invocations). No lock file is involved; the kernel
  drops the lock when the process ends, crash included.
- **Other tools are not covered by that lock.** Yepkit's `ykushcmd`, or any other HID tool,
  run against the same board while `ykushctrl` is running is not held back. A build of such a
  tool that goes through hidraw opens the node beside this program, and answers can reach
  the wrong process. A build that goes through libusb detaches the kernel's HID driver,
  and the board disappears from under this program, see the point above. **Use one YKUSH3
  tool per host.**

### FreeBSD

The board is `/dev/hidrawN` here too. It needs the `hidraw` module and the newer USB HID
driver, and a devfs rule that hands the nodes to a group, here `operator` (the rule covers
every hidraw node, devfs can not tell boards apart):

```
# /boot/loader.conf
hidraw_load="YES"
hw.usb.usbhid.enable="1"

# /etc/devfs.rules, then devfs_system_ruleset="hidraw" in /etc/rc.conf
[hidraw=10]
add path 'hidraw*' mode 0660 group operator
```

hidraw lets one program open a node at a time, so that open is the turn: a second invocation
waits for it, see [Concurrent invocations](#concurrent-invocations). A board another program
has open is missing from the device list; `ykushctrl` finds it through `sysctl` and waits for
it, and `-l` lists it.

## Using it

The **switches** are compatible with the original, and a leading `ykush3` is accepted and
ignored so existing invocations keep working. What the original tolerated silently - a
stray first word, a second `-s` - is an error here; the
[differences table](#where-it-differs-from-the-c-original) has the full list:

```
ykushctrl -u 1
ykushctrl ykush3 -u 1        # the same thing
```

The **messages** deliberately are not compatible - they are worded independently, see
[License](#license). A script that picks apart the output of the original needs adapting.
So does one that relies on the original's exit codes, which say little: see
[Exit status](#exit-status).

| Option | Effect |
|---|---|
| `-u 1\|2\|3\|4\|a` | power a port up (4 is the external 5V output, a is all of them) |
| `-d 1\|2\|3\|4\|a` | power a port down |
| `-g 1\|2\|3\|4` | report whether a port is on |
| `-on` / `-off` | switch the external 5V output |
| `-l` | list the attached boards |
| `-s <serial>` | address the board with that serial number |
| `--no-wait` | fail at once when another invocation is using the board, see below |
| `-c 1\|2\|3\|e 0\|1\|2` | configure the power-on state (off / on / last state) |
| `-r 1\|2\|3` | read a GPIO pin |
| `-w 1\|2\|3 0\|1` | drive a GPIO pin |
| `--gpio enable\|disable` | hand the GPIO pins to the board as a control interface |
| `--i2c-slave enable\|disable` | I2C slave mode |
| `--i2c-master enable\|disable` | I2C master mode |
| `--i2c-set-address <hex>` | the address the board answers to as a slave |
| `--i2c-write <hex-addr> <byte>...` | write bytes to an I2C slave (60 at most) |
| `--i2c-read <hex-addr> <n>` | read n bytes from an I2C slave (60 at most) |
| `--reset` | restart the board |
| `--boot` | stop in the bootloader |
| `--firmware-version` / `--bootloader-version` | print the versions |
| `-v`, `--version` / `-h`, `--help` | version and help |

### Exit status

The exit status tells a script what to do next; the values follow sysexits(3). The codes
are a stable interface, the wording of the messages is not.

| Code | Meaning | What to do |
|---|---|---|
| `0` | success; also `-l` with no board attached | go on |
| `64` | invalid command line | correct the call |
| `69` | no board, unknown serial number, the board gone, HID driver detached, no hidraw node, a node that keeps changing, the HID stack cannot list devices; **the board refused the command**: status `0x00`, or an I2C transfer failed | stop, check the board and its connection |
| `74` | no answer within the timeout, a truncated report, a transport error, output failed | retry if the command may be repeated |
| `75` | the board is in use (waited 5 seconds, or `--no-wait`) | retry later |
| `76` | an answer that does not fit the request: wrong echo, unknown status, wrong length | retry if the command may be repeated; if it recurs, check the board |
| `77` | access denied (on Linux the udev rule is missing) | fix the setup |
| `101` | a bug in this program (a Rust panic) | report it |

**May be repeated** are switching a port on or off, configuring a port, writing a GPIO,
enabling GPIO control, setting the I2C mode or address, and reading the board's own state
(ports, GPIO, versions): they set a state or only read, so repeating them gives the same
result.
**Not** blindly repeatable are I2C transfers, since the slave may act on a write or a read
twice (a counter, a FIFO), and `--reset` and `--boot`. After 74 or 76 the
board may have carried out the command before the answer was lost.

The protocol has no way to tell which request an answer belongs to, and the switching and
configuration commands answer with nothing beyond a status byte to check. So before its first
command, and after any exchange that failed or got an answer that does not fit, `ykushctrl` asks
for the firmware version and drops every report that arrives before that answer, then waits 50 ms
for the queue to stay quiet, since the answer may itself have been left behind. Firmware before
1.1.0 answers the version query with an empty report, which is handled the same way. An answer
left behind by an invocation that was killed between asking and reading is not taken for the
answer to this one. Every read gives up after 5 seconds, and so does every write on
macOS; on Linux the write runs inside the kernel, whose USB timeout bounds it.

sysexits(3) has no code for a device that refuses a command. A refusal is a valid answer, not
a protocol violation (`76`), so it gets the catch-all `69`. `77` is meant for permissions
above the file system; a denied device node is a file system matter, but its cause is the
setup, and that is what the code says. Before 0.5.0 every error was `1`.

```
$ ykushctrl -l
YKUSH3 boards on this host:
   1  Y3N13808

$ ykushctrl -s Y3N13808 -d 2      # cut power to port 2
$ ykushctrl -g 2
Port 2: off
$ ykushctrl -c 2 1                # port 2 comes up on the next reset
$ ykushctrl --firmware-version
Firmware 1.5.0
```

### Concurrent invocations

Two invocations addressing the same board at the same time take turns: the second waits
until the first is done, **up to 5 seconds**, and then gives up with exit status `75`:

```
$ ykushctrl -g 1
The YKUSH3 Y3N13808 is still not available after 5.0 s: another ykushctrl is using it
$ ykushctrl --no-wait -g 1
The YKUSH3 Y3N13808 is not available: another ykushctrl is using it
```

That is the message on Linux. On macOS the end reads `another program holds it exclusively`,
on FreeBSD `another program has it open`.

In the runs on macOS below, one invocation held the board for less than a tenth of a second,
most of it the 50 ms quiet after the version query, so five seconds lets dozens of them queue
up without any failing. A board taken for longer than that is held by something that
does not let go after one command - a program keeping a handle open, or one stuck in its own
five second read timeout - and waiting longer would not help. `--no-wait` is for a script
that would rather retry on its own terms.

How the turn is held differs:

- **Linux**: an advisory `flock(LOCK_EX)` on the board's `/dev/hidrawN`, taken on a second
  file opened only for that. The node is found through sysfs, the same way the HID library
  finds the board. The kernel releases the lock when the process ends, however it ends. The
  lock orders the instances of this program, and programs built on its library, among
  themselves - nothing else asks for it, see [Linux](#linux).
- **FreeBSD**: the open itself, since hidraw allows one at a time. That holds back every
  program that opens the node, see [FreeBSD](#freebsd).
- **macOS**: the board is opened exclusively (seized, `Access::Exclusive` of the transport,
  from sidit77/async-hid#55, pinned until it is released). While one handle holds the board,
  every other open fails, in this process and in any other, and is repeated until the time is
  up; the message then reads `another program holds it exclusively`, since the system does
  not say who. Closing the handle, or the end of the process, releases the board. The seize
  also holds back programs that do not know about this one, and a program that already had
  the board open loses its writes while the seize lasts.

  What the transport does under a seize was measured in sidit77/async-hid#55 (2026-10-11,
  YKUSH3 on Apple silicon). With this program, measured on 2026-11-02 on a Mac (Apple
  silicon) with board `Y3N13808`, release builds of the commit before the exclusive open and
  of the exclusive open:

  | Run | Shared open (before) | Exclusive open |
  |---|---|---|
  | two loops reading ports 1 and 2 at the same time, 2 x 500 | 496 failed, each an answer meant for the other loop (exit 76) | none failed |
  | the same, 2 x 2540 | - | none failed in 5080 runs |
  | the same with `--no-wait`, 2 x 300 | - | 300 succeeded, 300 found the board in use (exit 75), nothing else |

  The hardware test `a_held_board_is_not_opened_a_second_time` passes on macOS, and so do the
  other hardware tests that do not switch a port; the switching test was left out because a
  board hung on one of the ports.

  Before the exclusive open, measured on 2026-10-11: two loops asking for ports 1 and 2 at
  the same time failed in 2534 of 5073 runs, each failure an answer meant for the other loop.
  A status read notices this, because the answer names the port and is checked against the
  request. A switching command does not reliably notice it: identical commands are echoed
  identically, so an answer taken from another process looks like one's own.

Boards with different serial numbers do not wait for each other.

## The hardware

Every figure below is from the vendor datasheet, see [Documents](#documents-and-sources).

### Layout

The YKUSH3 is a USB hub with power switches in front of its ports and a microcontroller to
drive them. Plugging it in makes **three** devices appear on the host: a USB 3.2 Gen1 hub, a
USB 2.0 hub, and the controller as a USB HID device, `04D8:F11B`. The controller sits on the
bus permanently and cannot be switched off, which is why the board still answers when every
port is dead.

Ports can be switched three ways: over **USB**, which is always active, over **GPIO**, and
over **I2C**. The latter two are disabled from the factory and have to be enabled once over
USB; that setting survives resets and power cuts.

### Downstream ports

| Item | Value |
|---|---|
| Count | 3, individually switched, USB 3.0 type A |
| What gets switched | VBUS **and** the data lines, SuperSpeed as well as USB 2.0 |
| Hub core | USB 3.2 Gen1 (5 Gbps) plus USB 2.0 (480/12/1.5 Mbps) |
| Battery charging | BC1.2: DCP, CDP (1.5 A with data), SDP (0.5 A with data) |

Because the data lines are cut as well, a device disappears from the bus completely, even a
self-powered one. Switching off is the same as pulling the plug; switching on triggers a
fresh enumeration.

### Switched 5V output

| Item | Value |
|---|---|
| Connector | two pin breakout, 3.24 mm pitch, (+) is +5V, (-) is GND |
| Max continuous current | 6 A |
| On resistance | 10 mOhm |
| Switch | the same type as on the downstream ports |

No USB, power only. The output voltage sags with the load current in proportion to R_ON.

### GPIO

| Item | Value |
|---|---|
| Count | 3 |
| Output levels | high about 3.3 V, low about 0 V |
| Buffers | GPIO 1 and 2 are TTL, GPIO 3 is a Schmitt trigger |
| Input thresholds, TTL | low 0.50 V max, high 1.63 V min |
| Input thresholds, Schmitt | low 0.6 V max, high 2.64 V min |
| Max current per pin | 25 mA, sourcing and sinking |
| Absolute limit per pin | -0.3 V to 3.6 V |

Two modes: as a **control interface**, where the pins become inputs and the like-numbered
downstream port follows the level on the pin, or as a **GPIO/USB bridge**, where the host
uses the pins as digital I/O. `--gpio enable|disable` switches between them, taking effect
after the next reset.

### I2C

| Item | Value |
|---|---|
| Lines | SDA on pin SDI, SCL on pin SCK, pull-ups on the board |
| Factory address of the board | 7 bit `0b1010011` = `0x53` |
| Address format in the protocol | 7 bit address aligned to the top of the byte, LSB zero - the address shifted left by one |
| Slave mode | the board becomes a control interface an outside master can switch |
| Master mode | the board becomes a USB-to-I2C bridge |
| Payload per transfer | 60 bytes at most |
| Factory state | both modes off, and the setting is persistent |
| `--i2c-master enable` | makes the board re-enumerate; unreachable for about a second |
| `--i2c-master disable` | is acknowledged but often does not take - see [Checked against the hardware](#checked-against-the-hardware) |
| Missing ACK from a slave | is **not** reported: a read returns `0xff`, a write reports either success or a transmission error |

Command bytes an outside master sends to the YKUSH3 in **slave mode**:

| Byte | Effect | Byte | Effect |
|---|---|---|---|
| `0x03` | port 1 on | `0x07` | port 1 off |
| `0x04` | port 2 on | `0x08` | port 2 off |
| `0x05` | port 3 on | `0x09` | port 3 off |
| `0x0B` | all ports on | `0x0C` | all ports off |

### Other connectors

Broken out but unused by `ykushcmd`: **UART** (RX/DT, TX/CK) and **SPI** (SS, SDO, SDI,
SCK), plus an **ICSP** header for programming firmware and bootloader. The factory firmware
has no commands for UART or SPI.

### Power and operating conditions

| Item | Value |
|---|---|
| Supply modes | bus powered or self powered, selected by a jumper |
| External supply | 5.00-5.25 V through a screw terminal, 3.54 mm pitch |
| Operating temperature | -40 to +85 degC |
| Absolute limit on the +PWR 5V pin | -0.3 V to +6 V |

Under a heavy load - several hard drives - self powered with an adequate supply is the way
to go; the drop across the power switches grows with the current.

### Hardware revision 1.3.0

A drop-in replacement with no change to features or layout. New: independent switching
sub-circuits per port, an extra GND pin on the GPIO header, better conditioning of the data
lines. Requires firmware 1.5.0 or newer.

## USB control protocol

The host is the master and the board answers. Command and answer are 64 bytes each. Byte 0
of an answer is `0x01` on success and `0x00` on failure.

| Opcode | Command | Layout | Answer |
|---|---|---|---|
| `0x01`-`0x03`, `0x0A` | port 1-3 / all off | `[op]` | `0x01`, echo |
| `0x11`-`0x13`, `0x1A` | port 1-3 / all on | `[op]` | `0x01`, echo |
| `0x21`-`0x23` | port state | `[op]` | byte 1: high nibble on/off, low nibble port number |
| `0x04` / `0x14` / `0x24` | 5V output off / on / state | `[op]` | `0x01`, byte 1 is `0x04`/`0x14` |
| `0x30` | read GPIO | `[op, pin]` | byte 3 is the level |
| `0x31` | write GPIO | `[op, pin, value]` | echo |
| `0x32` | GPIO control interface | `[op, 0/1]` | `0x01, 0x32, action` |
| `0x41` | power-on state | `[op, port, state]` | `0x01, 0x41, port, state` |
| `0x42` | enter bootloader | `[op]` | none, the board restarts |
| `0x51` | I2C configuration | `[op, action, value]` | `0x01` |
| `0x52` | I2C write/read | `[op, action, addr, n, data...]` | `status, 0x52, n, data...` |
| `0x55` | reset | `[op]` | none |
| `0x61` | version | `[op, 0x01\|0x02]` | `0x01, 0x61, major, minor, patch` |

`0x51` actions: `0x01` slave mode, `0x02` master mode, `0x03` set own address.
`0x52` actions: `0x01` write, `0x02` read. Status values for `0x52`: `0x01` success,
`0x02` the board is not in master mode, `0x03` a transmission error on the bus; `0x00`
is the general failure status, as for every command.

The table was derived from the C++ original and checked against the vendor reference. Two
things `ykushcmd` uses are **not** documented anywhere: `-c` with port `e`, the external 5V
output, and the configuration value `2` for the last state. Neither appears in datasheet
v1.2.1 or in the online reference, yet both work - see
[Checked against the hardware](#checked-against-the-hardware).

## How it is put together

The package builds two targets: the `ykush3` **library**, which holds everything that
touches the board, and the `ykushctrl` **binary**, a command line front end on top of it.

| File | Target | Contents |
|---|---|---|
| `src/lib.rs` | library | the public surface, everything re-exported at the root |
| `src/device.rs` | library | the `Transport` trait and `Board`, its HID implementation |
| `src/lock.rs` | library | taking turns at a board: the wait, and the `flock` on Linux |
| `src/ykush3.rs` | library | the protocol: opcodes, reports, reading the answers |
| `src/fake.rs` | library | `FakeBoard`, the test double for the transport, behind the `fake` feature |
| `src/sanitize.rs` | library | the control character filter for text from outside |
| `src/error.rs` | library | the error type all layers share |
| `src/main.rs` | binary | entry point, running a `Command`, output |
| `src/cli.rs` | binary | argument splitting, value checking, the `Command` enum |
| `src/help.rs` | binary | help text |

The layers are separated so that each one can be exercised without hardware:

```
arguments --cli::parse--> Invocation{serial, Command} --execute--> Ykush3<T: Transport>
                                                                        |
                                                     Board (async-hid) -+- FakeBoard (test)
```

`Ykush3<T>` is generic over the transport and `Ykush3::open()` gives the variant backed by a
real HID device. `execute()` writes into an `impl Write`, so the output can be checked too.

### Scripting the board from Rust

A Rust program can depend on the library and skip the command line entirely:

```rust
use ykush::{Port, Ykush3};

let board = Ykush3::open(None)?;            // or Some("Y3N13808")
board.port_down(Port::Downstream(2))?;
println!("{}", board.port_status(Port::Downstream(2))?);
```

**Open the board once and keep it.** A rig that opens per operation still works, but every
open costs about 17 kB that the process does not give back, measured over 25 000 openings
in ten minutes. With one handle held instead, 88 000 exchanges over three minutes left the
process flat at 8 MB. Opening is also the slow part of an exchange, so holding the handle
is faster as well.

The 17 kB are not a leak and not this library's doing. `leaks` reports nothing lost, and
`heap` says why: after 2 000 openings the process holds 4 000 `NSMutableData` objects of
8 256 bytes each, plus one `CFString` per open, all of them owned by Foundation and Core
Foundation. That is two report sized buffers per open which IOKit keeps rather than
releases on close. hidapi grows by the same amount per open, because the cost sits below
both of them, where neither can reach it.

`Ykush3::open` and `Board::open` wait up to `DEFAULT_WAIT`, five seconds, for a board
another process is using, and fail with `Error::Busy` after that; `open_within` takes the
limit, and a limit of zero fails at once. On Linux a held `Board` keeps its lock for as long
as it lives, so a long running program makes every `ykushctrl` invocation on that board
wait - or fail, if it holds on for longer than they wait.

`FakeBoard` ships with the library, so code built on it can be tested the same way this
repository tests itself, against a prepared answer instead of a board. It sits behind the
`fake` feature, because its inspection methods panic on misuse as test helpers should; a
build that does not ask for the double gets a library without test gear:

```toml
[dev-dependencies]
ykushctrl = { path = "../ykush-rs", features = ["fake"] }
```

```rust
use ykush::{fake::FakeBoard, Port, Ykush3};

let board = Ykush3::with_transport(FakeBoard::answering(&[0x01]));
board.port_up(Port::Downstream(1))?;
assert_eq!(board.transport().sent_count(), 1);
```

The library is not on crates.io and makes no stability promise yet; it moves with the
command line application.

## Tests

```
cargo test                                    # 162 tests and a doctest, no
                                              # hardware needed (167 on Linux)
cargo test -- --ignored --test-threads=1      # 11 more: 9 need a board, 2 only
                                              # the HID stack of the system;
                                              # on Linux 3 more that need a board
```

Every push runs the first line on macOS and on Linux, along with `cargo fmt --check`, `cargo clippy`
with warnings denied, and a build against the `rust-version` from `Cargo.toml` so that
figure stays honest.

The second run **must** be single threaded. On Linux a second open of the board is not
refused and each answer reaches every open handle, so tests running side by side would read
each other's answers; on macOS the exclusive open makes them wait for each other and run
into the five second limit.

| Group | Tests | Subject |
|---|---|---|
| `cli` | 36 | grammar, board selection, `--no-wait`, every command, every error message |
| `ykush3` | 42 | the bytes each command sends, reading answers, error paths, a swept and a hand-picked pass over generated answers |
| `main` | 15 | output format, dispatch of every command, program name |
| `error`, `help`, `device`, `sanitize` | 40 | error texts, exit codes, help output, report padding, the Linux permission and driver diagnostics, the hidraw node from sysfs, the character filter |
| `lock` | 16, on Linux 21 | two invocations hold the board one after the other, waiting runs out at the limit, `--no-wait` makes one attempt, a failed command lets go, a board gone for a moment is waited for, a node that moves while it is locked is let go; on Linux the same against a real `flock` on a temporary file |
| `tests/cli.rs` | 11 | the built binary: exit codes, stdout versus stderr |
| `tests/encoding.rs` | 6 | every file is valid UTF-8 within Latin-1, and the rule itself |
| doctest | 1 | the library example compiles; `no_run`, since it needs a board |
| **running without hardware** | **166 + 1**, on Linux **171 + 1** | |
| `--ignored`, needs a board | 9, on Linux 12 | opening and real exchanges, the acknowledged switch, the echoed pin and port, 2000 status reads on one handle, a second open of a held board refused as busy; on Linux the built binary against a lock held by another process, with and without `--no-wait` |
| `--ignored`, needs only the HID stack | 2 | enumeration when nothing is attached |

The long one among the board tests reads port states 2000 times over one handle and checks
that every answer comes within the read timeout. It only reads, since the ports may power
real devices. `YKUSH3_SOAK_COUNT` sets another number, and `--nocapture` shows the count and
the slowest answer.

The `FakeBoard` transport records what a command sends and feeds prepared answers back.
That pins the wire bytes of every command against the vendor reference. A `FakeBoard::mute()`
with no answer prepared fails the test as soon as anything reads from it, which is how
`--reset` and `--boot` are held to not waiting for an answer.

### Coverage

Line coverage with `-C instrument-coverage`, measured on 2026-10-31 on macOS on Apple silicon
at commit 83c3327, before the exclusive open on macOS. The Linux-only code is not compiled
there and not counted.

| Run | Lines | Missed |
|---|---|---|
| `cargo test` only, no board needed | 90.36 % | 215 of 2231 |
| `cargo test` and the hardware run, board `Y3N13808` on firmware 1.5.0 attached | 96.91 % | 69 of 2231 |

`cli.rs`, `error.rs`, `fake.rs` and `sanitize.rs` are at 100 % in both runs. Without a board
most missed lines are in `device.rs` (143), the transport that needs a device. With the board,
what stays unreached is:

| Place | Why it is not reached |
|---|---|
| `device.rs` - refused open | needs another program holding the device exclusively, or a vanishing board |
| `device.rs` - `Error::NoResponse`, a failing read, discarding stale answers | needs a device that goes quiet for five seconds or reports a read error |
| `device.rs` - truncated read | needs a device that delivers only part of a report |
| `main.rs` - `unreachable!()` | help, version and listing are handled before a board is opened |
| `lock.rs` - the error of a failed lock | needs the operating system to refuse the lock itself |
| `device.rs`, `main.rs` - the hint for an empty list | with a board attached the list is never empty; on macOS the hint is always empty |
| `help.rs` - a failing write of the usage text | needs standard output to fail |
| the rest, about 15 lines | failure arms of assertions in tests, and the untaken half of the test that re-applies the port state it finds |

Reproducing the measurement, with the `llvm-tools` component of the toolchain
(`rustup component add llvm-tools`; another LLVM may not read the profiles of this rustc) and
without `cargo-llvm-cov`:

```
export RUSTFLAGS="-C instrument-coverage" LLVM_PROFILE_FILE="/tmp/yk/%p-%m.profraw"
cargo test && cargo test --no-fail-fast -- --ignored --test-threads=1
llvm-profdata merge -sparse /tmp/yk/*.profraw -o /tmp/yk/yk.profdata
llvm-cov report -object target/debug/deps/<every test binary> \
                -object target/debug/ykushctrl \
                -instr-profile=/tmp/yk/yk.profdata \
                -ignore-filename-regex='(\.cargo/|\.rustup/|/rustc/|/tests/)'
```

`--no-fail-fast` matters without a board: the library's hardware tests fail first, and
without the flag cargo would stop before the binary's HID stack test has run. The built
binary has to be the last object, otherwise `main()` is missing from the figures.

## Checked against the hardware

The runs below predate the exit codes of 0.5.0: where they say "exits 1", today's program
exits with the code from [Exit status](#exit-status), `69` for an unknown serial number and
`75` for a held board.

Against a YKUSH3 with serial number `Y3N13808`, firmware 1.5.0, bootloader 1.2.0, on macOS
running on Apple silicon. Nothing was attached to the downstream ports.

The answer checking added after the first run of this record was validated against the same
board on 2026-08-16, still on firmware 1.5.0, in two rounds: first the ACK the board sets
on a switching command, then the answer-to-request correlation - the echoed GPIO pin and
the external port nibble. All nine hardware tests pass; the firmware really does echo what
the stricter checks require.

| Area | What was run | Result |
|---|---|---|
| Test suite | `cargo test -- --ignored --test-threads=1` | 9 hardware tests pass |
| Enumeration | `-l`, `-s <serial>`, an unknown serial | correct, unknown serial exits 1 |
| Versions | firmware, bootloader | 1.5.0 / 1.2.0 |
| Port switching | `-d`/`-u` singly and with `a`, each read back with `-g` | correct |
| 5V output | `-on`/`-off` as well as `-u 4`/`-d 4` | both spellings correct |
| Port state | `-g 1` through `-g 4` | correct, the 5V port included |
| GPIO | `-w`/`-r` on pins 1 and 3, both levels | correct |
| Power-on state | `-c`, verified across real resets | takes effect and persists |
| I2C | master, slave, address, error paths | see below |
| Exit codes | 0 on success, 1 on every error path | correct |

On 2026-11-02, with the version query that gets an invocation in step, against the same
board on macOS:

| Run | Result |
|---|---|
| `cargo test -- --ignored --test-threads=1` | the 9 hardware tests and the 2 that need the HID stack pass |
| `-g 1`, 20 times, release builds | median 11.0 ms before the change, 74.4 ms after it |
| two loops of `-g 1` and `-g 2` in parallel, 200 each | none failed |
| a status query for port 1 written by a separate program that exits without reading, then `-g 2`, 3 times | `Port 2: off`, exit 0, before the change and after it |

The last row is the case the version query is for. On macOS the answer left behind did not
reach the next invocation in these runs, so they show nothing there; on Linux it has not been
tried.

Not exercised: `--boot`. The board would stay in its bootloader and only the firmware update
tool would get it back.

### On Linux

The same board on 2026-10-11, from a Raspberry Pi 4 under Debian 13 (kernel 6.18, aarch64),
with an ST-Link debug probe on each of the three downstream ports and a udev rule that gave
`plugdev` the hidraw node and also the USB device. The switching and the comparison with
`ykushcmd` ran on an early state of the Linux support, before `-l` kept succeeding and named
every detached board, both test suites again on the state that added it:

| Area | What was run | Result |
|---|---|---|
| Test suite | `cargo test` and `cargo test -- --ignored --test-threads=1` | 116 + 1 and 9 hardware tests pass |
| Enumeration | `-l`, `-s <serial>`, an unknown serial | correct, serial read from `HID_UNIQ`, unknown serial exits 1 |
| Port state | `-g 1` through `-g 3`, compared with `ykushcmd ykush3 -g` | the same states |
| Port switching | `-d`, then `-u`, on each port in turn, checked with `lsusb` | the probe on that port, and only that one, left the bus and came back within about a second |
| Detached driver | `ykushcmd` run first, then this program | reported with the interface name; binding `usbhid` again restores access |

Taking turns, on the same Pi and board on 2026-10-11, every step read-only. The runs were
taken on an earlier state of the change, which differs from the final one only in not
looking the node up again after locking; `cargo test` passed on the final state as well,
but the board had moved to the Mac before the hardware runs could be repeated:

| Area | What was run | Result |
|---|---|---|
| Test suite | `cargo test` and the ignored tests except the one that re-applies a port state | 139 + 1 and 9 pass |
| Held handle | 2000 status reads on one handle | all answered, slowest 4.0 ms, 6.0 s in all |
| Two loops of `-g 1` | in parallel for 30 s | 1190 and 1220 runs, no error, no wrong answer |
| `-g 1` against `-g 2` | in parallel for 30 s | 1298 and 1227 runs, no error, no wrong answer |
| The same, previous build | in parallel for 15 s | 225 errors in 450 runs, each an answer describing the other port |
| A held lock | `flock /dev/hidraw0 sleep 8` beside `-g 1` | waits 5.0 s and exits 1; with `--no-wait` exits 1 after 10 ms |

The comparison against `-g 2` is the one that tells: two loops asking about the same port get
byte-identical answers, so a crossed answer would go unnoticed there.

The same board on a Mac on 2026-10-11, on this change before the held-board test was marked
as expected to fail there, every step read-only:

| Area | What was run | Result |
|---|---|---|
| Test suite | the ignored tests except the one that re-applies a port state | 9 pass, `a_held_board_is_not_opened_a_second_time` fails: the second open succeeds |
| Held handle | 2000 status reads on one handle | all answered, slowest 3.0 ms, 4.0 s in all |
| `-g 1` against `-g 2` | in parallel for 30 s | 2536 and 2537 runs, 1260 and 1274 errors, each an answer describing the other port |

What turned up beyond what the documentation says:

- **The undocumented configuration values work.** `-c e 1` sets the 5V output to come up by
  default, and after a reset it was on. Persistent mode `-c 1 2` holds in both directions:
  a port that was off before the reset stayed off, one that was on stayed on.
- **Changing I2C mode makes the board re-enumerate.** After `--i2c-master enable` it was
  unreachable for about a second. Scripts have to wait, or the commands that follow go
  nowhere.
- **`--i2c-master disable` cannot be relied on.** The board acknowledges the command and
  stays in master mode anyway - after a `--reset` as well, after sending it twice, and after
  an enable/disable cycle. The mode reproducibly went off only after this sequence:
  `--i2c-slave enable` -> `--i2c-set-address` -> `--i2c-slave disable` ->
  `--i2c-master disable` -> `--reset`. Which part of it does the work is open; presumably
  cycling slave mode re-initialises the same MSSP unit. Since the setting is persistent,
  only turn master mode on when it is meant to stay on.
- **A missing ACK from an I2C slave is not reported.** On an empty bus `--i2c-read` always
  returns `255` (`0xff`, the idle level of the bus). `--i2c-write` reports success
  sometimes and status `0x03` other times, neither of them because a slave answered. So the
  return value says nothing about whether a device is on the bus. Only the reverse holds:
  status `0x02` definitely means the board is not in master mode.
- **A bus scan across all 7 bit addresses** (`0x08`-`0x77`) ran through without a single
  answer with nothing on the I2C header, so the method is sound for finding a device once
  one is there.
- **The board could only be open once** on macOS with the earlier `hidapi` transport. A
  second attempt failed with "exclusive access and device already open". The transport
  that followed opened shared, and a second open succeeded with both handles receiving
  every answer. Since the exclusive open of the current transport a second open fails
  again, measured on 2026-11-02, see [Concurrent invocations](#concurrent-invocations).
  Linux does not refuse the second open, see [Linux](#linux).
- **The connector is the most common fault.** When the USB 3.0 micro-B plug is not seated
  fully, only the SuperSpeed hub appears; the USB 2.0 hub and the controller are missing and
  the board cannot be reached even though the power LED is lit. Check the plug before
  swapping the cable.

### On FreeBSD

The same board on 2026-11-03, from FreeBSD 15.1 (aarch64) in a VMware Fusion VM on a Mac: the
9 hardware tests and the 2 that need only the HID stack pass; two loops reading ports 1 and 2
in parallel, 300 each, did not fail; with the board held by another process, `--no-wait`
exits `75`, a waiting call goes through once it is free, and `-l` still lists the board;
without access to the node, a call exits `77` at once.

## Where it differs from the C++ original

| Point | Original | Here |
|---|---|---|
| `-r <gpio>` | prints nothing, the value only reaches the exit code | prints the value on stdout, exit code `0` |
| `-g 4` | the answer is not decoded, returns `-1` | the external port is decoded, which tables 6 and 7 of the datasheet say is right |
| `--i2c-read <n>` with n >= 10 | `dec2bin()` writes one byte per digit and corrupts the report | the length goes out as a single number |
| hex values without `0x` | are misread | accepted with and without the prefix |
| an invalid port number | help with no explanation | a specific error message |
| `--reset`, `--boot` | wait for an answer the board no longer sends | send without reading |
| `-s` after the command | is ignored | works wherever it appears |
| `-s` given twice | the last one wins silently | rejected as ambiguous |
| a stray first word (`ykushcmd foo -u 1`) | silently skipped | rejected with a message |
| the external port | `4` for switching, `e` for configuring | both spellings everywhere |
| I2C error status | only `0x00`/`0x01` told apart, otherwise a silent `0` | `0x02` and `0x03` are reported in words |
| a command the board rejects | still exits with `0`, the answer is never read | the ACK is checked, exit `69`; an answer that does not fit, `76` |
| an answer shorter than 64 bytes | missing bytes read as zeros | rejected as truncated |
| a garbled version answer | reported as the legacy version `1.0.0` | an error instead of a guess |
| a status answer for the wrong port or pin | taken at face value | rejected - the answer must match the request |
| an I2C answer claiming more bytes than asked | clamped and returned | rejected |
| messages | - | worded independently, see [License](#license) |

## Driving a YKUR relay board over I2C

The YKUR, Yepkit's USB relay board, has an I2C control interface besides USB and can
therefore be switched by a YKUSH3 in master mode. Both datasheets say so.

**What a YKUR is:** one on-board relay, rated 250 Vac / 30 Vdc at 10 A, plus four driver
outputs for the coils of external relays. Each output handles up to 360 mA at 5 V, but **the
board as a whole is limited to 500 mA**, so four hefty coils at once is not on.

**Addressing:** the YKUR sits at the fixed 7 bit address `0b1010001` = `0x51`. The YKUSH3
wants the address aligned to the top of the byte, so `0x51 << 1 = 0xA2`. No clash with the
YKUSH3's own factory address `0x53`, and it is the master here anyway.

**The command:** a single data byte written to `0xA2`.

| Byte | Effect | Byte | Effect |
|---|---|---|---|
| `0x01` | relay on | `0x02` | relay off |
| `0x03`-`0x06` | port 1-4 on | `0x07`-`0x0A` | port 1-4 off |
| `0x0B` | all ports on | `0x0C` | all ports off |
| `0x0D` | all ports and the relay on | `0x0E` | all ports and the relay off |
| `0x0F` | read the relay state | `0x1F`/`0x2F`/`0x3F`/`0x4F` | read the state of port 1-4 |

Which gives:

```
ykushctrl --i2c-master enable       # once, and it survives a reset
ykushctrl --i2c-write 0xA2 0x01     # on-board relay on
ykushctrl --i2c-write 0xA2 0x02     # on-board relay off
ykushctrl --i2c-write 0xA2 0x03     # external relay 1 on
ykushctrl --i2c-write 0xA2 0x0E     # everything off
```

Worth knowing before wiring it up:

- **Measure the levels first.** The YKUSH3 pins take 3.6 V absolute maximum. Whether the
  YKUR pulls its I2C bus to 3.3 V or to 5 V is in neither datasheet. Measure at the YKUR I2C
  pins with the bus idle; at 5 V a level shifter is needed.
- **Both boards carry their own pull-ups.** In parallel that halves the resistance - usually
  harmless, but remove one set if the bus misbehaves.
- **Tie the grounds together.** I2C needs a common reference.
- **The YKUR needs its own supply.** Two I2C wires do not feed it. The obvious arrangement
  is to run the YKUSH3's switched 5V output, good for 6 A, into the YKUR's external supply
  input - which also lets `ykushctrl -on`/`-off` cut the YKUR's power entirely.
- **The exit code tells you nothing.** Verified on hardware: on an empty bus `--i2c-write`
  reports success and `--i2c-read` returns `255`. Whether the YKUR answers at all shows only
  in the content of a reply that cannot be `255` - or simply in whether the relay clicks.
- **Reading state is untested.** Switching is documented. Whether the query bytes (`0x0F`
  and relatives) work across the bridge - `--i2c-write` with the query byte followed by
  `--i2c-read 0xA2 1` - is in no datasheet, because only the write sequence is described
  there. It has to be tried on the YKUR.
- **Wait after changing mode.** `--i2c-master enable` makes the board re-enumerate and the
  commands that follow fail for about a second. The command itself reports a read error even
  though it was carried out.
- **`enable` is effectively a one-time action.** The mode survives resets and power cuts, and
  turning it off again is awkward - see
  [Checked against the hardware](#checked-against-the-hardware). For running a YKUR that is
  no drawback; the mode is meant to stay on.
- **Firmware:** the YKUR only gained its I2C control interface in revision 1.2.1.

The simpler route, if a PC is standing there anyway: hang the YKUR off a downstream port of
the YKUSH3 over USB and drive it with `ykurcmd`. The I2C route earns its keep when the relay
has to be switched without a second USB device, or from a microcontroller.

## Documents and sources

The vendor documentation is **not** in this repository. It belongs to Yepkit and is not
under the Apache license; redistributing it would be a separate question to ask them. Every
source, with the date it was retrieved, is listed in [`doc/README.md`](doc/README.md).

The main ones:

- [YKUSH3 datasheet](https://www.yepkit.com/uploads/documents/9f39a_ykush3-datasheet.pdf)
- [YKUSH3 USB control interface](https://ykushboards.yepkit.com/docs/ykush3/reference/usb/)
- [YKUSH3 I2C](https://ykushboards.yepkit.com/docs/ykush3/reference/i2c/)
- [YKUR datasheet](https://www.yepkit.com/uploads/documents/bb69a_YKUR_datasheet_Rev.1.2.1.pdf)
- [ykushcmd on GitHub](https://github.com/Yepkit/ykush) - Yepkit's own C++ application,
  the reference this was compared against

## License

Apache License 2.0, full text in [`LICENSE`](LICENSE). Section 5 of the license governs
contributions, see [`CONTRIBUTING.md`](CONTRIBUTING.md).

The protocol comes from Yepkit's **published documentation** - the USB control interface
and I2C references listed above - and not from the source of their `ykushcmd`. What this
program shares with that one is the protocol both must speak because the hardware requires
it, and the names of the command line switches. Both are facts and interfaces, and neither
is what copyright protects.

No source, no help text and no message of the original is present. Help and output are
written from scratch; that was checked against every string literal in `ykush_help.cpp`,
`ykush3.cpp` and `yk_usb_device.cpp` - nothing overlaps.

The choice of the Apache license is therefore freely made rather than inherited. The credit
in [`NOTICE`](NOTICE) is there voluntarily, because the protocol would not exist without
Yepkit's work. Yepkit has neither endorsed nor reviewed this and does not support it.

No third party C library is vendored or linked, so nothing has to accompany a binary
release beyond this. Every crate in the binary is under MIT or Apache-2.0 terms and listed
in [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md).
