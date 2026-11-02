# Reporting a vulnerability

Report privately to the maintainer, Gerhard Schuster
<gerhard.schuster@44qm.net>, rather than opening a public issue. That address is
the one in the commit history, so it is not new information.

Say what the program does wrong and how to bring it about. A patch is welcome but
not expected. Expect an answer within a week; if none arrives, the report has gone
astray and is worth sending again.

Only the current state of the default branch is maintained. There are no
supported older releases to backport a fix to.

## What is in scope

This program talks to one USB device and prints what it says. The interesting
boundary is what comes back from that device: it is not trusted, and anything
that makes the program mishandle a hostile or broken board is in scope. So is
anything reachable through the command line.

Out of scope, because they are properties of the hardware rather than of this
program:

- that any local process allowed to open the device can switch ports or drop the
  board into its bootloader - macOS hands HID devices to any process that asks,
  on Linux the udev rule decides who may open the hidraw node, and this program
  neither adds nor could add a check there
- firmware behaviour of the board itself, which belongs to Yepkit
- denial of service by holding the device open; on Linux an invocation of
  this program waits at most five seconds for the lock described below and
  then gives up, and on macOS another program that seizes the device makes
  this program's opens and writes fail while it holds it (measured on
  2026-10-11 with a YKUSH3, see sidit77/async-hid#55, README section "Shared and
  Exclusive Access")
- interference between programs that have the device open at once, which
  Linux does not prevent; every answer goes to all of them. On macOS this
  program opens the device exclusively. See below for what it does about it
  and what it cannot do

## Concurrent use

Invocations of `ykushctrl` take turns at a board through an advisory
`flock` on its `/dev/hidrawN`. Advisory means it only orders the programs
that ask for it: the instances of `ykushctrl` and programs built on its
library. It is not access control and does not try to be one.

Nothing protects a run of Yepkit's `ykushcmd`, or of any other HID tool,
against the same board in parallel. A build of such a tool that uses hidraw
opens the node beside this program, and answers can be read by the wrong
process. A build that uses libusb detaches the kernel's HID driver, and the
board disappears from under this program. Any local user the udev rule lets
open the node can also take the lock and hold it, which makes this program
wait and then fail; that is the denial of service above, with a limit.

The rule that follows: **one YKUSH3 tool per host**.

On macOS this program opens the device exclusively (seized,
`Access::Exclusive` from sidit77/async-hid#55). While it holds the device,
every other open through the HID stack fails, from this program or any
other, and a second invocation waits for the first. A program that already
had the device open keeps its handle, but its writes fail while the seize
lasts. A tool that bypasses the HID stack is not covered. In turn, another
program that seizes the device makes this one wait and then fail, the
denial of service above.

Measured on 2026-11-02 on a Mac with a YKUSH3: two invocations reading at
the same time failed in none of 5080 runs; before the exclusive open, on
2026-10-11, they read each other's answers in about half the runs.

## What has already been looked at

[SECURITY-REVIEW-0.4.0.md](SECURITY-REVIEW-0.4.0.md) records the current review
of the trust boundary to the device, the command line, and the dependency
surface, taken after the transport moved to `async-hid`.
[SECURITY-REVIEW.md](SECURITY-REVIEW.md) is the earlier one, for the `hidapi`
transport, and is kept as the record of that state. Note who wrote them: the
author of the code, not an outside party. They are a record of what was
examined, not an independent audit.
