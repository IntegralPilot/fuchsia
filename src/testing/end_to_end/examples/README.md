# Example Lacewing tests

This directory contains example Mobly tests demonstrating different capabilities
of the Lacewing test framework.

[TOC]

## Setup

1. Check that a device or emulator is running and detectable by FFX:

   ```posix-terminal
   ffx target list
   ```

   This command prints output similar to the following:

   ```none {:.devsite-disable-click-to-copy}
   NAME                SERIAL       TYPE       STATE      ADDRS/IP             RCS
   fuchsia-emulator*   <unknown>    core.x64   Product    [127.0.0.1:38101]    Y
   ```

   To start a headless emulator:

   ```posix-terminal
   ffx emu start --headless
   ```

2. Add the example tests to your build configuration:

   ```posix-terminal
   fx set core.x64 --with //src/testing/end_to_end/examples
   ```

## Running standard tests

The following tests run on any standard Fuchsia emulator or device without
requiring specialized hardware or custom configuration:

* **`hello_world_test`**: Demonstrates basic test structure, DUT connection,
  and health checks.

* **`fidl_hello_world_test`**: Demonstrates direct FIDL protocol communication
  via Fuchsia Controller (`fuchsia.buildinfo.Provider`).

* **`data_resource_access_test`**: Demonstrates accessing custom test data
  files at runtime.

* **`test_cases_example`**: Demonstrates structuring reusable test logic using
  `FuchsiaTestCases`.

To run any of these tests:

```posix-terminal
fx test //src/testing/end_to_end/examples/<test_name> --e2e --output
```

For example:

```posix-terminal
fx test //src/testing/end_to_end/examples/hello_world_test --e2e --output
fx test //src/testing/end_to_end/examples/fidl_hello_world_test --e2e --output
```

You can also run all emulator-compatible example tests together:

```posix-terminal
fx test //src/testing/end_to_end/examples:emulator_tests --e2e --output
```

## Tests requiring special setup

Some example tests demonstrate advanced hardware features or workflows that
require specific configurations:

### Soft reboot test (`test_soft_reboot`)

Tests rebooting the device and validating recovery. When testing on an emulator,
persistent tap networking is required:

```posix-terminal
ffx emu stop
ffx emu start --headless --net tap
fx test //src/testing/end_to_end/examples/test_soft_reboot:soft_reboot_test --e2e --output
```

Available target variants in `test_soft_reboot`:

* `soft_reboot_test`: Default Fuchsia Controller transport.

* `soft_reboot_test.hermetic`: Hermetic session variant.

* `soft_reboot_test_with_ffx_monitor`: Monitored session variant. Requires an
  active `ffx monitor` daemon in the environment (`use_monitor_state = true`).

* `test_soft_reboot_sl4f`: Legacy SL4F transport variant. Requires the target
  system image to include SL4F packages (`start_sl4f`).

### Power cycle test (`power_cycle_test`)

Demonstrates hard power cycling via auxiliary hardware (DMC or PDU). Requires a
hardware testbed and a custom
[Mobly Config YAML file](../README.md#mobly-config-yaml-file).

```posix-terminal
fx test //src/testing/end_to_end/examples/power_cycle_test --e2e --output
```

### OpenWrt access point test (`openwrt_ap_test`)

Demonstrates interacting with and controlling an OpenWrt Access Point auxiliary
device in the testbed. Requires an OpenWrt AP device and a custom Mobly config
YAML.

```posix-terminal
fx test //src/testing/end_to_end/examples/openwrt_ap_test --e2e --output
```

### Multi-device test (`multi_device_test_sl4f`)

Demonstrates coordinating tests across multiple Fuchsia devices in a single
testbed.
See the [Multi-Device Test README](multi_device_test_sl4f/README.md) for testbed
setup.

```posix-terminal
fx test //src/testing/end_to_end/examples/multi_device_test_sl4f --e2e --output
```

### Revive test cases (`test_case_revive_example` / `test_cases_revive_example`)

Demonstrates reviving test cases after lifecycle operations (such as soft
reboots or idle suspend/resume) using the `@tag_test` decorator. Requires a
product configuration with revive support (for example, `workbench_eng.x64`):

```posix-terminal
fx set workbench_eng.x64 --with //src/testing/end_to_end/examples

# Run without reviving test cases
fx test //src/testing/end_to_end/examples/test_case_revive_example:run_wo_test_case_revive --e2e --output

# Run with Soft-Reboot revive operation
fx test //src/testing/end_to_end/examples/test_case_revive_example:test_case_revive_with_soft_reboot --e2e --output

# Run with Hard-Reboot revive operation (requires hardware testbed)
fx test //src/testing/end_to_end/examples/test_case_revive_example:test_case_revive_with_hard_reboot --e2e --output

# Run FuchsiaTestCases + TestCaseRevive example
fx test //src/testing/end_to_end/examples/test_cases_revive_example --e2e --output
```

## Infrastructure canaries

* **`exoneration_failing_test`**: An intentionally failing test case used
  exclusively by Continuous Integration infrastructure to validate test failure
  exoneration and flake detection pipelines.
