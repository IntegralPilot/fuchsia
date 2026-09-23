---
name: driver-fidl-server-impl-rust
description: >
  Serve (implement and advertise) a FIDL protocol or service from a Rust DFv2
  driver as the provider/server. Use when a Rust driver must build a
  ServiceFs, call add_fidl_service_instance/serve_outgoing, handle a request
  stream, offer a service to a child node via ServiceOffer/NodeBuilder,
  declare capabilities/expose in .cml, or choose Zircon vs Driver transport
  (build_zircon_offer/build_driver_offer). Not for C++ drivers (use the C++
  server skill) and not for connecting as a client (use the Rust client
  skill).
---

# Serve FIDL in a Rust Driver

## Dependencies

**GN**:
```gn
deps = [
  # Provides fdf_component::Driver and DriverContext
  "//sdk/lib/driver/component/rust",
  # Add bindings for the FIDL service
  "//sdk/fidl/fuchsia.hardware.example:fuchsia.hardware.example_rust",
  # Provides fuchsia_async
  "//src/lib/fuchsia-async",
  # Provides futures
  "third_party/rust_crates:futures",
]
```

**Bazel**:
```bazel
deps = [
  "@fuchsia_sdk//pkg/driver_component_rust",
  "@fuchsia_sdk//fidl/fuchsia.hardware.example:fuchsia.hardware.example_rust",
]
```

## Implement FIDL Serving

### 1. Define the Request Enum

Create an enum to represent all possible service requests. This is used to
multiplex services.

```rust
use fidl_fuchsia_hardware_example as fhardware;

enum IncomingRequest {
    MyService(fhardware::ServiceRequest),
}
```

### 2. Define the Protocol Handler

Implement a helper function to process requests for the specific protocol.

```rust
use futures::TryStreamExt;
use fidl_fuchsia_hardware_example as fhardware;

async fn handle_requests(mut stream: fhardware::MyProtocolRequestStream) {
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            fhardware::MyProtocolRequest::MyMethod { responder } => {
                // Handle request.
                let _ = responder.send();
            }
        }
    }
}
```

### 3. Choose Your Serving Strategy

Depending on your intent, choose one of the following options to set up your
outgoing directory.

#### Option A: Advertise Only (No Child Node)

Use this if you only want to make the service available to other components in
the system via normal capability routing.

```rust
use fuchsia_component::server::ServiceFs;

let mut outgoing = ServiceFs::new();
outgoing.dir("svc").add_fidl_service_instance(
    "default",
    IncomingRequest::MyService,
);
```

#### Option B: Advertise and Offer to Child Node

Use this if you are creating a child node and need to offer the service to it.
This helper both registers with `ServiceFs` and creates the offer.

```rust
use fdf_component::ServiceOffer;
use fuchsia_component::server::ServiceFs;

let mut outgoing = ServiceFs::new();

// This registers with ServiceFs AND prepares the offer.
let offer = ServiceOffer::new()
    .add_default_named(&mut outgoing, "default", IncomingRequest::MyService)
    .build_zircon_offer();
```

### 4. Tie it Together in Driver Start

Here is a complete example for both options in the driver's `start` method.

#### For Option A (Advertise Only):

```rust
use fuchsia_async::Task;
use futures::StreamExt;

let mut outgoing = ServiceFs::new();
outgoing.dir("svc").add_fidl_service_instance(
    "default",
    IncomingRequest::MyService,
);

// Serve outgoing directory.
context.serve_outgoing(&mut outgoing)?;

// Spawn background task.
let outgoing_task = Task::spawn(async move {
    outgoing
        .for_each_concurrent(None, move |req| async move {
            match req {
                IncomingRequest::MyService(
                    fhardware::ServiceRequest::MyProtocol(stream),
                ) => {
                    handle_requests(stream).await;
                }
            }
        })
        .await;
});

// Store `outgoing_task` in your driver struct.
```

#### For Option B (Advertise and Offer to Child):

```rust
use fdf_component::{NodeBuilder, ServiceOffer};
use fuchsia_async::Task;
use futures::StreamExt;

let mut outgoing = ServiceFs::new();

// 1. Register and create offer.
let offer = ServiceOffer::new()
    .add_default_named(&mut outgoing, "default", IncomingRequest::MyService)
    .build_zircon_offer();

// 2. Serve outgoing directory.
context.serve_outgoing(&mut outgoing)?;

// 3. Create child node with the offer.
let child_node = NodeBuilder::new("my-child-node")
    .add_offer(offer)
    .build();

let child = node.add_child(child_node).await?;

// 4. Spawn background task for the event loop.
let outgoing_task = Task::spawn(async move {
    outgoing
        .for_each_concurrent(None, move |req| async move {
            match req {
                IncomingRequest::MyService(
                    fhardware::ServiceRequest::MyProtocol(stream),
                ) => {
                    handle_requests(stream).await;
                }
            }
        })
        .await;
});

// Store `outgoing_task` in your driver struct.
```

### 5. Update Manifest (.cml)

If you chose **Option A** (or if you want to expose the service outside the
driver host for Option B), ensure the driver manifest declares and exposes the
service.

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

> [!IMPORTANT]
> If your driver implements the FIDL service itself, you must **both** advertise
> it (serve it in outgoing) and offer it to the child node. If you are simply
> forwarding a service from your parent to a child, you only need to offer it.

## Driver Transport vs Zircon Transport

FIDL services in drivers can use either **Zircon transport** (standard channels)
or **Driver transport** (optimized for in-process communication).

### Zircon Transport
This is the default transport used for communication with components outside the
driver host or when performance is not critical.

* **Offer Method**: `ServiceOffer::build_zircon_offer()`
* **Channel Type**: `zx::Channel`

### Driver Transport
This transport is used for high-performance communication between drivers
running in the same driver host process.

* **Offer Method**: `ServiceOffer::build_driver_offer()`
* **Channel Type**: `fdf_fidl::DriverChannel`

Note: Driver transport in Rust is often used in conjunction with the `fidl_next`
library for efficient message handling.

## Common Pitfalls

* **Scope Lifecycle**: Ensure the driver's state struct holds onto the `Scope`
  object. If the `Scope` is dropped (for example, at the end of `start()`), all
  tasks spawned on it will be aborted immediately.
* **Missing Expose in Manifest**: If the service is not exposed in the `.cml`,
  other components will not be able to discover it.
* **Forgetting to offer to child nodes**: When serving this capability for use
  by a child node, simply adding it to the outgoing directory is not enough. The
  capability must also be explicitly **offered** in the `NodeAddArgs` when
  calling `add_child()`.
* **Advertising vs Offering**:
  * To **advertise** a service (make it visible to other components via
    routing), use `ServiceFs::dir("svc").add_fidl_service_instance(...)`.
  * To **offer** a service to a specific child node being created by the driver,
    use `ServiceOffer::add_default_named(...)` which both registers with
    `ServiceFs` and creates the offer.

## Further Reading

* [Driver FIDL Usage
  Skill](/src/devices/skills/driver_fidl/client/implementation/rust/SKILL.md)
* [Server FIDL
  Debugging](/src/devices/skills/driver_fidl/server/debugging/SKILL.md)
* [Zircon Transport Rust Example
  Driver](/examples/drivers/transport/zircon/rust/)
* [Driver Transport Rust Example
  Driver](/examples/drivers/transport/driver/rust_next/)
