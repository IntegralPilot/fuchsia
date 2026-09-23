# Rust driver rubric

## Writing a basic driver

The code structure should follow the `fx` [create driver goldens][goldens]
template.

### Meta directory

Every driver directory is required to have a `meta` subdirectory containing
these files:

* A `bind` file defining the driver's `bind` rules
* A [component manifest][component-manifest]

### Driver source code

The Rust driver source code should be in a `src` subdirectory in a file called
`lib.rs`. The Rust source code should use the [fdf\_component library][fdf-component]
to define the driver.

The module structure should use a flat file module landing (e.g., `src/foo.rs`)
instead of `mod.rs` in a folder (e.g., `src/foo/mod.rs`). Submodules
should be placed in a sibling folder (e.g., `src/foo/bar.rs`).

The driver must be defined as a struct that implements the
[fdf\_component::Driver trait][driver-trait]. The implemented
[start method][start-method] receives a [DriverContext][driver-context] struct
which contains structures necessary to connect to and serve protocols and logs
for the driver. Additionally, the driver code must use the `driver_register!()`
macro to register the driver with the Driver Framework.

### Component manifest

The component manifest should not declare the main dispatcher to
`allow_sync_calls` since Rust drivers must be asynchronous.

### Initializing a driver

All initialization logic should be placed in the [start method][start-method]
and must store the Node handle from the [DriverContext][driver-context] to the
`Driver` struct. The initialization logic contains:

* Take (using [DriverContext::take\_node][take-node]) and store the
  [Node][node-struct] object for the driver until shutdown. Dropping the `Node`
  object will cause the driver to be shut down, intentionally or not. In most
  cases this will be unused, but should be stored in the `Driver` object anyways
  (as `_node` to silence the warning about it being unused). This will correctly
  drop the `Node` when the driver shuts down.
* Fetching and configuring all driver resources.
* Establishing service connections.
* Adding the driver's own service to the outgoing directory.
* Add child nodes (to be performed after resource setup and providing own
  service).
* Release BTI from quarantine.

### Shutting down a driver

In the majority of scenarios, `Driver` `stop()` implementation is empty. Refrain
from adding more to it unless explicit clean up specifically required for your
driver's functionality, such as performing graceful hardware shutdowns or
unpinning DMA.

If a driver controls when it should be shut down, it should store the
[Node][node-struct] object as something that can be dropped, like in an
`Option<Node>` or potentially behind a mutex if necessary. Dropping the `Node`
object will start the driver shutting down.

## Build file

All Rust drivers must use GN for the build process and must include the
following targets:

* `fuchsia_driver_bind_bytecode`
* `fuchsia_rust_driver`
* `fuchsia_driver_component`

All Rust driver targets are encouraged to enforce strict lints by including the
following configs in their GN definition:

```gn
configs += [
  "//build/config/rust/lints:clippy_warn_all",
]
```

## Driver communication

Drivers communicate with their parent drivers through FIDL services.

### Serving a service

Use [examples/drivers/transport/driver/rust\_next/parent][rust-next-parent] and
[examples/drivers/transport/zircon/rust\_next/parent/][zircon-next-parent] as
guidance for using the `rust_next` bindings, which Fuchsia *recommends* for
normal FIDL transport services, and *requires* for driver transport services (as
the old bindings do not support it).

Drivers serving FIDL services must implement the FIDL service as a trait. During
initialization, it needs to add a `ServiceOffer` in an outgoing directory and
then serve the directory to the `DriverContext` with the `serve_outgoing()`
function. Once the directory is served, spawn a task with the
[fuchsia\_async Scope library][fasync-scope] to run the `ServiceFs` event loop
in.

The CML file must specify the service in the capability and expose it from
`self`.

### Using a service

Use [examples/drivers/transport/driver/rust\_next/child][rust-next-child] and
[examples/drivers/transport/zircon/rust\_next/child/][zircon-next-child] as
guidance for using the `rust_next` bindings, which Fuchsia *recommends* for
normal FIDL transport services, and *requires* for driver transport services (as
the old bindings do not support it).

To use a service, drivers should include it in their `bind` rules and specify it
within the `uses` section of the CML. When connecting to services, use the
service capability instead of the protocol.

## Adding a child

The primary reason for adding a child node is if the driver needs to provide
services or resources for another driver. Avoid adding a child without reason.
Unless necessary, the child node should be added as part of the initialization
logic in the `start()` function.

Child nodes should be added using the Rust wrapper
[`Node::add_child`][node-add-child]. All child nodes should be unowned unless
it’s being used to support `devfs`, which is unsupported in Rust drivers.
Therefore, there should be no owned children.

## Logging

Use the standard [Rust logging API][rust-logging] for all logs. Follow the
[Fuchsia logging guidelines][logging-guidelines] by using `warning` or `error`
log levels when documenting failures such as FIDL errors.

## Zircon resources

Use the Zircon kernel bindings for resources such as `Vmo`, and `Interrupt`.

### Interrupts

Avoid storing interrupts as raw `zx::Handle` objects, as this necessitates using
`unsafe` code during wait operations. Instead, define your device struct as
generic over K: [zx::InterruptKind][interrupt-kind] (for example, `Device<K>`)
using the [Zircon interrupt bindings][zircon-interrupt]
([sdk/rust/zx/src/interrupt.rs][zircon-interrupt-source]) and use it to wrap and
store the handle within a [zx::Interrupt][zx-interrupt]\<K\> object.

