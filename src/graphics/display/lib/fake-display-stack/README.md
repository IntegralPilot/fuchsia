# Fake Display Stack

The **Fake Display Stack** is a driver-like library providing a display engine
(`fake-display`) and a Display Coordinator implementation within a hermetic
testing environment. It serves a [`fuchsia.hardware.display/Service`] service
instance independent of the one provided by the system Display Coordinator and
display engine drivers. The fake display engine may use a fake sysmem instance,
in which case tests won't impact real sysmem resources, such as the continuous
memory pool.

Strictly speaking, the fake display stack is not a driver. It doesn't bind to
device nodes nor run on driver hosts. However, its behavior is similar to the
production display engine and coordinator drivers. It runs on driver dispatchers
provided by the [driver runtime][driver-runtime] with the same threading model
as the drivers running in production under
[Driver Framework v2][driver-framework].

## Usage

The fake display stack is a test double of the display engine and coordinator
drivers. We can use it either as a standalone component or as a library.

### Run the `fake-display-stack-host` component

[`fake-display-stack-host`][fake-display-stack-host] is a standalone component
hosting a fake display stack in a driver-like runtime environment. It exposes
the [`fuchsia.hardware.display/Service`] service to other components over the
component framework.

See the `fake-display-stack-host` [README doc][fake-display-stack-host-readme]
for more details.

### Import `FakeDisplayStack` as a library

Tests may use the `FakeDisplayStack` library if they need to peek into the
internal state of the display engine driver, or if they need to bring up their
own Sysmem service.

Tests using the `FakeDisplayStack` library must manually manage its lifecycle.
Typically, a test initializes a `FakeDisplayStack` during `SetUp()` and shuts it
down during `TearDown()` for each test case.

Example:

- [Display Coordinator integration test fixture][display-coordinator-integration-test-fixture]

[driver-framework-v2]: /docs/concepts/drivers/driver_framework.md
[display-coordinator]: /src/graphics/display/drivers/coordinator/
[display-coordinator-integration-test-fixture]: /src/graphics/display/drivers/coordinator/testing/base.h
[driver-runtime]: /docs/concepts/drivers/driver_framework.md#driver_runtime
[fake-display-stack-host]: /src/graphics/display/testing/fake-display-stack-host/
[fake-display-stack-host-readme]: /src/graphics/display/testing/fake-display-stack-host/README.md
[fake-display]: /src/graphics/display/lib/fake-display-stack/
[flatland-display-compositor-smoketest]: /src/ui/scenic/lib/flatland/engine/tests/smoketests/display_compositor_smoketest.cc
[realm-builder]: /docs/development/testing/components/realm_builder.md
[scenic]: /docs/concepts/ui/scenic/index.md
[sysmem]: /src/sysmem/server/
