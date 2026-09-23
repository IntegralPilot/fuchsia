# Software Delivery README

## Overview

The Software Delivery (SWD) stack is responsible for all updates of runnable
software on a Fuchsia device, including system updates and individual package
updates.

This document serves as a jumping-off point for the SWD codebase, and a
contribution guide.

Before starting, you may wish to read some [documentation](https://fuchsia.dev/fuchsia-src/concepts/packages/package) on
Fuchsia software packaging, or the [long term SWD goals](https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0133_swd_goals).

### Software Delivery (SWD) Subsystems

![Software Delivery Diagram](doc/overview.png)

### Major Binaries

All subsystems listed below are written in Rust.

Updated: January 2025

| Subsystem                 | Purpose                                                                                            | Location                                     |
|-----------------------    |----------------------------------------------------------------------------------------------------|----------------------------------------------|
| pkg-cache                 | Caches downloaded packages in case they are needed again, interacting as necessary with blobfs.    | `//src/sys/pkg/bin/pkg-cache`                |
| pkg-resolver              | Main entry point for software delivery stack. Coordinates retrieval and  installation of packages. | `//src/sys/pkg/bin/pkg-resolver`             |
| omaha-client              | Checks for [system updates using the Omaha server infrastructure](https://fuchsia.dev/fuchsia-src/concepts/packages/ota#update-omaha)                                    | `//src/sys/pkg/bin/omaha-client`             |
| pkgctl                    | CLI for pkg-resolver                                                                               | `//src/sys/pkg/bin/pkgctl`                   |
| system-update-checker     | Does what it says on the tin, [checks for system updates!](https://fuchsia.dev/fuchsia-src/concepts/packages/ota#update-system)                                           | `//src/sys/pkg/bin/system-update-checker`    |
| system-updater-committer  | Component responsible for [committing the update.](https://fuchsia.dev/fuchsia-src/concepts/packages/ota#committing-update)                                                   | `//src/sys/pkg/bin/system-update-committer`  |
| system-updater            | Actually [performs system updates.](https://fuchsia.dev/fuchsia-src/concepts/packages/ota#staging-update)                                                                  | `//src/sys/pkg/bin/system-updater`           |
| update                    | Developer CLI for interacting and configuring system updates.                                      | `//src/sys/pkg/bin/update`     |
| far                       | Creates Fuchsia archive files.                                                                     | `//src/sys/pkg/bin/far`                      |

### Testing

Integration tests for all binaries listed above can be found in their respective paths within `//src/sys/pkg/tests/{BINARY_NAME}`. Related helper libraries used in these integration tests can be found at `//src/sys/pkg/testing`.

For wider system tests, including end-to-end tests of system OTA updates, see `//src/sys/pkg/tests/system-tests`.

#### Key Dependencies

*   [rust-tuf](https://fuchsia.googlesource.com/third_party/rust-mirrors/rust-tuf/)
*   [go-tuf](https://fuchsia.googlesource.com/third_party/go-tuf/)
*   [hyper](https://github.com/hyperium/hyper)
*   [rustls](https://github.com/ctz/rustls).

#### Useful Debugging Tools

*   [`fidlcat`](https://fuchsia.dev/fuchsia-src/development/tools/fidl_inspecting):
    it’s `strace`, but for every IPC on the system, not just syscalls.
*   [`zxdb`](https://fuchsia.dev/fuchsia-src/development/debugger):
    Fuchsia’s debugger. Similar usage to `gdb`, and has Unicode support
    (emoji!). Doesn’t currently work well with golang, but works fine with Rust.
*   [Inspect](https://fuchsia.dev/fuchsia-src/development/diagnostics/inspect):
    Opt-in APIs for components to expose aspects of their state. Several
    portions of the SWD stack implement this, and more to come.

##### IDEs

VS Code seems to work pretty well. Follow the instructions
[here](https://fuchsia.dev/fuchsia-src/reference/tools/editors#vs-code), including
any language-specific instructions you find relevant; the Rust instructions are
a good place to start.

#### Style Guide

Use the style guide appropriate for the language you’re in. The SWD stack is
mostly in Rust and Go, which have strong opinions about style. All commits in
those languages should be formatted with `rustfmt` and `gofmt` respectively, and
most editors/IDEs have a mode in which you can run those commands when you save
a file. Do so!

### Setup

#### Fuchsia Setup

Read the Fuchsia Getting Started guide
[first](https://fuchsia.dev/fuchsia-src/get-started/learn-fuchsia).

Most of the SWD stack is in the `base` image of fuchsia, so to get a swd stack
working with tests, your build configuration is quite simple:

#### First tests

```sh

Tab 1 > fx set core.x64 --with //bundles/tests && fx build && fx serve -v

Tab 2 > sudo ip tuntap add dev qemu mode tap user $USER && sudo ip link set qemu up

Tab 2 > ffx emu start --net tap -H # starts Fuchsia in an emulator running in the background, headless

Tab 2 > fx test pkg-resolver-integration-tests # example of running the pkg-resolver integration tests

```

For further info on fx workflows:
https://fuchsia.dev/fuchsia-src/development/workflows/fx

#### More general tests

If you’ve successfully run the above, you have a working Fuchsia system with the
software delivery stack working.

You can discover more tests with by running `fx list-packages` on the host.

### Common Workflows

##### Updating a base image to get a new version of pkg-resolver or pkgctl

To update the base of your fuchsia image, you can use `fx ota` if you’re
running on hardware which supports OTA. If you’re running under QEMU, you’ll
usually just restart QEMU to get an updated base after a rebuild. Don’t worry,
it’s fast. If you are running an emulator that supports OTA workflows (i.e.
started with `ffx emu start --uefi ...`), you can use `fx ota` if you prefer.

##### Pulling down a new version of an external dependency

You’ll need to update that dependency’s vendored repository in //third_party.
See the
[Rust documentation](https://fuchsia.dev/fuchsia-src/development/languages/rust/external_crates)
for examples.

##### Resolve a package and run a contained component

The package resolver is configured by default to resolve `fuchsia.com` to the
local development host. To run a component in a package you’ve built locally,
you can run something like `ffx component run /core/ffx-laborary:test
fuchsia-pkg://fuchsia.com/<package_name>#meta/<component_name>.cm`

### FAQs

#### How do I run a hosted package server?

See the instructions on
[publishing a package with ffx](https://fuchsia.dev/fuchsia-src/development/idk/documentation/packages).

### More information:

*   [OTA Flow](https://fuchsia.dev/fuchsia-src/concepts/packages/ota)
*   [Package overview](https://fuchsia.dev/fuchsia-src/concepts/packages/package)
*   [Package updates](https://fuchsia.dev/fuchsia-src/concepts/packages/update_pkg)
*   [Package metadata](https://fuchsia.dev/fuchsia-src/concepts/packages/package#meta-far)
*   [Package URLs](https://fuchsia.dev/fuchsia-src/concepts/packages/package_url)
*   [Software updates](https://fuchsia.dev/fuchsia-src/concepts/packages/software_update_system)