Use [fasync::OnInterrupt][fasync-interrupt] to create a stream of interrupts and
process them in an async task instead of spawning a dedicated thread with
`std::thread::spawn` and blocking on `irq.wait()`.

### DMA

MMIO handles must be mapped into a [MmioRegion][mmio-region] object using
[`VmoMapping::map()`][vmo-map] ([sdk/lib/driver/mmio/rust/][mmio-rust-source]).
Drivers must never perform manual bitwise operations (for example,
`val |= 1 << 5;`) on raw integers when accessing MMIO registers. Instead,
they should use the `mmio::register!` and `mmio::register_block!` macros

### Clocks

Clocks are controlled via the [fuchsia.hardware.clock FIDL service][clock-fidl].
Drivers are required to call `Enable()` on all clocks they depend on and
subsequently call `Disable()` once the clock signal is no longer needed.
Drivers must not call `Disable()` without first enabling the clock.

## Asynchronous code

Do not detach tasks using [`.detach()`][task-detach]. Instead, initialize a
[`fuchsia_async::Scope`][fasync-scope-struct] within the driver's `start` method
and use it to spawn concurrent work. The driver must retain ownership of the
`Scope`.

Prefer `Mutex` and `RwLock` from the `fuchsia_sync` crate to `std::sync`.

## Testing

Verify that build targets include the necessary driver tests.

### Unit Testing

You can and should test as much of your driver’s code in normal unit tests. To
"unit test" something while including driver startup and shutdown, you can use
the [fdf\_component::testing::TestHarness][test-harness] to start your driver
and interact with it.

If your driver declaration had an output\_name of `my_driver`, then the GN
targets for your driver would be `my_driver_test`. See the GN rules for any of
the rust example drivers in [//examples/drivers][examples-drivers] for
examples.

### Integration Tests

Use the [DriverTestRealm library][dtr-lib] to write integration tests, just like
you would with a C++ driver.

### General testing advice

When testing drivers that use `MmioRegion`:

* Prefer real VMOs: For simple use cases back the memory region with a real VMO,
  don't mock `read`/`write` methods.
* Advanced usages: Mocking `read`/`write` is reasonable for complex scenarios,
  consider using [`MockMemoryOps`][mock-memory-ops] or something like it.
* Use VMO Injection: In tests, create a `zx::Vmo`, map it using
  `VmoMapping::map()`, and pass the resulting `MmioRegion` to the driver.

[goldens]: /tools/create/goldens/my-driver-rust/
[component-manifest]: /docs/concepts/components/v2/component_manifests.md
[fdf-component]: https://fuchsia-docs.firebaseapp.com/rust/fdf_component/index.html
[driver-trait]: https://fuchsia-docs.firebaseapp.com/rust/fdf_component/trait.Driver.html
[start-method]: https://fuchsia-docs.firebaseapp.com/rust/fdf_component/trait.Driver.html#tymethod.start
[driver-context]: https://fuchsia-docs.firebaseapp.com/rust/fdf_component/struct.DriverContext.html
[take-node]: https://fuchsia-docs.firebaseapp.com/rust/fdf_component/struct.DriverContext.html#method.take_node
[node-struct]: https://fuchsia-docs.firebaseapp.com/rust/fdf_component/struct.Node.html
[rust-next-parent]: /examples/drivers/transport/driver/rust_next/parent/
[zircon-next-parent]: /examples/drivers/transport/zircon/rust_next/parent/
[fasync-scope]: /src/lib/fuchsia-async/src/runtime/portable/scope.rs
[rust-next-child]: /examples/drivers/transport/driver/rust_next/child/
[zircon-next-child]: /examples/drivers/transport/zircon/rust_next/child/
[node-add-child]: /src/devices/bin/driver_manager_rust/node/src/add.rs
[rust-logging]: https://docs.rs/log/latest/log/
[logging-guidelines]: /docs/contribute/governance/rfcs/0003_logging.md#log_severity_levels
[interrupt-kind]: https://fuchsia-docs.firebaseapp.com/rust/fidl_next/fuchsia/zx/trait.InterruptKind.html
[zircon-interrupt]: https://fuchsia-docs.firebaseapp.com/rust/fidl_next/fuchsia/zx/struct.Interrupt.html
[zircon-interrupt-source]: /sdk/rust/zx/src/interrupt.rs
[zx-interrupt]: https://fuchsia-docs.firebaseapp.com/rust/fidl_next/fuchsia/zx/struct.Interrupt.html
[fasync-interrupt]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_async/struct.OnInterrupt.html
[mmio-region]: https://fuchsia-docs.firebaseapp.com/rust/mmio/region/struct.MmioRegion.html
[vmo-map]: https://fuchsia-docs.firebaseapp.com/rust/mmio/vmo/struct.VmoMapping.html#method.map
[mmio-rust-source]: /sdk/lib/driver/mmio/rust/
[clock-fidl]: /sdk/fidl/fuchsia.hardware.clock/clock.fidl
[task-detach]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_async/struct.Task.html#method.detach
[fasync-scope-struct]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_async/struct.Scope.html
[test-harness]: https://fuchsia-docs.firebaseapp.com/rust/fdf_component/testing/harness/struct.TestHarness.html
[examples-drivers]: /examples/drivers
[dtr-lib]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_driver_test/index.html
[mock-memory-ops]: https://fuchsia-docs.firebaseapp.com/rust/mmio/mock/struct.MockMemoryOps.html
