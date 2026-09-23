# C++ driver rubric

## Writing a basic driver

The code structure should follow the `fx` [create driver goldens][goldens]
template.

### Meta directory

Every driver directory is required to have a `meta` subdirectory containing
these files:

* A `bind` file defining the driver's `bind` rules
* A [component manifest][component-manifest]

### Driver source code

Write drivers using the [Driver Component library][driver-component]
(`sdk/lib/driver/component/`). Drivers should inherit from the
[fdf::DriverBase2][driver-base] class defined in the header
`<lib/driver/component/cpp/driver_base2.h>`.

The driver provides the `FUCHSIA_DRIVER_EXPORT2` macro used to export the
driver symbol which is defined in the header
 `<lib/driver/component/cpp/driver_export2.h>`. This macro should be located
 in a `.cc` file.

### Initializing a driver

Avoid placing any logic within the driver's constructor. Rather than using a
constructor, you must implement the driver’s initialization logic by overriding
exactly *one* of the `Start()` methods provided by
[fdf::DriverBase2][driver-base], even if the initialization logic is empty.
Ensure that only a single `Start()` variant is used for this purpose.

The initialization logic contains:

* Fetching and configuring all driver resources.
* Establishing service connections.
* Adding the driver's own service to the outgoing directory.
* Add child nodes (to be performed after resource setup and providing its own
  service).
* Release BTI from quarantine with `zx_bti_release_quarantine()`.

### Shutting down a driver

The driver's destructor should remain free of any logic. In the majority of
scenarios, overriding and providing implementation for `DriverBase2::Stop()` is
unnecessary. Refrain from implementing `DriverBase2::Stop()` unless it is
specifically required for your driver's functionality, such as performing
graceful hardware shutdowns or unpinning DMA. The `fdf::StopCompleter` in the
function parameter must be called at the end of the function.

## Build file

All new drivers must use Bazel for the build process and must include the
following targets:

* `fuchsia_driver_bind_bytecode`
* `fuchsia_cc_driver`
* `fuchsia_driver_component`

## Driver communication

Drivers communicate with their parent drivers through FIDL services.

### Serving a service

Drivers serving FIDL services are required to maintain a
`fidl::ServerBindingGroup` (when using Zircon transport) or an
`fdf::ServerBindingGroup` (for the driver transport). Unless the driver must
be restricted to a single client connection, avoid using `fidl::ServerBinding`
or `fdf::ServerBinding`.

The server bindings must be added to the driver’s outgoing directory within the
`Start()` function before any child nodes are created. Furthermore, the CML
file must specify the service in the capability and expose it from `self`.

### Using a service

To use a service, drivers should include it in their `bind` rules and specify
it within the `uses` section of the CML.

## Adding a child

The primary reason for adding a child node is if the driver needs to provide
services or resources for another driver. Avoid adding a child without reason.
Unless the driver needs to add child nodes dynamically, the child node should
be added as part of the initialization logic in the driver’s `Start()`
function.

Child nodes should be added using `DriverBase2`'s `AddChild()` or the
[add_child][add-child] helper library (`sdk/lib/driver/node/cpp/add_child.h`).
The helper library should only be used if the child node needs to use a
different logger.

All child nodes should be unowned unless it’s being used to support `devfs`,
which is deprecated. If the child node is owned, then the driver must store the
`fuchsia_driver_framework::Node` client end that it receives from the helper
functions.

When adding a child node, explicitly define the child node’s name instead of
using `name()` from `DriverBase2`. Create the offers with the
[node_offers helper library][node-offers]
(`sdk/lib/driver/component/cpp/node_offers.h`). Create the properties using the
[node_properties library][node-properties]
(`sdk/lib/driver/component/cpp/node_properties.h`). The node property key and
values should use the generated bind library code bindings.

## Logging

For logging, drivers should use format-based log APIs like `fdf::info()` found
in the [driver logger library][driver-logger]
(`sdk/lib/driver/logging/cpp/logger.h`). If you need to log a custom type,
implement the `std::format`.

Follow the [Fuchsia logging guidelines][logging-guidelines] by using `warning`
or `error` log levels when documenting failures such as FIDL errors.

## Zircon resources

### Interrupts

Interrupts should be retrieved and stored in the driver during initialization.
The interrupts should be wrapped by an `async::Irq` object
(`sdk/lib/async/include/lib/async/cpp/irq.h`).

An `IrqMethod` object should be used to handle interrupt triggers. Once the
interrupt trigger is handled, the `Irq` object should be acknowledged with
`ack()` so the interrupt is re-armed and can be triggered again.

### DMA

For memory operations, drivers should use [MmioBuffer][mmio-buffer], located at
`sdk/lib/driver/mmio/cpp/mmio-buffer.h`. This library provides a wrapper around
the raw `mmio_block_t` object for reading and writing.

To ensure safe access to register bitfields, it is recommended to use the
[hwreg/bitfields library][bitfields]. This library is found at
`zircon/system/ulib/hwreg/include/hwreg/bitfields.h`.

The driver should take control of its BTI and stop any DMA which might be
ongoing during initialization. Once that is complete, it should tell the BTI
that it has regained control of the hardware by calling
`zx_bti_release_quarantine()` on it. Drivers that intend to share DMA with
hardware are required to pin the BTI beforehand. After the hardware has
finished accessing the memory, the driver is responsible for unpinning it.
Under no circumstances should the BTI be unpinned within the destructor.

### Clocks

Clocks are controlled via the [fuchsia.hardware.clock FIDL service][clock-fidl].
Drivers are required to call `Enable()` on all clocks they depend on and
subsequently call `Disable()` once the clock signal is no longer needed.
Drivers must not call `Disable()` without first enabling the clock.

