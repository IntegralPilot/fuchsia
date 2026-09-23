---
name: driver-fidl-server-impl-cpp
description: >
  Serve (implement and advertise) a FIDL protocol or service from a C++ DFv2
  driver as the provider/server. Use when a C++ driver must inherit
  fidl::WireServer, manage clients with fidl::ServerBindingGroup, call
  outgoing() AddService with an InstanceHandler, offer a service to a child
  node via fdf::MakeOffer2/AddChild, declare capabilities/expose in .cml, or
  choose Zircon vs Driver transport. Not for Rust drivers (use the Rust server
  skill) and not for connecting as a client (use the C++ client skill).
---

# Serve FIDL in a C++ Driver

## Dependencies

**GN**:
```gn
deps = [
  # Provides fdf::DriverBase2 and outgoing()
  "//sdk/lib/driver/component/cpp",
  # Add bindings for the FIDL service
  "//sdk/fidl/fuchsia.hardware.example:fuchsia.hardware.example_cpp",
]
```

**Bazel**:
```bazel
deps = [
  "@fuchsia_sdk//pkg/driver_component_cpp", # Provides fdf::DriverBase2
  "@fuchsia_sdk//fidl/fuchsia.hardware.example:fuchsia.hardware.example_cpp",
]
```

## Implement FIDL Serving

### 1. Implement the Server Interface

Inherit from `fidl::WireServer<Protocol>` and implement the required methods.

```cpp
#include <lib/driver/component/cpp/driver_base2.h>
#include <fidl/fuchsia.hardware.example/cpp/wire.h>

class MyDriver : public fdf::DriverBase2,
                 public fidl::WireServer<fuchsia_hardware_example::MyProtocol> {
 public:
  // ...

  // Method without parameters
  void MyMethod(MyMethodCompleter::Sync& completer) override {
    // Implement logic
    completer.Reply();
  }

  // Method with parameters
  void MyMethodWithArg(MyMethodWithArgRequestView request,
                       MyMethodWithArgCompleter::Sync& completer) override {
    // Access request parameters
    auto value = request->value;
    // Implement logic
    completer.Reply();
  }

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_example::MyProtocol> bindings_;
};
```

### 2. Advertise the Service

In the driver's `Start()` method, create an `InstanceHandler` and add the
service to the outgoing directory.

```cpp
zx::result<> Start(fdf::DriverContext context) override {
  // Advertise the service
  fuchsia_hardware_example::Service::InstanceHandler handler({
      .device = bindings_.CreateHandler(
          this, dispatcher(), fidl::kIgnoreBindingClosure),
  });

  zx::result result = outgoing()->AddService<fuchsia_hardware_example::Service>(
      std::move(handler));
  if (result.is_error()) {
    fdf::error("Failed to add service: {}", result.status_string());
    return result.take_error();
  }

  return zx::ok();
}
```

> [!NOTE]
> In the example above, `bindings_` is an instance of
> `fidl::ServerBindingGroup<fuchsia_hardware_example::MyProtocol>`. Use
> `ServerBindingGroup` to manage connections, as it supports multiple
> concurrent clients and handles cleanup automatically when channels close.
> Avoid using `fidl::ServerBinding` because it only supports a single
> connection.

## Offer FIDL to Child Nodes

If the driver creates a driver child node and needs to offer the service to it,
use `fdf::MakeOffer2` to create the offer.

```cpp
#include <fidl/fuchsia.component.decl/cpp/wire.h>
#include <fidl/fuchsia.driver.framework/cpp/wire.h>

// ... inside Start method ...

// Create the offer for the child node.
// Assumes you are offering a FIDL Service (the modern pattern).
auto offer = fdf::MakeOffer2<fuchsia_hardware_example::Service>();

std::vector<fuchsia_driver_framework::Offer> offers = { offer };

// Create the child node using the DriverBase helper.
auto result = AddChild("my-child-node", {}, offers);
```

> [!IMPORTANT]
> If your driver implements the FIDL service itself, you must **both** advertise
> it (serve it in outgoing) and offer it to the child node. If you are simply
> forwarding a service from your parent to a child, you only need to offer it.

### 3. Update Manifest (.cml)

Ensure the driver's manifest declares and exposes the service.

```json5
capabilities: [
    {
        service: "fuchsia.hardware.example.Service",
    },
],
expose: [
    {
        service: "fuchsia.hardware.example.Service",
        from: "self",
    },
],
```

## Driver Transport vs Zircon Transport

FIDL services in drivers can use either **Zircon transport** (standard channels)
or **Driver transport** (optimized for in-process communication). The
implementation differs in includes, base classes, and method signatures.

### Zircon Transport

Used for communication with components outside the driver host or when
performance is not critical.

* **Include**: `#include <fidl/library.name/cpp/wire.h>`
* **Base Class**: `fidl::WireServer<Protocol>`
* **Binding Group**: `fidl::ServerBindingGroup<Protocol>`
* **Handler Creation**: Requires an `async_dispatcher_t*` (e.g.,
  `dispatcher()`).
* **Method Signature**: `void Method(RequestView, Completer::Sync&)`

### Driver Transport

Used for high-performance communication between drivers in the same process.

* **Include**: `#include <fidl/library.name/cpp/driver/wire.h>`
* **Base Class**: `fdf::WireServer<Protocol>`
* **Binding Group**: `fdf::ServerBindingGroup<Protocol>`
* **Handler Creation**: Requires an `fdf_dispatcher_t*` (e.g.,
  `driver_dispatcher()->get()`).
* **Method Signature**: `void Method(RequestView, fdf::Arena& arena,
  Completer::Sync&)`

Note that methods in Driver transport take an extra `fdf::Arena&` parameter for
memory allocation.

## Common Pitfalls

* **Using `fidl::ServerBinding`**: Drivers typically need to support multiple
  client connections. `fidl::ServerBinding` only supports a single connection.
  Always use `fidl::ServerBindingGroup` to manage multiple client connections
  automatically.
* **Missing Expose in Manifest**: If the service is not exposed in the `.cml`,
  other components will not be able to discover it, even if `AddService`
  succeeds.
* **Incorrect Dispatcher**: Use the correct dispatcher (usually the driver's
  default dispatcher) when creating the handler.
* **Forgetting to offer to child nodes**: When serving this capability for use
  by a child node, simply adding it to the outgoing directory is not enough. The
  capability must also be explicitly **offered** in the `NodeAddArgs` when
  calling `AddChild()`.
* **Forgetting to serve the capability**: Offering a capability to a child node
  only sets up the routing. You must still actually serve the protocol in your
  outgoing directory (advertise it) if you are the one implementing it.
* **Advertising vs Offering**:
  * To **advertise** a service (make it visible to other components via
    routing), use `outgoing()->AddService<Service>(...)`.
  * To **offer** a service to a specific child node being created by the driver,
    use `fdf::MakeOffer2<Service>()` and pass it to `AddChild()`.

## Further Reading

* [Driver FIDL Usage
  Skill](/src/devices/skills/driver_fidl/client/implementation/cpp/SKILL.md)
* [Server FIDL
  Debugging](/src/devices/skills/driver_fidl/server/debugging/SKILL.md)
* [Zircon Transport Example Driver](/examples/drivers/transport/zircon/v2/)
* [Driver Transport Example Driver](/examples/drivers/transport/driver/v2/)
