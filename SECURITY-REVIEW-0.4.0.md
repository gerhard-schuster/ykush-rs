# Security review - 0.4.0

| | |
|---|---|
| Subject | `ykush3cmd` after the HID transport moved from `hidapi` to `async-hid` |
| State | commit `26e550e`; the working tree carries documentation changes only |
| Scope | `src/`, `tests/`, `Cargo.toml`, `Cargo.lock`, `deny.toml`, and the patched `async-hid` the build resolves to |
| Date | 2026-09-24 |
| Method | reading the transport and protocol layers along the categories below, reading the IOKit backend of the dependency, `cargo deny check`, `cargo test`, and an attempt to build the package from a bare clone |

[SECURITY-REVIEW.md](SECURITY-REVIEW.md) covers the state before that move, when the only
dependency was `hidapi 2.6.6`. It is correct for what it reviewed and is left as it stands.
Where the two disagree, this one is current.

**Result: no HIGH or MEDIUM severity findings. One LOW, reachable only through the library
and not through the command line program.** A second LOW was raised and later withdrawn; the
section below says why, and the addendum records the correction.

## Why a second review was needed

Everything below the `Transport` trait was replaced. The earlier review could say nothing
about any of it:

- the dependency graph grew from one crate with a vendored C library to 28 crates of Rust,
  reaching IOKit through `objc2`
- the five second read timeout is now a race between the read and a timer, which means a
  future is dropped while it holds a mutable borrow of the answer buffer
- the leading report id byte is no longer prepended by this program
- the build resolved `async-hid` to a patched local checkout rather than to crates.io

## Attack surface examined

| Category | Finding |
|---|---|
| Command injection | no `std::process::Command` in production code; only in `tests/cli.rs`, with `CARGO_BIN_EXE_*` and fixed arguments |
| Path traversal, file access | no file operations at all; `Path` is used solely for the basename of `argv[0]` (`src/main.rs`) |
| Deserialisation, templating, XXE, SQL | not applicable - no parser and no serialisation format anywhere in the tree |
| Secrets, crypto | no keys, tokens or crypto calls; only the constants VID `0x04D8` and PID `0xF11B` |
| Format string injection | every `format!` and `writeln!` uses a literal with the values as arguments, which is not injectable by construction |
| `unsafe` in this program | none |
| `unsafe` in the dependency | 27 sites in the IOKit backend of `async-hid`, all of them FFI; the local patch adds one `unsafe impl Send` for a device handle moved into a dispatch block |
| Panics in production code | none reachable. `src/main.rs:95` is an `unreachable!` that `run()` guards by handling `Help`, `Version` and `List` before a board is opened; `src/fake.rs:72` sits behind the `fake` feature, which the shipped library does not enable |
| Test suite | 100 tests pass, 9 are ignored for want of a board |

## Focus: the new transport

### Cancelling a read is safe by construction

`Board::transfer` (`src/device.rs`) races `read_input_report(&mut resp)` against a timer and
drops whichever loses. That is the shape in which a use-after-free would live, if the
library handed the caller's buffer to the operating system and let IOKit write into it after
the future was gone.

It does not. `ReaderState::read` is a `poll_fn` that takes a finished report out of an
internally owned queue and copies it into the caller's buffer inside the poll
(`read_writer.rs`). IOKit writes only into buffers the library owns, registered once at open
time. A dropped read therefore leaves nothing pointing at `resp`, and the borrow ends with
the future.

### The write no longer hands out a pointer either

The published `async-hid` 0.5.3 passes the caller's buffer to
`IOHIDDeviceSetReportWithCallback` and returns; if that future is dropped before the
callback fires, IOKit is left holding a pointer into a buffer that may be gone. The local
patch (`e0aec7b`) replaces it with the synchronous `IOHIDDeviceSetReport` on a global
dispatch queue and copies the report into the block. That closes the hole, and it is the
reason this build must not fall back to the released crate.

### What a hostile board can put in an answer

Unchanged in substance from the earlier review, and re-checked against the current code:

- `Report` is a fixed `[u8; 64]`, freshly zeroed per exchange, so nothing leaks between
  commands
- a short read is rejected rather than interpreted (`Truncated answer from the board`),
  which the earlier transport could not distinguish
- the one length the device controls is now *rejected* when it exceeds the request rather
  than clamped: `i2c_read` refuses a reported count above the requested one, and the slice
  `resp[3..3 + reported]` has 62 as its highest index in a 64 byte buffer
- every other access to an answer is at a constant index inside the buffer

## Findings

### LOW 1 - WITHDRAWN: a device that publishes no maximum input report size

