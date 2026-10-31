# Contributing

## What license your contribution is under

This project is under the Apache License 2.0, and section 5 of that license already settles
the question:

> Unless You explicitly state otherwise, any Contribution intentionally submitted for
> inclusion in the Work by You to the Licensor shall be under the terms and conditions of
> this License, without any additional terms or conditions.

So sending a patch offers it under the same license, unless you say otherwise when you send
it. There is no separate contributor license agreement, and none is needed.

Two consequences are worth knowing before you send anything:

- **You keep your copyright.** Nothing is assigned; you grant a license. Your copyright
  notice may stand in your contribution, and it then has to be kept.
- **You grant a patent license** (section 3) covering what your own contribution
  necessarily infringes. It terminates for anyone who brings patent litigation over this
  work. If that does not suit you, do not contribute.

A `Signed-off-by` line is not required. Setting one out of habit (`git commit -s`) breaks
nothing.

## Third party code in your contribution

If you bring code you did not write, its license has to fit Apache 2.0, and the
contribution has to make that visible:

| Origin | What to do |
|---|---|
| Apache 2.0, including the C++ original at `github.com/Yepkit/ykush` | keep the copyright notice, state the change, add the origin to `NOTICE` |
| MIT, BSD, ISC | add the license text to `THIRD-PARTY-NOTICES.md` |
| GPL, LGPL, AGPL | **do not send it** - incompatible with this project's license |
| unclear | ask first rather than sending it and hoping |

The same goes for dependencies: a new crate belongs in `THIRD-PARTY-NOTICES.md` together
with its license. Dual licensed crates are used here under Apache-2.0.

The case that comes up here in particular: the C++ original often sits in a neighbouring
directory. It is under the same license, so taking from it is allowed - but not silently.
Text copied word for word needs the notice, otherwise the license statement of this
repository stops being true. Protocol knowledge - opcodes, report layout, what answers
mean - is not affected; those are facts.

## Vendor documentation

Datasheets and manuals from Yepkit do **not** belong in this repository. They are under no
free license. `doc/README.md` links to them, and that is where a new source goes.

## What a contribution has to satisfy

```
cargo test                                  # has to pass
cargo clippy --all-targets                  # has to be silent
cargo test -- --ignored --test-threads=1    # only with a board attached
```

- **Tests are part of it.** The protocol layer can be exercised without hardware through the
  `Transport` trait; `FakeBoard` records the bytes that go out and feeds answers back. A new
  command without a test pinning its wire bytes will not be taken.
- **Hardware tests** are marked `#[ignore]` and run one at a time (`--test-threads=1`),
  because every answer of the board reaches every open handle, so tests running side by
  side would read each other's answers.
- **Protocol changes need evidence**: a place in the vendor documentation, or a measurement
  reproduced on real hardware. Guessing is not enough.

## Form

- **Code and commit messages in English.**
- Commit messages carry **no trailers**. Subject line in the imperative, and below it a
  paragraph explaining *why* - the *what* is in the diff.
- Do not mix reformatting with substantive changes in one commit.

## Sending it

A patch by mail to the maintainer, or a pull request once the project has a remote. The
maintainer decides about releases.
