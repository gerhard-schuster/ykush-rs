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
- denial of service by another program that opens the device exclusively, a
  seize on macOS, which makes this program's opens and writes fail while it
  holds the device
- interference between programs that have the device open at once: neither
  macOS nor Linux opens it exclusively for this program, and every answer
  reaches all of them

## What has already been looked at

[SECURITY-REVIEW-0.4.0.md](SECURITY-REVIEW-0.4.0.md) records the current review
of the trust boundary to the device, the command line, and the dependency
surface, taken after the transport moved to `async-hid`.
[SECURITY-REVIEW.md](SECURITY-REVIEW.md) is the earlier one, for the `hidapi`
transport, and is kept as the record of that state. Note who wrote them: the
author of the code, not an outside party. They are a record of what was
examined, not an independent audit.
