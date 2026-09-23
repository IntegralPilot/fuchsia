# User guide for Lacewing

This page provides best practices, examples, and reference materials for using
Fuchsia's Lacewing framework for writing host-driven tests (also known as
end-to-end tests or system tests).


## Vision

Lacewing is Fuchsia's unified host-driven testing framework. It's designed to
serve users both in-tree and out-of-tree.

At a high level, Lacewing aims to:

*   Eliminate host-driven test framework fragmentation

*   Improve host-driven test stability and reliability

*   Centralize host-target interaction to versioned and officially supported
    APIs

*   Lower the barrier of entry for host-driven test development

## Getting started

See [Getting Started][getting-started].

## Example tests

The Fuchsia repository contains several example tests in
[`//src/testing/end_to_end/examples/`][examples-dir] demonstrating different
Lacewing capabilities:

*   **[Hello World Test][hello-world-test]**: Demonstrates basic test
    structure, DUT connection, and built-in health checks.

*   **[FIDL Hello World Test][fidl-hello-world-test]**: Demonstrates direct FIDL
    protocol communication with on-device services using Fuchsia Controller.

*   **[Data Resource Access Test][data-resource-access-test]**: Demonstrates
    packaging and reading custom test data files at runtime.

*   **[Test Cases Example][test-cases-example]**: Demonstrates structuring
    reusable test logic using `FuchsiaTestCases`.

*   **[Soft Reboot Test][soft-reboot-test]**: Demonstrates rebooting a Fuchsia
    device and verifying recovery.

*   **[Power Cycle Test][power-cycle-test]**: Demonstrates hard power cycling
    via auxiliary hardware (DMC/PDU).

*   **[Multi-Device Test][multi-device-test]**: Demonstrates coordinating tests
    across multiple devices in a testbed.

To run example tests locally on a Fuchsia emulator:

```posix-terminal
# Include example tests in build configuration
fx set core.x64 --with //src/testing/end_to_end/examples

# Start emulator (if not already running)
ffx emu start --headless

# Run specific example test
fx test //src/testing/end_to_end/examples/hello_world_test --e2e --output
fx test //src/testing/end_to_end/examples/fidl_hello_world_test --e2e --output
```

For more detailed setup and execution instructions, see the
[Examples README][examples-readme].

## Coding guidelines

See [Honeydew Contribution][honeydew-contribution].

<!-- Reference links -->

[getting-started]: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/src/testing/end_to_end/README.md#getting-started-30-mins
[examples-dir]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/examples/
[hello-world-test]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/examples/hello_world_test/
[fidl-hello-world-test]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/examples/fidl_hello_world_test/
[data-resource-access-test]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/examples/data_resource_access_test/
[test-cases-example]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/examples/test_cases_example/
[soft-reboot-test]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/examples/test_soft_reboot/
[power-cycle-test]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/examples/power_cycle_test/
[multi-device-test]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/examples/multi_device_test_sl4f/
[examples-readme]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/examples/README.md
[honeydew-contribution]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/honeydew#Contributing
