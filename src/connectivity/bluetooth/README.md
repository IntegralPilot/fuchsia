# Bluetooth

The Fuchsia Bluetooth system (a.k.a. Sapphire) provides a modern, dual mode
implementation of the Bluetooth Host Subsystem (5.0+) supporting a number of
Low Energy and Traditional profiles.

## High-level Architecture Overview

The Sapphire stack is organized in three major layers with communication
between layers happening via FIDL and Zircon channels: driver, host, and
profile.

The driver layer is responsible for discovery and communication with controller
hardware in two stages:
 - transport: implements the HCI Transport protocol on the underlying
   hardware bus (USB, Serial, or SD)
 - vendor: loads firmware and manages handling and/or encoding vendor-specific
   commands.  Controllers that do not require firmware are handled using the
   `passthrough` vendor driver.

SCO, ACL, and HCI sockets are connected to the bt-host component through the
[Bluetooth Hardware HCI Protocol](/sdk/fidl/fuchsia.hardware.bluetooth/hci.fidl).

The host layer is comprised of the bt-init, bt-host, and bt-gap components.

The bt-init component is eagerly started on a Fuchsia system to discover and
connect bt-host components as well as manage the lifetime of ambient profile
components and route protocols through pairing and RFCOMM passthrough
components.

One bt-host is started for each connected controller, and manages
host state as well as sending commands and receiving events, tracking state,
and managing connections.  It also contains Low Energy advertising and GATT
handlers, and an SDP server.  L2CAP and GATT traffic is routed through bt-host
and sent to the above associated profiles.

The bt-gap component coordinates the system Bluetooth state to all of the
bt-host components, serves the [system APIs](/sdk/fidl/fuchsia.bluetooth.sys)
and routes connections from profile and protocol components to the active
host.

The profile layer encompasses all profile components included in the product
assembly.  Profiles are enabled by including them.
They register themselves with the host layer when started.
Common profile configurations will be provided in separate Assembly Input Bundles
and are documented in their individual directories and/or bundles.
Profile components are independently testable and updatable.

Source code shortcuts:

-   Public API:
    *   [Shared](/sdk/fidl/fuchsia.bluetooth)
    *   [System API](/sdk/fidl/fuchsia.bluetooth.sys)
    *   [BR/EDR (Profile)](/sdk/fidl/fuchsia.bluetooth.bredr)
    *   [GATT](/sdk/fidl/fuchsia.bluetooth.gatt)
    *   [LE](/sdk/fidl/fuchsia.bluetooth.le)
-   [Private API](fidl)
-   [Tools](tools)
-   [bt-host](core/bt-host)
-   [bt-gap component](core/bt-gap)
-   [bt-init component](core/bt-init)
-   [Profiles](profiles)
-   [HCI Drivers](hci)
    *   [Transport Drivers](hci/transport)
    *   [Vendor Drivers](hci/vendor)

For more information about the structure of the Sapphire stack, please refer to the
[Bluetooth Architecture guide](/docs/development/bluetooth/concepts/architecture.md),
or READMEs in specific subdirectories.

For a note on used (and avoided) vocabulary, see [Bluetooth Vocabulary](docs/vocabulary.md)

## Getting Started

### API Examples

Examples using Fuchsia's Bluetooth Low Energy APIs can be found
[here](examples).

### Privileged System API

Dual-mode (LE + Classic) GAP operations that are typically exposed to privileged
clients are performed using the
[fuchsia.bluetooth.sys](/sdk/fidl/fuchsia.bluetooth.sys) protocol. This API is
intended for managing local controllers, device discovery & discoverability,
pairing/bonding, and global settings.

[`bt-cli`](tools/bt-cli) is a command-line front-end for privileged access
operations:

```
$ bt-cli
bt> list-controllers
HostId            Addresses             Active  Technology  Name                    Discoverable  Discovering
e5878e9f642d8908  [34:13:E8:86:8C:19]   true    DualMode    fuchsia-12ab-34cd-56ef  true          false
de22345ab1234cc0  [34:13:E8:86:8D:20]   false   DualMode    fuchsia-77aa-88bb-99cc  false         false
```

### Tools

See the [bluetooth/tools](tools/) package for more information on available
command line tools for testing/debugging.

### Running Tests