This was raised on the strength of `.expect("Device is not readable")` in
`read_input_report` (`read_writer.rs`), reading it as a panic a malformed device could
provoke. Following the open path afterwards showed that it cannot be reached, and the
finding does not stand:

- when the device does not publish `kIOHIDMaxInputReportSizeKey` and a readable handle was
  asked for, `DeviceReadWriter::new` closes the device and returns
  `HidError::message("Failed to read input report size")`. That is a handled error at open
  time, not a panic at read time
- `read_state` is `None` only for a handle opened write-only, and `open()` then hands out no
  reader at all: `read.then_some(rw.clone())`
- the public API cannot conflate the two. `DeviceReader`, `DeviceWriter` and
  `DeviceReaderWriter` are separate types, and `DeviceWriter` does not implement
  `AsyncHidRead`

The assertion guards an invariant the type system already enforces, which is what an
assertion is for. Recorded rather than deleted because a withdrawn finding is part of what a
review found.

### LOW - an answer that arrives after the timeout can be read as the answer to the next command

When the timer wins the race, the report that was in flight is not consumed: it stays in the
queue inside the library, and the next `transfer` pops it. The following command would then
read the previous command's answer.

The switching and configuration commands check only `resp[0] == op::ACK` (`request_acked`),
deliberately, because their answers beyond the status byte are undocumented. A stale ACK
passes that check, so such a command could report success without the board having acted on
it. Commands that decode data - port status, GPIO reads, versions, I2C - validate the echoed
opcode and would reject it.

**The command line program cannot reach this.** One invocation opens a board, performs
exactly one exchange and exits; `src/main.rs:270,280` assert that count for every command.
A queued report dies with the handle. The exposure is to library consumers that hold one
`Ykush3` across several commands - a test rig, which is the case the transport was hardened
for in the first place. Draining the queue after an `Error::NoResponse`, or reopening the
handle, would close it.

## Observations below the reporting bar

1. **`sanitize()` covers control characters, not format characters.** `char::is_control()`
   is the Unicode `Cc` category, so it catches C0 and C1 escapes - which is what the
   function was written for and what its tests assert. Characters in `Cf`, notably the bidi
   overrides `U+202E` and `U+202D`, pass through into terminal output from a device serial
   number or from `argv`. They cannot drive a terminal, only reorder what it displays.
2. **There is no write timeout.** `send()` has none because the library offers none, as the
   code says. A device that stalls its endpoint blocks for as long as the operating system
   allows.
3. **Any local process can drive the board.** macOS hands a HID device to whoever opens it
   first, with no privilege required. A property of the platform and the device, not of this
   program, and unchanged since the earlier review.

## Supply chain

`cargo deny check` passes in full: advisories ok, bans ok, licenses ok, sources ok. The
graph resolves to 28 crates including this package, all under MIT, Apache-2.0 or - for
`unicode-ident`, through proc macros of the Windows crates this target never builds  - 
Unicode-3.0.

One gap is worth stating plainly. The build depends on `[patch.crates-io] async-hid = { path
= "../async-hid" }`, a sibling checkout that is not part of this repository: branch
`setreport-fix`, one commit `e0aec7b` on top of the released 0.5.3. A clone of this
repository on its own does not build - `cargo metadata` on a bare export fails with `failed
to load source for dependency async-hid`. So the artifact reviewed here is not the artifact
a third party can currently produce from the published sources, and anything that did
resolve `async-hid` from crates.io would get 0.5.3, with both the input-report stall and the
dropped-pointer hole described above. This closes when the fix is released upstream; until
then the patch should point at a git revision of the fork rather than at a path.

## Compared with the hidapi state

The position improved in three ways and became more dependent in one.

| | |
|---|---|
| No third party C is compiled or shipped any more | better |
| A dropped write no longer leaves IOKit holding a pointer into this program's memory | better - and it is a hole the released crate still has |
| A truncated answer is rejected instead of being read together with buffer padding | better |
| A stale-answer path now lives in code this project does not own | worse |

## Addendum - what was fixed afterwards

The findings above describe the state at the time of the review. All three were acted on
the same day; the sections are left as they were written.

**LOW 1 was withdrawn, not fixed.** It was first acted on by turning both assertions into
returned errors. Checking the open path afterwards showed there is nothing to fix: the
missing-property case already errors at open time, and a handle without a reader is never
handed to a caller who could read from it. The change was dropped again rather than left in
as defensive code for a state that cannot occur. The finding section above carries the
reasoning.

