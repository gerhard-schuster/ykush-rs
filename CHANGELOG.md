# Changelog

Notable changes, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

Linux and FreeBSD on 64 bit Arm join macOS on Apple silicon as supported
systems, Linux run against a board on a Raspberry Pi 4 under Debian 13, FreeBSD
15.1 in a VM. The protocol and the report exchange are the same on all three;
what differs is how the device is reached and who may open it. On FreeBSD the
open itself is the turn, since hidraw allows one at a time, and a board another
program has open is found through `sysctl`.

### Changed

- **Incompatible:** the exit status tells what to do next instead of being
  `1` for every error, after sysexits(3): `64` invalid command line, `69` no
  board, board unreachable or the board refused the command, `74` no answer
  or transport error, `75` board in use, `76` an answer that does not fit
  the request, `77` access denied. A script
  that checks for a non-zero status keeps working; one that checks for `1`
  does not. The library gains `Error::exit_code()` and the variants
  `Unavailable`, `PermissionDenied` and `Rejected`; a refusal (status
  `0x00`) and an answer that does not fit the request are now told apart.
- macOS: the board is opened exclusively (seized), so a second invocation,
  or any other program, cannot open it at the same time; it waits up to 5
  seconds like on Linux, or fails at once with `--no-wait`. Needs
  `Access::Exclusive` from sidit77/async-hid#55; `async-hid` is pinned to
  that revision on the fork until it is released. Measured on a Mac with a
  board: two parallel loops failed in none of 5080 runs, against about half
  before. The pin also moves the Linux transport from 0.5.3 to
  0.6.0 of the fork, not yet run on the Raspberry Pi.
- Renamed: the program is `ykushctrl`, the package `ykushctrl`, the library
  `ykush` (was `ykush3cmd` and `ykush3`), and the repository `ykush-rs`. The
  board it supports is still the YKUSH3 only. The udev rule file is
  `60-ykush.rules`.

### Added

- Linux on aarch64 is a supported target. The transport needs no change: the
  pinned `async-hid` revision drives the board through hidraw, finds it by
  reading `/sys`, and needs neither `libudev` nor `pkg-config` to build. The
  macOS write path of that revision is untouched. Linux on x86_64 still
  refuses to compile, because no board has been driven from it.
- A refused open on Linux names the udev rule that grants access,
  `/etc/udev/rules.d/60-ykush.rules`, instead of a bare "Permission denied".
  The README has the rule.
- A board that is on the USB bus but has no HID driver bound - the state a
  libusb based program such as `ykushcmd` leaves it in - is reported as such,
  with the interface names of every such board and how to bind the driver
  again, instead of as a board that is not attached. A command that opens a
  board fails with that message. `-l` keeps its contract: it prints the empty
  list and exits 0, and the hint goes to stderr.
- `ykush::detached_hint()` in the library: the explanation for YKUSH3 boards
  on the bus whose HID driver is detached, or `None`. Always `None` outside
  Linux.
- CI builds, lints and tests on Linux (arm64) as well as on macOS.
- Concurrent invocations take turns at a board. The second one waits until
  the first is done, up to five seconds, and then fails with exit status 75
  and a message naming the board. On Linux the turn is an advisory `flock`
  on the board's hidraw node, which the kernel releases when the process
  ends; no lock file is involved. Before this, two invocations on Linux could
  read each other's answers: two loops asking for ports 1 and 2 in parallel
  failed in 225 of 450 runs, and none fail now.
- On macOS, before the exclusive open (see above), concurrent invocations
  were not serialized: the same two loops failed in 2534 of 5073 runs.
- `--no-wait`: fail at once when the board is in use, instead of waiting.
- `Board::open_within()` and `Ykush3::open_within()` take the time to wait,
  `DEFAULT_WAIT` is the five seconds `open()` waits, and `Error::Busy` is
  what both return when the time is up.
- On Linux, the lock serializes `ykushctrl` instances only. Another tool on
  the same board, Yepkit's `ykushcmd` among them, is not held back; README and
  `SECURITY.md` say so and recommend one YKUSH3 tool per host.
- A hardware test that reads port states 2000 times over one held handle,
  read-only; `YKUSH3_SOAK_COUNT` changes the number.

### Changed

- **Breaking, so the next release is 0.5.0.** `Error` has a new variant,
  `Busy`, and is now `#[non_exhaustive]`, so a `match` on it outside this
  crate needs a catch-all arm; later variants will not break it again.
  `Ykush3::open()` and `Board::open()` now wait up to five seconds for a
  board another process is using, where they used to fail or share it at
  once; `open_within(serial, Duration::ZERO)` is the old immediate answer.
