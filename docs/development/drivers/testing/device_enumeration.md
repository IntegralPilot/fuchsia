# Device enumeration tests

*System-level driver topology verification*

Note: ***Device enumeration tests verify driver stack integration across the full board
topology.***

When developing new drivers, modifying driver binding rules, or migrating legacy DFv1 drivers to
the new driver framework (DFv2), it is crucial to verify that all expected devices enumerate in the
system topology. Device enumeration tests (`device-enumeration-test`) run against the actual
target environment (emulator or physical hardware) to ensure that the driver tree initializes
successfully without crashes, hangs, or regressions.

While unit tests and
[DriverTestRealm](/docs/development/drivers/testing/driver_test_realm.md) verify
individual drivers or isolated component realms, device enumeration tests validate the integrated
system. They confirm that your driver binds to its parent nodes, publishes expected child nodes,
and allows composite devices to assemble properly on the board.

## Overview of device enumeration tests

Device enumeration tests connect to the `fuchsia.driver.development/Manager` (`GetNodeInfo`) FIDL
protocol to inspect the tree of device nodes in the driver topology during system boot.

Each target board (`aemu_x64`, `qemu_x64`, `vim3`, `nelson`, as well as vendor-specific boards)
defines a list of required node monikers that must be present after driver discovery completes.
If a required device node fails to enumerate within a timeout, the test fails, indicating that a
driver in the stack failed to bind, crashed, or encountered an error.

## Discovering available device enumeration tests

You can list the available device and driver host enumeration tests in your environment by running
`fx test` with the `--dry` flag:

```posix-terminal
fx test --dry device-enumeration
```

```posix-terminal
fx test --dry driver-host-enumeration
```

## Running device enumeration tests

Before running a device enumeration test, ensure your active build directory (`fx use`) and target
device (`fx set-device` or `fx -t <device> test ...`) match the board under test.

Depending on whether your target board includes the enumeration test as a **packaged test** or a
**bootfs test**, use one of the following two primary execution methods:

### 1. Running packaged tests (`fuchsia_unittest_package`) using `fx test`

If the test is built as a standard component package (`fuchsia_unittest_package`) against a
networked build (such as a `minimal` or `workbench` product where package serving and SSH are
active), execute it dynamically over package serving using `fx test`:

1.  If the test package target is not yet in your active build configuration (`tests.json`), add it
    using `fx add-test`:

    ```posix-terminal
    fx add-test //path/to/test:device-enumeration-test-myboard
    fx build
    ```

2.  Run the test using `fx test` with `--package` and `-o` (or specifying the full URL):

    ```posix-terminal
    fx test --package device-enumeration-test-myboard -o
    ```

    ```posix-terminal
    PKG="device-enumeration-test-myboard"
    fx test "fuchsia-pkg://fuchsia.com/${PKG}#meta/${PKG}.cm" -o
    ```

Note: Passing `--package` is critical when both the `bootfs_test` binary and the component
package coexist in `tests.json` (such as on `minimal` builds). Running the test by name matches
both targets and attempts to run the `bootfs` binary (`expects_ssh: False`) locally on your host
workstation, producing a false `FAILED: /boot/test/...` result. The `--package` flag instructs
`fx test` to filter out `bootfs` entries and run only the `.cm` package over RCS.

Tip: You can include the `-o` (or `--output`) argument when running `fx test` during driver
development (for example, `fx test --package device-enumeration-test-vim3 -o`). This displays the
test's standard output directly in your terminal, which lets you inspect detailed logs and see
exactly which device node monikers passed or timed out.

### 2. Running bootfs tests (`bootfs_test`) directly over SSH or serial

If your target board configures the enumeration test inside `bootfs` (for example, `/boot/test/...`),
do not run `fx test /boot/test/...` directly without flags because `bootfs_test` targets do not run
over standard SSH test runner pipelines by default.

Instead, execute the `/boot/test/` binary directly on the booted target:

*   **Execution over SSH (`minimal` builds)**: If your target is running a networked build
    (such as `minimal.<board>`) that embeds the binary inside `bootfs`, run the binary directly
    on the booted device over SSH:

    ```posix-terminal
    ffx target ssh "/boot/test/device-enumeration-test-myboard-bin"
    ```