## Testing

Verify that build targets include the necessary driver tests.

### Unit tests

Unit tests should be written using [gtests][gtests]
(`third_party/googletest/src/googletest/include/gtest/gtest.h`).

#### Testing the entire driver

If the entire driver is being tested, the driver should be wrapped by
`ForegroundDriverTest` or `BackgroundDriverTest` from the
[driver test library][driver-test]
(`sdk/lib/driver/testing/cpp/driver_test.h`). The unit test should call
`StartDriver()` right after the test is initialized and call `StopDriver()` in
the `Teardown()` function. All services and resources needed by the driver
should be initialized and served in the custom driver test’s `Environment`
class.

If the driver requires a platform device service, the test should instantiate a
[FakePlatformDevice][fake-pdev]
(`sdk/lib/driver/fake-platform-device/`) `DriverTestEnvironment` and serve it.
Similarly, if the driver requires a FIDL service, a test implementation of the
service should be instantiated and served in the `DriverTestEnvironment`.

#### Testing parts of the driver

Tests may require specific driver environment components if only a subset of the
driver's logic is being isolated for testing.

When evaluating logic that utilizes the [driver logger library][driver-logger]
(`sdk/lib/driver/logging/cpp/logger.h`), the test is required to instantiate
and maintain a `fdf_testing::ScopedGlobalLogger` instance
(`sdk/lib/driver/testing/cpp/scoped_global_logger.h`).

Furthermore, if a driver dispatcher is necessary for the logic, the test must
configure and hold a `fdf_testing::DriverRuntime` object
(`sdk/lib/driver/testing/cpp/driver_runtime.h`).

#### Fakes/mocks libraries

Tests requiring fake FIDL services or resources should utilize the fake and
mock libraries provided in `sdk/lib/driver/` whenever they are available.

The following libraries are available for common FIDL services:

* [//sdk/lib/driver/fake-clock][fake-clock] - `fuchsia.hardware.clock` service.
* [//sdk/lib/driver/fake-gpio][fake-gpio] - `fuchsia.hardware.gpio` service.
* [//sdk/lib/driver/fake-interconnect][fake-interconnect] -
  `fuchsia.hardware.interconnect` service.
* [//sdk/lib/driver/fake-pin][fake-pin] - `fuchsia.hardware.pin` service.
* [//sdk/lib/driver/fake-platform-device][fake-pdev] -
  `fuchsia.hardware.platform.device` and `fuchsia.hardware.power` services.
* [//sdk/lib/driver/fake-reset][fake-reset] - `fuchsia.hardware.reset` service.
* [//sdk/lib/driver/fake-vreg][fake-vreg] - `fuchsia.hardware.vreg` service.

The following libraries are available for common internal Zircon objects,
memory regions, or driver-runtime resources:

* [//sdk/lib/driver/fake-bti][fake-bti]: Fakes Zircon BTI (`zx::bti`) objects.
* [//sdk/lib/driver/fake-object][fake-object]: Fakes general Zircon kernel
  objects (`zx::handle`).
* [//sdk/lib/driver/fake-resource][fake-resource]: Fakes Zircon resource
  (`zx::resource`) objects.
* [//sdk/lib/driver/fake-mmio-reg][fake-mmio-reg]: Fakes MMIO registers via
  memory structures.
* [//sdk/lib/driver/mock-mmio][mock-mmio]: Mocks MMIO regions for
  memory-mapped I/O register testing.

### Integration tests

Integration tests should use the [Driver Test Realm][dtr]
(`sdk/lib/driver_test_realm/`) framework. To begin, the test must establish a
connection to the `DriverTestRealm` service, perform necessary configuration in
`RealmArgs`, and then call `Start()` with the arguments.

[goldens]: /tools/create/goldens/my-driver-cpp/
[component-manifest]: /docs/concepts/components/v2/component_manifests.md
[driver-component]: /sdk/lib/driver/component/
[driver-base]: /sdk/lib/driver/component/cpp/driver_base2.h
[add-child]: /sdk/lib/driver/node/cpp/add_child.h
[node-offers]: /sdk/lib/driver/component/cpp/node_offers.h
[node-properties]: /sdk/lib/driver/component/cpp/node_properties.h
[driver-logger]: /sdk/lib/driver/logging/cpp/logger.h
[logging-guidelines]: /docs/contribute/governance/rfcs/0003_logging.md#log_severity_levels
[mmio-buffer]: /sdk/lib/driver/mmio/cpp/mmio-buffer.h
[bitfields]: /zircon/system/ulib/hwreg/include/hwreg/bitfields.h
[clock-fidl]: /sdk/fidl/fuchsia.hardware.clock/clock.fidl
[gtests]: /third_party/googletest/src/googletest/include/gtest/gtest.h
[driver-test]: /sdk/lib/driver/testing/cpp/driver_test.h
[fake-pdev]: /sdk/lib/driver/fake-platform-device/
[fake-clock]: /sdk/lib/driver/fake-clock
[fake-gpio]: /sdk/lib/driver/fake-gpio
[fake-interconnect]: /sdk/lib/driver/fake-interconnect
[fake-pin]: /sdk/lib/driver/fake-pin
[fake-reset]: /sdk/lib/driver/fake-reset
[fake-vreg]: /sdk/lib/driver/fake-vreg
[fake-bti]: /sdk/lib/driver/fake-bti
[fake-object]: /sdk/lib/driver/fake-object
[fake-resource]: /sdk/lib/driver/fake-resource
[fake-mmio-reg]: /sdk/lib/driver/fake-mmio-reg
[mock-mmio]: /sdk/lib/driver/mock-mmio
[dtr]: /sdk/lib/driver_test_realm/