**The remaining LOW is closed for the common case.** `Board` now carries a `stale` flag
(`src/device.rs`). A read that loses the race against the timer sets it, and the next
exchange empties the queue of the transport before sending, so an answer that arrived late
is dropped instead of being read as the answer to the following command. The drain is
bounded twice, by a 50 ms wait and by 16 reports, so neither a silent board nor one that
streams can hold the program in it.

What remains: an answer that arrives *after* the drain and before the real one is still
ambiguous, because the protocol carries no correlation id and the switching commands answer
with nothing beyond the status byte. Closing that would take either a board that echoes its
opcode on those commands - which has not been observed - or reopening the handle after every
timeout. The window is now the length of one exchange rather than unbounded.

The drain has no automated test. Provoking it needs a board that answers late, which no fake
can stand in for and no healthy device does.

**The sanitiser now covers format characters too.** `sanitize()` replaced the `Cc` category
and nothing else, so the bidirectional overrides and the zero width characters reached the
terminal. They cannot drive it, but `U+202E` reorders the rest of the line and `U+200B` and
its neighbours print as nothing, which is enough to display one board's serial number as
another's. Both groups are replaced now, with two tests naming what each one could have
done. `U+FFFD` is still kept, so a lossy `argv` conversion still shows where the bad byte
was.

**The supply chain gap is closed.** `[patch.crates-io]` pointing at `../async-hid` is gone.
`async-hid` is an ordinary git dependency now, pinned by revision to the fork, and
`Cargo.lock` records which one. The revision has moved since this was written, when
sidit77/async-hid#50 was merged and the branch rebased onto it; the tree did not change. A copy of this repository with no sibling directory builds, passes
the suite and drives a board, which is what the review found it could not do. The pin is
itself temporary: it goes away when the fix is released on crates.io.

## Addendum 2026-11-02 - from the review of ykur-rs

ykur-rs, the program for Yepkit's YKUR relay board, shares this exchange. Its security review
(`SECURITY-REVIEW-0.1.0.md` there) found four things that apply here as well, and closes the
gap described above under "What remains".

- **An answer left behind by an earlier invocation.** On Linux the kernel polls the board only
  while the node is open, so an answer to a command whose invocation was killed between write
  and read can wait in the board and reach the next invocation first. The command line program
  here makes one exchange per run, which the review above took as safe; this way that exchange
  is the one hit, and a switching command, which checks only the status byte, takes any answer
  with an ACK in it for its own. Whether the board keeps such an answer is not measured.
- **Getting back in step only after a timeout, and only within 50 ms.** A read error, a short
  report or an answer that does not fit left the queue as it was.
- **No timeout on the write.**
- **`sanitize()` let other invisible and look-alike characters through**, such as tag
  characters, U+2028, U+3164 and Cyrillic letters.

What changed: a handle starts out of step, and every exchange that fails or gets an answer that
does not fit puts it out of step again. Out of step, the next exchange first asks for the
firmware version and drops every report before its answer, at most 16 and within five seconds
plus the quiet described next.
Since that answer, or the empty report firmware before 1.1.0 gives instead, may itself have been
left behind, the queue then has to stay quiet for 50 ms; another report means the wait goes on.
The stale flag and the 50 ms drain are gone. The write races a five second timer like the read,
which bounds it on macOS; on Linux the transport's write runs inside the kernel and the kernel's
USB timeout bounds it. `sanitize()` lets printable ASCII through and U+FFFD, nothing else. The
exchange now has tests through a stand-in for the transport, which the drain above did not.
Boards with the same serial number are still merged; SECURITY.md lists that as a known limitation.

On 2026-11-02 the changed exchange ran against board `Y3N13808` on macOS: the hardware tests
pass, two parallel loops of 200 invocations did not fail, and an invocation takes about 74 ms
instead of 11 ms. An answer left behind by a program that exits without reading did not reach
the next invocation on macOS, before the change or after it, so the case is not reproduced
there. It has not been tried on Linux, where the review expects it.

## Addendum 2026-11-07 - re-review based on `09fadd5`

Corrected on 2026-11-09 after an independent review of the branch. The addendum named `09fadd5`
as its state, but its fixes came after that commit, in the change that added it. References now
name symbols instead of line numbers, and the checks below are from the correction. The
corrections change the record, not its findings, and are not a new review.

