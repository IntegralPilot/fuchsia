---
name: driver-fidl-client-impl-cpp
description: >
  Connect a C++ DFv2 driver (client/consumer) to a FIDL protocol or service
  from its incoming namespace. Use when a C++ driver must call
  context.incoming().Connect(), inherit fdf::DriverBase2, add a use entry to
  its .cml, pick Zircon vs Driver transport (fidl::WireClient /
  fdf::WireClient), or connect to a named service instance. Not for Rust
  drivers (use the Rust client skill) and not for serving/advertising FIDL
  (use the C++ server skill).
---

# Using FIDL in a C++ Driver

## Dependencies

To use the patterns in this skill, ensure your build files include the following
dependencies:

**GN:**
```gn
deps = [
  "//sdk/lib/driver/component/cpp", # For fdf::DriverBase2
  # Add the C++ bindings for the FIDL protocol you are connecting to
  "//sdk/fidl/fuchsia.hardware.example:fuchsia.hardware.example_cpp",
]
```

**Bazel:**
```bazel
deps = [
    "@fuchsia_sdk//pkg/driver_component_cpp", # For fdf::DriverBase2
    # Add the C++ bindings for the FIDL protocol you are connecting to
    "@fuchsia_sdk//fidl/fuchsia.hardware.example:fuchsia.hardware.example_cpp",
]
```

## Implementation Steps

### 1. Inherit from `fdf::DriverBase2`

Your driver class must inherit from
[`fdf::DriverBase2`](/sdk/lib/driver/component/cpp/driver_base2.h) to access the
`context.incoming()` namespace during `Start`.

```cpp
// Contains fdf::DriverBase2.
#include <lib/driver/component/cpp/driver_base2.h>

class MyDriver : public fdf::DriverBase2 {
 public:
  // Override Start() and declare your FIDL clients here.
};
```

### 2. Connect to the Parent

Choose the appropriate pattern based on how the capability is exposed. These
examples assume you are inside the `Start(fdf::DriverContext context)` method.

#### **If** the capability is exposed directly as a protocol:

**C++ Code:**
```cpp
#include <fidl/fuchsia.hardware.example/cpp/wire.h>

zx::result<> Start(fdf::DriverContext context) override {
  zx::result client =
      context.incoming().Connect<fuchsia_hardware_example::MyProtocol>();
  if (client.is_error()) {
    fdf::error("Failed to connect to MyProtocol: {}", client.status_string());
    return client.take_error();
  }
  my_client_ = std::move(client.value());

  return zx::ok();
}
```

**Manifest (.cml):**
```json5
use: [
    { protocol: [ "fuchsia.hardware.example.MyProtocol" ] },
],
```

#### **Otherwise** (If the capability is exposed within a Service):

**C++ Code:**
```cpp
#include <fidl/fuchsia.hardware.example/cpp/wire.h>

zx::result<> Start(fdf::DriverContext context) override {
  zx::result client =
      context.incoming().Connect<fuchsia_hardware_example::Service::Device>();
  if (client.is_error()) {
    fdf::error("Failed to connect to Service: {}", client.status_string());
    return client.take_error();
  }
  my_client_ = std::move(client.value());

  return zx::ok();
}
```

**Manifest (.cml):**
```json5
use: [
    { service: "fuchsia.hardware.example.Service" },
],
```

### 3. Connecting to a Named Instance

If the parent exposes multiple instances of the same protocol/service, specify
the instance name when connecting by passing it as an argument to `Connect()`.

```cpp
  zx::result client =
      context.incoming().Connect<fuchsia_hardware_example::Service::Device>(
          "instance_name");
```

To find the correct instance name by looking at the bind rules, see the [Finding
Instance Names from Bind
Rules](/src/devices/skills/driver_bind_find_instance_name/SKILL.md) skill.

## Driver Transport vs Zircon Transport

When connecting to FIDL services, clients must also use the correct transport.

### Zircon Transport

Used for communication with components outside the driver host or when
performance is not critical.

* **Include**: `#include <fidl/library.name/cpp/wire.h>`
* **Client Type**: `fidl::WireClient<Protocol>` or
  `fidl::WireSyncClient<Protocol>`
* **Connection**: `context.incoming().Connect<Service::Device>()` returns
  `zx::result<fidl::ClientEnd<Protocol>>`.

### Driver Transport

Used for high-performance communication between drivers in the same process.

* **Include**: `#include <fidl/library.name/cpp/driver/wire.h>`
* **Client Type**: `fdf::WireClient<Protocol>` or
  `fdf::WireSyncClient<Protocol>`
* **Connection**: `context.incoming().Connect<Service::Device>()` returns
  `zx::result<fdf::ClientEnd<Protocol>>`.
* **Dispatcher**: Requires an `fdf_dispatcher_t*` to bind (e.g.,
  `driver_dispatcher()->get()`).
* **Calls**: Methods require an `fdf::Arena` for allocation.

## Common Pitfalls

* **Forgetting the `.cml` entry**: The C++ code will compile, but `Connect` will
  fail at runtime with `ZX_ERR_NOT_FOUND` or similar because the framework won't
  route the capability to the driver sandbox.
* **Mixing up Protocol and Service in `.cml`**: If the capability is a FIDL
  `service`, use the `service` field in the `.cml`. If it is a direct
  `protocol`, use the `protocol` field. Declaring a service under `protocol` (or
  vice versa) will result in routing failures at runtime, even if the driver
  code compiles successfully.
* **Using the wrong bindings**: Use the new C++ bindings (e.g.,
  `fuchsia_hardware_example::MyProtocol`) instead of the legacy ones.
* **Ignoring the result**: Always check the `zx::result` returned by `Connect`
  and handle errors gracefully.

## Further Reading

* [FIDL Tutorial for
  Drivers](/docs/development/drivers/tutorials/fidl-tutorial.md) - Learn how to
  define and use FIDL protocols in drivers.
* For guidance on debugging FIDL connection issues, see the [Driver FIDL Usage
  Debugging](/src/devices/skills/driver_fidl/client/debugging/SKILL.md) skill.
* [Zircon Transport Example Driver](/examples/drivers/transport/zircon/v2/)
* [Driver Transport Example Driver](/examples/drivers/transport/driver/v2/)
