# DFv2 driver development documentation

The documentation in this section is designed to help driver developers create
new drivers using Fuchsia's [driver framework version 2][dfv2] (DFv2).

## How-to {:#how-to}

- [**Write a minimal DFv2 driver**][write-a-minimal-dfv2-driver]: Learn how to
  create a minimal DFv2 driver from scratch.
- [**Driver examples**][driver-examples]: Explore sample drivers intended to
  demonstrate various Fuchsia driver concepts.

## Tutorials {:#tutorials}

- [**Create a composite node**][composite-node]: Learn how to create a
  composite node using composite node specifications.
- [**Bind rules tutorial**][bind-rules-tutorial]: Learn how to write bind rules
  for DFv2 drivers to discover and match devices.
- [**Bind library code generation tutorial**][bind-libraries-codegen]: Learn
  how to use bind libraries to auto-generate code for DFv2 drivers.
- [**FIDL tutorial**][fidl-tutorial]: Learn how to define FIDL protocols,
  export them from a driver, and use them in another driver.
- [**Metadata tutorial**][metadata-tutorial]: Learn how to pass metadata from
  one driver to another using the metadata library in DFv2.

## Debugging {:#debugging}

- [**Troubleshoot common issues in DFv2 driver development**][troubleshoot]:
  Learn how to debug and fix common errors in DFv2 driver development.
- [**Driver utilities**][driver-utils]: Learn how to use driver utilities for
  communicating with devices for diagnostics.

## Testing {:#testing}

- [**DriverTestRealm**][driver-test-realm]: Learn how to use the
  `DriverTestRealm` framework for running driver integration tests.
- [**Device enumeration tests**][device-enumeration]: Learn how to run device
  enumeration tests across boards to verify full driver topology integration.
- [**Threading tips in tests**][treading-tips]: Understand best practices for
  handling threading in driver tests to avoid crashes.

## Best practices {:#best-practices}

- [**VMO registration pattern**][vmo-pattern]: Understand approaches for
  transferring bulk data between applications and drivers.
- [**Driver stack performance**][driver-stack-performance]: Understand
  performance-related best practices for authoring new device driver APIs.

## Guidelines {:#guidelines}

- [**Driver runtime API guidelines**][driver-runtime-api]: Follow these
  guidelines when defining C APIs in the driver runtime.
- [**Driver C++ rubric**][driver-cpp-rubric]: Follow these rules when creating a
  new C++ driver in the Fuchsia source repository.
- [**Driver Rust rubric**][driver-rust-rubric]: Follow these rules when creating a
  new Rust driver in the Fuchsia source repository.

## Concepts {:#concepts}

- [**DMA (Direct Memory Access)**][dma]: Understand DMA and its importance in
  DFv2 driver development.
- For more information on DFv2-specific concepts, see [Drivers][dfv2]
  under the **Fundamentals** section.

## Additional information {:#additional-information}

See the following tutorials under the **SDK** section:

- [**Codelab: Build a driver**][driver-codelab]: Learn how to create a DFv2
  driver from scratch in the Fuchsia SDK environment.
- [**Write bind rules for a driver**][write-bind-rules]: Learn how to write
  bind rules for DFv2 drivers in the Fuchsia SDK environment.
- [**Driver unit testing quick start**][driver-unit-testing]: Learn how to
  write unit tests for DFv2 drivers in the Fuchsia SDK environment.
- [**View driver information**][view-driver-info]: Learn how to use the
  `ffx driver` command to retrieve information about drivers.

<!-- Reference links -->

[dfv2]: /docs/concepts/drivers/README.md
[write-a-minimal-dfv2-driver]: /docs/development/drivers/developer_guide/write-a-minimal-dfv2-driver.md
[driver-examples]: /docs/development/drivers/developer_guide/driver-examples.md
[composite-node]: /docs/development/drivers/developer_guide/create-a-composite-node.md
[fidl-tutorial]: /docs/development/drivers/tutorials/fidl-tutorial.md
[bind-rules-tutorial]: /docs/development/drivers/tutorials/bind-rules-tutorial.md
[bind-libraries-codegen]: /docs/development/drivers/tutorials/bind-libraries-codegen.md
[metadata-tutorial]: /docs/development/drivers/tutorials/metadata-tutorial.md
[troubleshoot]: /docs/development/drivers/developer_guide/troubleshoot-common-issues.md
[driver-utils]: /docs/development/drivers/diagnostics/driver-utils.md
[driver-test-realm]: /docs/development/drivers/testing/driver_test_realm.md
[device-enumeration]: /docs/development/drivers/testing/device_enumeration.md
[treading-tips]: /docs/development/drivers/testing/threading-tips-in-tests.md
[vmo-pattern]: /docs/development/drivers/best_practices/vmo-registration-pattern.md
[driver-stack-performance]: /docs/development/drivers/best_practices/driver_stack_performance.md
[driver-runtime-api]: /docs/development/drivers/developer_guide/driver-runtime-api-guidelines.md
[driver-cpp-rubric]: /docs/development/drivers/developer_guide/cpp-rubric.md
[driver-rust-rubric]: /docs/development/drivers/developer_guide/rust-rubric.md
[dma]: /docs/development/drivers/concepts/driver_development/dma.md
[driver-codelab]: /docs/development/drivers/tutorials/sdk_build_driver/introduction.md
[write-bind-rules]: /docs/development/sdk/write-bind-rules-for-driver.md
[driver-unit-testing]: /docs/development/sdk/driver-testing/driver-unit-testing-quick-start.md
[view-driver-info]: /docs/development/tools/ffx/workflows/view-driver-information.md