- The release profile turns on link time optimisation and a single codegen
  unit. The binary goes from 557 to 407 kB, a third less, and a clean build
  takes about a second longer. Neither setting changes behaviour.
  `opt-level` stays at `"s"`: `"z"` measured 320 bytes larger here, because it
  switches off optimisations that shrink this code.
- The transport follows its revision on the fork, which moved when
  sidit77/async-hid#50 was merged and the branch was rebased onto it. The tree
  at the new revision is byte for byte the tree at the old one.

- An unknown option is named correctly when `-s` comes first: `-s A -x`
  used to report `-s` as the unknown option.

- Before its first command, and after an exchange that failed or got an
  answer that does not fit, the program asks for the firmware version,
  drops every report before its answer and waits 50 ms for the queue to
  stay quiet. An answer left behind by a killed
  invocation is no longer read as the answer to the next one, which for a
  switching command passed unnoticed, and a late answer that comes before
  the answer to the version query is dropped, not only one within 50 ms.
  `Transport` gains `desync()`, with a default that does nothing, and
  `FakeBoard` counts the calls in `desyncs()`. On macOS writes give up after 5
  seconds like reads. `sanitize()` lets only printable ASCII through, and
  U+FFFD. Found by the security review of ykur-rs, which shares this code;
  see the addendum in `SECURITY-REVIEW-0.4.0.md`.
- `tests/encoding.rs` holds every file in the repository to valid UTF-8 within
  Latin-1: no character above U+00FF, and none of the Latin-1 characters that
  are invisible or steer a terminal. Umlauts are fine, typography is not. The
  rule has six tests of its own.

## 0.4.1 - 2026-09-24

0.4.0 declares a minimum supported Rust it cannot be built with. The code is
fine; the number was stale. This corrects it and changes nothing else.

### Changed

- The minimum supported Rust is 1.86, up from 1.74. Two things push it there,
  both in the dependency and neither in this crate: `async-hid` is edition
  2024, which no compiler before 1.85 accepts, and it calls `Once::wait`, which
  is stable since 1.86. The figure moves back down if the dependency ever does.

## 0.4.0 - 2026-09-24

The HID transport moves from `hidapi`, which carries a C library and compiles
it, to `async-hid`, which reaches IOKit from Rust. Building needs a Rust
toolchain and nothing else - no C compiler, no Xcode command line tools. The
nine hardware tests were run against a board before and after the move.

Two findings from the review of that new transport are fixed, the account of
where the protocol came from is corrected, and the transport is pinned to a
revision of the fork instead of a directory on one machine, so a clone of this
repository builds on its own.

### Changed

- The transport is `async-hid`. The library is asynchronous because that is how
  macOS delivers HID reports; the `Transport` trait and everything above it stay
  synchronous, so the change is confined to `device.rs`.
- The read timeout of five seconds is now built by racing the read against a
  timer, since the new library offers no timed read. A read that loses the race
  leaves the device usable.
- No leading report id byte is prepended any more. The new library takes the
  bare 64 byte report, which suits a board that uses unnumbered reports.
- The documentation now states where the protocol came from. It was implemented
  from Yepkit's published USB control interface and I2C references and checked
  against a board, not taken from the source of the C++ `ykushcmd`; the README,
  `NOTICE`, `doc/README.md` and the crate documentation had described this
  program as a port of that source. `NOTICE` also records that the two are built
  differently - a library behind a substitutable transport here, a single
  executable whose board classes inherit their HID access there. No code
  changed; what was wrong was the account of how the code came to be.

### Security

- `SECURITY-REVIEW-0.4.0.md` reviews the state after the transport change; the
  earlier `SECURITY-REVIEW.md` is kept as the record of the `hidapi` state. No
  HIGH or MEDIUM finding, one LOW, and a second LOW that was withdrawn after a
  closer look - the review carries an addendum saying what changed and why the
  withdrawn one does not stand.
- An answer that arrives after a read has timed out is no longer read as the
  answer to the next command. A timeout marks the board, and the next exchange
  empties the queue of the transport before it sends. The drain is bounded by a
  50 ms wait and by 16 reports. This was reachable only through the library: one
  run of the command line program performs one exchange and exits.
- `sanitize()` now replaces the bidirectional overrides and the zero width
  characters as well as the control characters. They cannot drive a terminal,
  but `U+202E` in a serial number reorders the line it is printed on and
  `U+200B` prints as nothing, which together are enough to show one board's
  serial number as another's. `U+FFFD` is still kept, so a lossy `argv`
  conversion still shows where the bad byte was.
