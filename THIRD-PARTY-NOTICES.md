# Third party notices

The crates that go into the binary. Versions are not repeated here - they are
pinned in `Cargo.lock`, which is where they stay correct. Crates that the
lockfile carries for other targets are left out, since they are never built
here.

No C library is vendored or linked. The HID access goes through Apple's IOKit
framework on macOS and through the kernel's hidraw interface on Linux, both
reached from Rust.

## Linked into the binary, macOS

Under **MIT**:

    async-hid   block2   objc2   objc2-encode   slab

Under **MIT or Apache-2.0**, used here under Apache-2.0:

    async-io          atomic-waker      bitflags       cfg-if
    concurrent-queue  crossbeam-queue   crossbeam-utils errno
    fastrand          futures-core      futures-io     futures-lite
    libc              log               parking        pin-project-lite
    polling           static_assertions

Under **Zlib, Apache-2.0 or MIT**, used here under Apache-2.0:

    dispatch2   objc2-core-foundation   objc2-io-kit

Under **Apache-2.0 with LLVM exception, Apache-2.0 or MIT**, used here under
Apache-2.0:

    rustix

## Linked into the binary, Linux

Under **MIT**:

    async-hid   memoffset   nix   slab

Under **MIT or Apache-2.0**, used here under Apache-2.0:

    async-io          bitflags          cfg-if         concurrent-queue
    crossbeam-utils   fastrand          futures-core   futures-io
    futures-lite      libc              log            parking
    pin-project-lite  polling           static_assertions

Under **Apache-2.0 with LLVM exception, Apache-2.0 or MIT**, used here under
Apache-2.0:

    linux-raw-sys   rustix

## Build time only

Runs during the build and leaves no code in the resulting binary:

    autocfg       (MIT or Apache-2.0)
    cfg_aliases   (MIT, Linux only)

## Other targets

Building for Windows pulls a different set, the `windows` crates. Among them
`unicode-ident` is published as `(MIT OR Apache-2.0) AND Unicode-3.0`, which is
why `deny.toml` allows the Unicode license as well. None of it is built on
macOS or Linux.

## MIT License

Applies to the crates listed under MIT above. The dual licensed ones are used
under Apache-2.0, whose text is in `LICENSE`.

```
Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

To regenerate this list after a dependency change:

```
cargo tree --edges normal,build --target aarch64-apple-darwin --prefix none
cargo tree --edges normal,build --target aarch64-unknown-linux-gnu --prefix none
cargo metadata --format-version 1 --filter-platform aarch64-apple-darwin
cargo metadata --format-version 1 --filter-platform aarch64-unknown-linux-gnu
```