Your build configuration may or may not include Bluetooth tests. Ensure
Bluetooth tests are built and installed when paving or OTA'ing with
[`fx set`](/docs/development/build/fx.md#configure-a-build):

```
  $ fx set workstation_eng.x64 --with //src/connectivity/bluetooth,//bundles/tools
```

#### Tests

The Bluetooth codebase follows the
[Fuchsia testing best practices](/docs/contribute/testing/best-practices.md). In
general, the Bluetooth codebase defines an associated unit test binary for each
production binary and library, as well as a number of integration test binaries.
Without good reason, no code should be added without an appropriate (unit,
integration, etc) corresponding test. Look in the `GN` file of a production
binary or library to find its associated unit tests.

For more information, see the
[Fuchsia testing guide](docs/development/testing/run_fuchsia_tests.md).

##### Running on a Fuchsia device

*   Run all the bt-host unit tests:

    ```
    $ fx test //src/connectivity/bluetooth/core/bt-host
    ```

*   Run a specific test within `bt-host`:

    ```
    $ fx test //src/connectivity/bluetooth/core/bt-host -- --gtest_filter='Foo.Bar'
    ```

    Where `Foo` and `Bar` in `Foo.Bar` are the fixture name and the test name,
    respectively.

To see all options for running these tests, run `fx test --help`.

##### Running on the Fuchsia Emulator

If you don't have physical hardware available, you can run the tests in the
Fuchsia emulator (FEMU) using the same commands as above. See the
[instructions for FEMU](/docs/get-started/set_up_femu.md).

#### Integration Tests

`TODO(https://fxbug.dev/42178537): Updated Integration Documentation`

### Controlling Log Verbosity

#### Logging in Drivers

The most reliable way to enable higher log verbosity is to enable them using kernel command
line parameters. The `//src/connectivity/bluetooth:driver-debug-logging` target is provided as a
convenience to add all the existing chipsets and should be included in `dev_bootfs_labels`,
for example using the `fx set` command:

```
  fx set core.x64 --args="dev_bootfs_labels=[\"//src/connectivity/bluetooth:driver-debug-logging\"]"
```

Using `fx set` writes these values into the image, so they will survive a system restart.
For more detail on driver logging, see
[Zircon driver logging](/docs/development/drivers/diagnostics/logging.md)

#### Other components

All other components can be controlled at runtime, using the diagnostics selector method.
For example, to show all logs up to `DEBUG` level in the A2DP profile component, use:

```
ffx log --set-severity core/bt-a2dp#DEBUG
```

Component monikers can be found using `ffx component list`.

### Inspecting Bluetooth State

On Fuchsia, Sapphire supports inspection through the
[Inspect API](/docs/development/diagnostics/inspect). With the exception of the
driver layer, all components expose information through inspect.  Each component's
documentation includes an example of the expected inspect tree.

#### Example Usage

*   bt-host: `ffx inspect show core/bluetooth-core/bt-host-collection*`
    exposes information about the controller, peers, and services.
*   bt-gap: `ffx inspect show core/bluetooth-core/bt-gap` exposes information on host devices
    managed by bt-gap, pairing capabilities, stored bonds, and actively connected peers.
*   bt-a2dp: `ffx inspect show core/bt-a2dp` exposes information on audio streaming
    capabilities and active streams
*   bt-snoop: `ffx inspect show core/bt-snoop` exposes information about which HCI
    devices are being logged and how much data is stored.
*   All core Bluetooth components `ffx inspect show core/bluetooth-core/*`
*   All other Bluetooth components: `ffx inspect show core/bt-*`

### Respectful Code

Inclusivity is central to Fuchsia's culture, and our values include treating
each other with dignity. As such, it’s important that everyone can contribute
without facing the harmful effects of bias and discrimination.

Bluetooth Core Specification 5.3 updated certain terms that were identified as
inappropriate to more inclusive versions. For example, usages of 'master' and
'slave' were changed to 'central' and 'peripheral', respectively. We have
transitioned our code's terminology to the more appropriate language. We no
longer allow uses of the prior terms. For more information, see the
[Appropriate Language Mapping Table](https://specificationrefs.bluetooth.com/language-mapping/Appropriate_Language_Mapping_Table.pdf)
published by the Bluetooth SIG.

See the Fuchsia project [guide](/docs/contribute/respectful_code.md) on best
practices for more information.