- The transport is pinned to a revision of the fork rather than patched to a
  directory next to this repository, so a clone of this repository builds on its
  own and `Cargo.lock` records a revision anyone can check. It was a
  `[patch.crates-io]` pointing at `../async-hid` before, which resolved to
  whatever happened to be on that disk. The pin goes away once the fix is
  released on crates.io; the open pull request is sidit77/async-hid#46.

### Removed

- `Error::HidInit` - it reported a failure of the C library's global
  initialisation, which no longer exists. Library callers matching on it have to
  drop the arm; the variant can never be constructed.
- `LICENSE-hidapi-bsd.txt` and the licensing apparatus around it. Nothing links
  a third party C library any more, so nothing has to accompany a binary release
  beyond the Apache license and the notices.

## 0.3.0 - 2026-08-16

The protocol layer now holds every answer to the shape of the request it
belongs to, and the command line rejects what it used to skip silently. The
differences table in the README lists every deliberate divergence from the
C++ application in one place.

### Changed

- The board's answers are held to the shape of the request. A port status
  must describe the port that was asked about and a GPIO read the pin that
  was asked about; a version answer is either the exact empty shape of a
  legacy board or a properly acknowledged version - a garbled answer is an
  error rather than a made-up `1.0.0`; an I2C read claiming more bytes than
  requested is rejected instead of clamped. Each of these was taken at face
  value before, as the C++ application does.
- GPIO numbers are validated in the library, not only in the command line.
- Only the literal word `ykush3` is accepted as a leading board name - any
  other free-standing first word is an error instead of being skipped - and
  a second `-s` on one line is rejected instead of the first silently
  winning.
- `report()` is no longer part of the library's public interface, and
  `Error::source()` hands out the underlying error of an output failure.

## 0.2.0 - 2026-08-16

Hardening after a security audit, and a library underneath the command line.
Scripts that only look at the exit status get stricter behaviour on purpose:
a command the board rejects now exits with 1 instead of passing silently.

### Added

- A library target `ykush3` underneath the command line application, so the
  board can be scripted from Rust directly. The `FakeBoard` test double ships
  with it behind the `fake` feature: code built on the library can be tested
  without hardware, the same way this repository tests itself, while a build
  that does not ask for the double gets a library without test gear or
  panicking paths.

### Changed

- Narrowed to macOS on Apple silicon, which is where this was written, built and
  run. Anywhere else the build now stops with a message saying so, rather than
  producing a binary nobody has tried.

### Fixed

- Port and GPIO commands now check the board's acknowledgement. Before, as in
  the C++ application, a rejected command still exited with 0 and a script had
  no way of noticing.
- An answer report shorter than the fixed 64 bytes is rejected instead of
  having its missing bytes read as zeros.
- An argument that is not valid Unicode ends in a usage error instead of a
  crash, and an empty argument vector - which `execve` permits - shows the
  help instead of crashing.

### Security

- Control characters in a command line argument are replaced before an error
  message echoes the argument, closing the same terminal escape route that was
  already closed for serial numbers a device reports.
- The CI workflows now grant the token read access only, stop persisting
  credentials into the checkout, pin the actions to commits instead of movable
  tags, and pin the cargo-deny version so the audit tool no longer sits frozen
  in a stale cache.

## 0.1.0 - 2026-08-15

First release. A control program for the Yepkit YKUSH3, written in Rust against
the USB protocol the vendor documents.

### Added

- Switching of the three downstream ports and the external 5V output, singly or
  all at once, and reading a port's state back
- Reading and writing the three GPIO pins, and handing them to the board as a
  control interface
- Configuring what a port does after a reset or a power cut
- Reset, entering the bootloader, and reading the firmware and bootloader
  versions
- I2C in both roles: as a slave the board takes switching commands from an
  outside master, as a master it carries transfers from the host onto the bus
- Listing attached boards and addressing one by serial number
- 81 tests that need no hardware, plus 6 that do: four want a board, two only
  the HID stack of the operating system

### Notes for anyone coming from the C++ application

The switches are the same and a leading `ykush3` is accepted, so existing
invocations keep working. The messages are worded differently, so a script that
parses output has to be adapted; exit codes are unchanged.

Behaviour that differs on purpose, each of them fixing something:

- the state of the external 5V port is decoded rather than reported as an error
- an I2C read longer than nine bytes no longer corrupts the report
- a GPIO read prints its value instead of returning it as the exit status
- hexadecimal arguments are accepted with and without the `0x` prefix
- `--reset` and `--boot` no longer wait for an answer the board never sends
- a serial number works wherever it appears on the line
- control characters in a serial number the device reports are replaced before
  the string reaches the terminal
