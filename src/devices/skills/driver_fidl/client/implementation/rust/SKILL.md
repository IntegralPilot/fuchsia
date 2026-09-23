---
name: driver-fidl-client-impl-rust
description: >
  Connect a Rust DFv2 driver (client/consumer) to a FIDL protocol or service
  from its incoming namespace. Use when a Rust driver must call
  connect_to_protocol, use context.incoming.service_marker(...).connect(),
  handle DriverContext, select a named .instance(), reference a Marker type,
  or add a use entry to its .cml. Not for C++ drivers (use the C++ client
  skill) and not for serving/advertising FIDL (use the Rust server skill).
---

# Using FIDL in a Rust Driver

## Dependencies

To use the patterns in this skill, ensure the build files include the following
dependencies:

**GN:**
```gn
deps = [
  # Add the Rust bindings for the FIDL protocol you are connecting to
  "//sdk/fidl/fuchsia.hardware.example:fuchsia.hardware.example_rust",
  # Add the driver framework crate if needed, e.g.,
  "//sdk/lib/driver/component/rust:fdf_component",
]
```

**Bazel:**
```bazel
deps = [
    # Add the Rust bindings for the FIDL protocol you are connecting to
    "@fuchsia_sdk//fidl/fuchsia.hardware.example:fuchsia.hardware.example_rust",
    # Add the driver framework crate if needed, e.g.,
    "@fuchsia_sdk//pkg/fdf_component",
]
```

## Implementation Steps

### 1. Access the Incoming Namespace

The driver's `start` method receives a
[`DriverContext`](/sdk/lib/driver/component/rust/src/context.rs) which contains
the `incoming` namespace.

```rust
use fdf_component::{Driver, DriverContext, Node};

impl Driver for MyDriver {
    async fn start(mut context: DriverContext) -> Result<Self, zx::Status> {
        // ...
    }
}
```

### 2. Connect to the Parent

Choose the appropriate pattern based on how the capability is exposed:

#### **If** the capability is exposed directly as a protocol:

**Rust Code:**
```rust
use fuchsia_component::client::connect_to_protocol;

let client = connect_to_protocol::<fidl_fuchsia_hardware_example::MyProtocolMarker>()
    .map_err(|_| zx::Status::INTERNAL)?;
```

**Manifest (.cml):**
```json5
use: [
    { protocol: [ "fuchsia.hardware.example.MyProtocol" ] },
],
```

#### **Otherwise** (If the capability is exposed within a Service):

**Rust Code:**
```rust
let client = context
    .incoming
    .service_marker(fidl_fuchsia_hardware_example::ServiceMarker)
    .connect()?
    .connect_to_device()
    .map_err(|_| zx::Status::INTERNAL)?;
```

**Manifest (.cml):**
```json5
use: [
    { service: "fuchsia.hardware.example.Service" },
],
```

### Connecting to a Named Instance

If the parent exposes multiple instances of the same protocol/service, specify
the instance name when connecting by using the `.instance()` method on the
service connector.

```rust
let client = context
    .incoming
    .service_marker(fidl_fuchsia_hardware_example::ServiceMarker)
    .instance("pdev")
    .connect()?
    .connect_to_device()
    .map_err(|_| zx::Status::INTERNAL)?;
```

To find the correct instance name by looking at the bind rules, see the [Finding
Instance Names from Bind
Rules](/src/devices/skills/driver_bind_find_instance_name/SKILL.md) skill.

## Common Pitfalls

* **Forgetting the `.cml` entry**: The Rust code will compile, but the
  connection will fail at runtime because the framework won't route the
  capability to the driver sandbox.
* **Mixing up Protocol and Service in `.cml`**: If the capability is a FIDL
  `service`, use the `service` field in the `.cml`. If it is a direct
  `protocol`, use the `protocol` field. Declaring a service under `protocol` (or
  vice versa) will result in routing failures at runtime, even if the driver
  code compiles successfully.
* **Using the wrong marker**: Use the `Marker` type for the protocol or service
  (e.g., `MyProtocolMarker` or `ServiceMarker`).
* **Ignoring errors**: Always handle the `Result` returned by the connect
  methods.

## Further Reading

* [FIDL Tutorial for
  Drivers](/docs/development/drivers/tutorials/fidl-tutorial.md) - Learn how to
  define and use FIDL protocols in drivers.
* For guidance on debugging FIDL connection issues, see the [Driver FIDL Usage
  Debugging](/src/devices/skills/driver_fidl/client/debugging/SKILL.md) skill.
* [Zircon Transport Rust Example
  Driver](/examples/drivers/transport/zircon/rust/)
* [Driver Transport Rust Example
  Driver](/examples/drivers/transport/driver/rust_next/)