Between `26e550e` (0.4.0) and `09fadd5` ("Test the waiting on macOS and FreeBSD against
fakes"), the repository was renamed to `ykush-rs` (package `ykushctrl`, library `ykush`), and the
program gained Linux and FreeBSD support, exclusive
access / advisory locking (`--no-wait`), Linux detached-interface diagnostics, waiting across
board resets on macOS and FreeBSD, and `sysexits(3)` exit codes. All source files (`src/`,
`tests/`, `Cargo.toml`, `Cargo.lock`, `deny.toml`, `SECURITY.md`, and `.github/workflows/`)
were re-read against the categories above.

**Result of the 2026-11-07 update: no HIGH or MEDIUM findings.** One new observation below the
reporting bar applies specifically to `ykush-rs`'s Linux detached-interface diagnostic
(`B-ykush-1` below), alongside the platform and supply-chain observations `B-ykush-2` to
`B-ykush-6`. The commands behind this result were not recorded (UNKNOWN).

### Checks run for the correction (2026-11-09, macOS aarch64, Rust 1.99.0 and 1.98.1)

No board was attached, and no test ran on Linux or FreeBSD. These checks say nothing about the
findings above; they record the state the correction was made on.

| Command | Exit | Result |
|---|---|---|
| `cargo test --all-features --locked` | 0 | 177 tests and a doctest pass, 11 ignored |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | 0 | no warning |
| the same without `--all-features`, with `--target aarch64-unknown-linux-gnu` | 0 | no warning |
| `cargo +1.98 check --all-targets --locked` | 0 | builds |
| the same with `--target aarch64-unknown-freebsd` | 0 | builds |
| `cargo +1.98 clippy --all-targets --locked --target aarch64-unknown-freebsd -- -D warnings` | 0 | no warning |
| `cargo fmt --check` | 0 | clean |
| `cargo deny --all-features check` | 0 | advisories, bans, licenses and sources ok |

### Updated attack surface (after the fixes)

| Area | What was found |
|---|---|
| Parsing the answers (`src/ykush3.rs`) | Fixed 64 byte `Report`, freshly zeroed per exchange; short reads rejected; `i2c_write` bounds `data.len() <= 60` (`I2C_MAX_BYTES`) so the request, 4 bytes and the data, fits inside 64 bytes; `i2c_read` bounds `len <= 60` and checks `reported <= usize::from(len)` before slicing `resp[3..3 + reported]` (highest index 62); all other reads use constant indices `0..=4`. Commands whose answer carries data or an echo check it; switching and configuration commands check only the status byte, since their answers beyond it are undocumented. The version-query resync (`0x61 0x02`) and the 50 ms quiet window narrow what a late answer can do but do not rule it out, see B-ykush-3 |
| Command execution (`src/device.rs`) | FreeBSD only: `/sbin/sysctl` is invoked via `Command::new("/sbin/sysctl").args(["-n", &name])` during enumeration (the `sysctl` closure in `hidraw_boards`). No shell is invoked; the binary path is fixed; `-n` precedes the key argument |
| File system & `/sys`, `/dev` access (`src/device.rs`, `src/lock.rs`) | Linux reads `/sys/bus/usb/devices` (`detached_interfaces`) and `/sys/class/hidraw/*/device/uevent` (inside `async-hid`), and opens `/dev/hidrawN` in `lock.rs` after checking `hidraw` + ASCII digits (`hidraw_node`). FreeBSD reads `/dev` (`hidraw_boards`) and opens `/dev/hidraw*` (`node_device`) |
| Locking on Linux (`src/lock.rs`) | `flock(LOCK_EX \| LOCK_NB)` on `/dev/hidrawN` via `rustix::fs::flock`. After locking, `take_stable` (`src/lock.rs`) re-reads `hidraw_node_of(&device)` to guard against udev renumbering while waiting (bounded by `NODE_ROUNDS = 3` and the total `wait` budget) |
| Locking & reset waiting on macOS (`Seize`, `src/device.rs`) | `Access::Exclusive` (`kIOHIDOptionsTypeSeizeDevice` in the pinned `async-hid` fork). `Seize` keeps polling across a brief disappearance while the board resets (`--reset` / `reset`), bounded by the wall-clock `wait` deadline in `lock::wait_for` |
| Locking & reset waiting on FreeBSD (`NodeWait`, `src/device.rs`) | `hidraw(4)` allows one open at a time. When a target board is held (`EBUSY`), `NodeWait` records its `(serial, node)` and on subsequent 10 ms polls calls `node_device` on that single `/dev/hidrawN` node only, avoiding `EBUSY` interference with other HID devices on the host and avoiding repeated `/sbin/sysctl` spawns during the wait |
| Terminal & CLI output (`src/sanitize.rs`, `src/main.rs`) | `sanitize()` permits only `0x20..=0x7E` and `U+FFFD`. Applied to USB serial numbers on all platforms (including Linux sysfs `serial` in `detached_interfaces` and FreeBSD `sernum` in `ykush3_serial`) and to `argv` items before putting them into `Error::Usage` |
| `unsafe` and panics | Zero `unsafe` in `src/` and `tests/`. No reachable panics in production code (the `unreachable!` in `src/main.rs` is guarded by `run()` handling `Help`, `Version` and `List`; the `expect` in `src/fake.rs` is behind `#[cfg(any(test, feature = "fake"))]`) |
| Test suite | see the checks above; `09fadd5` itself had 173 tests according to its README |

### Observations below the reporting bar (2026-11-07)

- **B-ykush-1: Linux `detached_interfaces` does not sanitise the interface directory name.**
  *Fixed:* `detached_interfaces` now passes each matched directory name
  through `sanitize()` before `detached_message` formats it into the `stderr` / `Error::Unavailable`
  hint.
- **B-ykush-2: FreeBSD `hidraw_boards` checks `starts_with("hidraw")` rather than digits.**
  *Fixed:* `is_hidraw_name` requires `hidraw` followed by one or more ASCII
  digits on both Linux (`hidraw_node`) and FreeBSD (`hidraw_boards`).
- **B-ykush-3: Residual resync ambiguity on legacy firmware (`< 1.1.0`) or back-to-back version queries.**
  `resync` (`src/device.rs`) takes either a 64 byte report starting `0x01 0x61` or an all-zero
  one (for firmware `< 1.1.0`) as the answer and then requires 50 ms of quiet. If a stalled exchange left an answer arriving *after* the 50 ms
  quiet window, that late answer will be read by the next command; commands that check an echoed
  opcode reject it (exit 76), except switching/config commands if the late answer happens to
  start with `0x01`, or a `--firmware-version` query if the late answer was itself a
  firmware version reply. Documented in `SECURITY.md` under known limitations.
- **B-ykush-4: Disconnect error mapping on Linux (`ENODEV` -> exit 74 vs exit 69).**
  *Fixed:* `Error::from(HidError)` now maps `ENODEV` (`19`) inside
  `HidError::Other` to `Error::Unavailable` (exit 69), matching `NotConnected` and `Disconnected`.
- **B-ykush-5: `src/help.rs` still states the exit status from before the `sysexits(3)` codes.**
  *Fixed:* `src/help.rs` now describes the `sysexits(3)` exit status (`64` to `77`).
- **B-ykush-6: No `#![forbid(unsafe_code)]` and CI/supply-chain hygiene.**
  *Fixed except git-pin availability:* `src/lib.rs` and `src/main.rs` now set
  `#![forbid(unsafe_code)]`; `audit.yml` runs `cargo deny` on `pull_request` as well as `push`
  and `schedule`; `ci.yml` pins `RUST_TOOLCHAIN` (included in the cache key) and checks
  `--target aarch64-unknown-freebsd` in both `lint` (`clippy -- -D warnings`) and
  `minimum-supported-rust`; `NOTICE` and `THIRD-PARTY-NOTICES.md` cover FreeBSD. The
  `async-hid` fork branch pin (`1d1a2bf4` until upstream `0.6.0` with `#55` is released) remains
  open.

### Checked against SECURITY.md (after the fixes)

| Statement in `SECURITY.md` | Assessment |
|---|---|
| Answers from the board are untrusted and checked against the request | Matches `src/ykush3.rs` and the 4000-round sweep test |
| Every read gives up after 5 seconds, and so does every write on macOS (in-process timer); on Linux and FreeBSD the kernel bounds the write | Matches `TIMEOUT` and the `Link` implementation in `src/device.rs` |
| Serial numbers and `argv` sanitised before reaching the terminal | Matches `src/sanitize.rs` and its call sites in `src/device.rs` and `src/main.rs`, which sanitises `argv` before parsing it |
| macOS exclusive open; Linux advisory `flock` on `/dev/hidrawN` (does not coordinate with `ykushcmd`); FreeBSD one open of a `hidraw` node at a time, with the board's own node opened every 10 ms while waiting and the other nodes only to find it | Matches `src/device.rs` and `src/lock.rs` |
| Known limitations: duplicate serial numbers merged, resync via firmware version query + 50 ms quiet window | Accurately documented in `SECURITY.md` |

## Scope

A snapshot of the states named above: `26e550e`, the addendum of 2026-11-02, whose commit it does
not name (UNKNOWN), and for 2026-11-07 `09fadd5` with the fixes the addendum lists. Changes made afterwards are not covered. A review with no finding above LOW is no proof of
correctness - it says that nothing more was found in the categories examined.