*   **Execution over serial (`bringup` builds)**: For early boot testing on `bringup` images, use
    the preconfigured `bringup_with_tests.<board>` product bundle (for example,
    `bringup_with_tests.iris`), which automatically embeds the `bootfs` test binaries without
    requiring manual assembly overrides (`--assembly-override`). Because `bringup` images operate
    without network discovery or a package server, execute the binary inside `fx serial`.

### 3. Running driver host enumeration tests (`driver-host-enumeration-test`)

In addition to verifying device node monikers (`device-enumeration-test`), boards define driver
host enumeration tests (`driver-host-enumeration-test`) to verify that drivers on the board are
grouped into driver hosts as expected (`*_host_golden.json`).

Because driver host enumeration tests connect over FIDL (`fuchsia.driver.development/Manager`) to
query the active driver hosts running on the target device during boot (`GetDriverHostInfo`), your
active build directory (`fx use`) and configured target device (`fx set-device`) must match the
board under test (such as `iris.minimal`).

To run a driver host enumeration test against a live target device (such as on the `iris` board):

1.  Switch to the board's build directory and set your target device:

    ```posix-terminal
    fx use out/iris.minimal
    fx set-device <device-name>
    ```

2.  If the test target is not yet in your active build configuration (`tests.json`), add it using
    `fx add-test` and build:

    ```posix-terminal
    fx add-test //vendor/google/iris/board/drivers/iris:driver-host-enumeration-test-iris
    fx build
    ```

3.  Run the test using `fx test` with `--package` and `-o`:

    ```posix-terminal
    fx test --package driver-host-enumeration-test-iris -o
    ```

### Automated verification in CQ

When uploading driver changes or new board configurations for code review in Gerrit, you can verify
device enumeration automatically in continuous integration by selecting board-specific bringup
tryjob builders (for example, `bringup.iris-debug`) using **Choose Tryjobs**.

## Adding an existing enumeration test to a build using assembly developer overrides

When a device enumeration test (or bootfs test package) already exists in the codebase (for
example, `//vendor/google/iris/enumeration:bootfs_test_files`), you can include it in your local
build without modifying in-tree product targets by using `assembly_developer_overrides()`.

1.  Define the assembly override in a local `BUILD.gn` file (typically `//local/BUILD.gn`):

    ```gn
    import("//build/assembly/developer_overrides.gni")

    assembly_developer_overrides("my_enumeration_overrides") {
      bootfs_files_labels = [
        "//vendor/google/iris/enumeration:bootfs_test_files",
      ]
    }
    ```

2.  Apply the override when configuring your build with `fx set`:

    ```posix-terminal
    fx set bringup.iris --assembly-override //local:my_enumeration_overrides
    fx build
    ```

When the `bringup.iris` product bundle is assembled, the specified bootfs test files are
automatically embedded directly into the bootfs image, which lets you run the test over serial
(`fx serial`).

## When and how to update device enumeration tests

You must update the board's device enumeration test whenever you make changes that alter the node
topology of a board, such as:

*   Adding a new driver or device to a board.
*   Renaming existing node monikers or changing node properties.
*   Migrating a driver from DFv1 to DFv2 where node names or parent-child topologies change.

### Test source locations

*   **In-tree board tests**: The source definitions and expected node lists for standard in-tree
    boards are located in
    [`//zircon/system/utest/device-enumeration/`](/zircon/system/utest/device-enumeration).
    For example, see `boards/aemu_x64.cc` and `boards/vim3.cc`.
*   **Vendor board tests**: For vendor-specific or out-of-tree boards, enumeration test definitions
    live inside their respective vendor repositories (for example, under
    `//vendor/google/<board>/enumeration/`).

When modifying an enumeration test, add your new node monikers to the `kNodeMonikers` array in the
corresponding board test file so that automated verification catches any future regressions in your
driver stack.

### Updating driver host enumeration golden files

When adding, removing, or relocating drivers across driver hosts (or changing collocation
properties in cml/bind files), the expected driver host groupings (`ExpectedDriverHost`) change.
When modifying a board's driver host topology, update the corresponding golden JSON file
(`*_host_golden.json`) alongside driver changes to keep verification passing.
