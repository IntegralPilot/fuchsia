---
name: rust-driver-best-practices
description: >
  Apply idiomatic patterns and the standard libraries for writing or reviewing
  a modern Rust DFv2 driver in Fuchsia. Use when authoring a new Rust driver
  or porting one and needing fidl_next, fdf_component
  (Driver/DriverContext/driver_register!), fuchsia_async (Scope/OnInterrupt),
  safe MMIO via the mmio register!/register_block! macros, the pdev library,
  service_marker connections, lock-free concurrency, NodeController lifecycle,
  or VMO-backed driver tests -- and to review a finished Rust driver change.
  Covers patterns only, not the migration process (use migrate-cpp-to-rust-
  driver).
---

# Rust Driver Best Practices in Fuchsia

This guide outlines the recommended patterns, libraries, and best practices for
authoring modern Rust drivers in Fuchsia. It focuses on using `fidl_next`, the
`fdf_component` framework, safe memory-mapped I/O (MMIO), and idiomatic Rust
patterns.

For guidance on the *process* of migrating a C++ driver to Rust, refer to the
`migrate_cpp_to_rust_driver` skill.

## 1. Available Libraries for Rust Drivers

When writing a Rust driver, you will typically rely on the following key
libraries:

* **`fdf_component`**: The core Driver Framework v2 (DFv2) library. Provides
  macros and structures for defining drivers (`driver_register!`, `Driver`,
  `DriverContext`), creating nodes (`Node`, `NodeBuilder`), and offering
  services (`ServiceOffer`, `ServiceInstance`).
* **`fidl_next`**: The modern, next-generation FIDL bindings for Rust. Provides
  asynchronous abstractions like `ClientEnd`, `ServerEnd`, `Request`, and
  `Responder`. It is preferred over the legacy `fidl` crate.
* **`fuchsia_async` (`fasync`)**: The Fuchsia asynchronous runtime. Provides
  utilities for running concurrent tasks (`fasync::Scope`, `fasync::Task`),
  waiting for interrupts (`fasync::OnInterrupt`), and handling timers
  (`fasync::Timer`).
* **`fuchsia_component::server::ServiceFs`**: Used to construct the outgoing
  directory and route FIDL protocols to clients.
* **`mmio`**: Provides abstractions for safely mapping and accessing device
  registers (`MmioRegion`, `VmoMapping`, `VmoMemory`).
* **`pdev`**: Helper library for interacting with the Platform Device protocol
  (`fidl_fuchsia_hardware_platform_device`). It simplifies tasks like fetching
  MMIO regions (`map_mmio_by_id`, `map_mmio_by_name`). Prefer the `by_name`
  variants.
* **`fdf_metadata`**: Used for interacting with driver metadata (e.g.,
  `MetadataServer`).
* **`log`**: The standard Rust logging facade (`info!`, `error!`, `warn!`,
  `debug!`), which integrates with Fuchsia's syslog.
* **`zx`**: The Zircon kernel bindings, used for handling Zircon primitives like
  `Status`, `Vmo`, and `Interrupt`.

## 2. Build Rules

When defining a Rust driver in your `BUILD.gn`, use the following targets:

* **`fuchsia_rust_driver`**: Use this template for the main driver library. This
  compiles the driver logic into a shared library.
* **`fuchsia_driver_component`**: This target binds the compiled driver library,
  the component manifest (`.cml`), and the bind rules (`.bind`) into a single
  driver component that the Driver Framework can load.

### Lints
All Rust driver targets should enforce strict lints by including the following
configs:
```gn
  configs += [
    "//build/config/rust/lints:clippy_warn_all",
  ]
```

## 3. Style and Naming Conventions

For generic Rust style, naming conventions, and best practices (like `use`
groupings, avoiding `allow_unused`, constants vs magic values, `Mutex`
preferences, etc.), please refer to the `rust_best_practices` skill.

### Error Handling & Diagnostics
* **Do not swallow errors:** Avoid discarding errors silently (e.g., using `let
  _ = ...` or empty `map_err` without action). If an error is expected and safe
  to ignore, document the reasoning in a comment. Otherwise, handle it,
  propagate it, or log it.
* **Provide context in errors:** When returning or logging errors, include
  relevant context (e.g., arguments that failed validation, current state) to
  aid debugging. Avoid generic "internal error" messages without diagnostics.
* **Use `inspect_err`:** Prefer `inspect_err` over `map_err` when you only need
  to perform a side-effect (like logging) on the error before passing it
  through.

### Style, Imports and Idioms
* **Group and alphabetize `use` statements:** Keep imports grouped together at
  the top of the file without blank lines, and ensure they are alphabetical.
* **Avoid nested names in code:** Prefer importing names at the top of the file
  with `use` rather than using fully qualified paths in the code body, unless
  the namespace is required for clarity (e.g., disambiguating same-named types).
* **Prefer enums:** Use enums instead of raw constants where it improves scoping
  and reduces visual clutter.
* **Avoid magic values:** Explicitly name and document all magic numbers, or
  mark them clearly as unused if they are placeholders.
* **Pattern matching references:** Prefer matching references directly in
  patterns (e.g., `let Some(val) = &optional_val`) rather than using `.as_ref()`
  (e.g., `if let Some(val) = optional_val.as_ref()`).
* **Visibility in standalone binaries:** In standalone drivers or executables
  (not libraries), prefer standard `pub` over `pub(crate)` as it is not consumed
  externally anyway.
* **Module Architecture & Structure**: Always use the modern Rust approach of
  using a flat file module landing (e.g., `src/foo.rs`) instead of the older
  `mod.rs` in a folder (e.g., `src/foo/mod.rs`). Place submodule implementations
  in a sibling folder (e.g., `src/foo/bar.rs`).
* **No Dead Code Blanket Suppressions**: Never use `#![allow(dead_code)]` or
  `#[allow(dead_code)]` blanket overrides. For scaffolded or unused structs,
  fields, and constants, always use `#[expect(unused)]` (or
  `#[expect(dead_code)]`) directly on the symbols to ensure clean
  compiler-driven warnings.
* **Inlined Format Arguments**: Always prefer inlining format arguments inside
  format strings and macros (e.g., `log::error!("Failed: {error:?}");`) rather
  than passing them as trailing positional arguments (e.g.,
  `log::error!("Failed: {:?}", error);`), unless formatting a complex
  expression.
* **No Exclamation Marks in Logs**: Avoid utilizing exclamation marks (`!`) in
  all logging statements (e.g., `log::info!`, `log::error!`, `log::warn!`). Keep
  logging strings objective, descriptive, and direct.
* **Type-Safe Casting**: Avoid unsafe or silent `as` casts (e.g., `bar.size as
  usize`) unless strictly required by hot-path performance constraints. Always
  prefer type-safe conversions:
  * Use `usize::from(...)` where the conversion is guaranteed to succeed.
  * Use `usize::try_from(...).unwrap()` where the conversion could fail,
    ensuring loud and explicit panic crashes in case of overflow.
* **Type-Annotating Unused Results**: When discarding non-trivial results (like
  Zircon/FDF syscall `Result`s or handles) via `let _ = ...`, always explicitly
  annotate the _whole_ ignored type (e.g., `let _: Result<(), zx::Status> = ..`)
  to ensure type clarity and prevent silent mistakes.
* **Mandatory Unsafe Safety Comments**: All `unsafe` blocks must be preceded by
  a descriptive `// SAFETY: <explanation>` comment detailing exactly why the
  unsafe call is valid, how the preconditions are met, and why it is guaranteed
  to be safe.
* **Import macros from log**: Always import the used macros from the log crate
  instead of using fully qualified names at call site.
* **No numbered comment lists**: Don't use comments with numbered stages in
  function bodies like `// 1. Foo`, `// 2. Bar`. Keep the section comments as
  simple sentences but avoid the numbering.
* **Importing types**: By default, types and traits should be referenced in code
  without using their module path, e.g., code should use `Regex`, not
  `regex::Regex,` with a `use regex::Regex` at the top of the file or
  potentially in the relevant function. Exceptions:
  * In cases where there are ambiguities, it is acceptable to directly reference
    types with common names like `io::Error` or rename (`as IoError`).
  * In cases where you have a large "group" of types, e.g. the `hir` module in
    rustc which contains a large number of HIR types.
  * In macros and generated code.
  * In code with lots of dependencies, where importing everything would lead to
    a very large import block with little benefit.
* **Prefer `Option` or `Result` over Sentinel Values**: Do not use sentinel
  values (such as `u64::MAX`, `-1`, or empty/placeholder values) to represent
  errors or missing data. Use `Option<T>` or `Result<T, E>` to leverage Rust's
  type safety and force callers to handle these cases.
* **Prefer Early Returns (Guard Clauses)**: Use early returns (`return`,
  `continue`, `break`) to handle argument validation, error conditions, or
  simple cases. Avoid nesting the primary logic of a function inside a large
  `else` block, which unnecessarily increases indentation.
* **Extract Complex Expressions**: Extract complex match expressions, long
  closures, or deeply nested logic into well-named helper functions or methods.
  This keeps the main control flow clean and readable.
* **No Redundant `Result` for Infallible Functions**: If a function cannot fail,
  its return type should not be wrapped in a `Result`. Only use `Result` when
  there is a legitimate error case that the caller must handle.
* **Do Not Return Input Parameters Unchanged**: Avoid signatures that return an
  input parameter unmodified, as the caller already has access to this value.
* **Encapsulate Operations as Methods**: If a function primarily operates on a
  struct's internal state (especially registers or MMIO blocks), define it as a
  method on that struct. This improves encapsulation and simplifies caller-side
  logic.

**Driver-Specific Guidelines:**

### Documentation
* **References to Specs**: If a name, value, or logic flow is derived from a
  hardware specification or a reference driver (e.g., from the Linux kernel),
  include a comment referencing that source so it can be easily looked up by
  future maintainers.
* **No Inline Comments**: Prefer descriptive doc comments on public constants
  and symbols above their definitions, rather than brief inline or end-of-line
  comments.
* **Missing Libraries**: If a utility library seems to be missing in Rust when
  porting from C++, try to find the equivalent library in the C++
  implementation. It might be trivial to implement the logic directly in Rust
  based on the C++ source.
* **Associated Functions vs Top-Level Functions**: When authoring helper
  functions in your driver file, if a function does not take `&self` or `&mut
  self` and does not need access to the struct's private fields or constructor,
  prefer moving it out of the `impl` block to become a top-level function in the
  file or module. This is considered more idiomatic Rust.

## 4. Using `fidl_next` for FIDL Operations

Modern Rust drivers should use `fidl_next` instead of the legacy `fidl` crate.
**Note:** For complete examples, prefer looking at the codebase (e.g.,
`examples/drivers/transport/zircon/rust_next/parent/src/lib.rs`).

### Serving a Protocol
When offering a service, use `fidl_next::ServerDispatcher` or
`server_end.spawn_on` to route incoming requests to a local handler struct.
Ensure that the handler logic executes on the driver's local async scope.

```rust
impl i2c::ServiceHandler for Service {
    fn device(&self, server_end: ServerEnd<i2c::Device>) {
        // Spawn the server on the driver's scope
        server_end.spawn_on(DeviceServer, &self.scope);
    }
}
```

### Recommendation: Default to `fidl_next` for New Drivers
For new drivers or new basic support, prefer using `fidl_next` for serving FIDL
protocols to match modern best practices.
- Ensure `enable_rust_next = true` is set in the corresponding `fidl(...)`
  target in `BUILD.gn`.
- Depend on `//src/lib/fidl/rust_next/fidl_next` and the `_rust_next` version of
  the FIDL target.
- Implement `VregLocalServerHandler` (or equivalent) instead of matching on
  request streams.

## 5. Connecting to Services

When connecting to services (like `pdev` or custom services offered via
dictionaries), use the service capability instead of the protocol.

### Idiomatic Connection Pattern
Use the `service_marker` API on `context.incoming`. This returns a
`ServiceConnector` which allows overriding the instance name before connecting.

```rust
        let service = context
            .incoming
            .service_marker(ft::MyServiceMarker)
            .instance("custom_instance") // Optional: defaults to "default"
            .connect()?;

        let proxy = service.connect_to_my_protocol()?;
```

### Platform Device (pdev) Instance Naming Rules
- **Composite Drivers**: Only specify the instance as `"pdev"` if the driver is
  a composite bind and the platform device parent is listed as `"pdev"` in the
  driver bind rules.
- **Non-Composite Drivers**: Avoid specifying the instance name. It will default
  to `"default"`.

### Dictionary-offered Services
If a service is offered via a dictionary, it may be exposed as a specific
instance in the incoming namespace (e.g., `"default"`, `"left"`, `"right"`,
`"opt"`).
- Use `.instance("instance_name")` to connect to a specific instance.
- If you need to fall back to other instances, you can use `if let Ok(...)` or
  match on the result of `.connect()`.

### Service Instance Connection Fallbacks
When connecting to service instances, note that `connect()` or
`connect_to_protocol()` might not fail immediately if the instance doesn't exist
(it creates a channel that will be closed by the peer). If you need to try
multiple instances (e.g., "default" then "leaf"), be aware that the first
attempt might return `Ok(proxy)` even if it fails later. It is better to
directly connect to the expected instance if known, or check for `PEER_CLOSED`
errors if you must fall back.

### Composite Nodes and Services
When a driver binds to a composite node, it can access services offered by its
parents.
- **Instance Naming**: The service instance names in the composite driver's
  incoming directory will match the **PARENT NAMES** defined in the bind rules
  or node spec, NOT the instance names used when offering the service to the
  child nodes.
- **Primary Parent Alias**: In C++, connecting to a service without an instance
  name typically defaults to `"default"`. If this automatic aliasing does not
  work in Rust, a valid workaround is to explicitly map `"default"` to the
  primary parent's name in the driver's connection logic.

## 6. Safe Hardware Register Access and `pdev`

### Context & Citations
Always document hardware registers and blocks with explicit doc comments
detailing their purpose, alongside **verbatim file path and function citations
or page/section numbers** from the reference source codebase or programming
guide.

### Shared Memory & Hardware Descriptors
Any struct representing a hardware-shared memory descriptor layout (such as
GPDs, BDs, and ring buffers) **must** derive standard Fuchsia `zerocopy` traits
to guarantee memory safety and alignment during serialization/deserialization.

Always import the `zerocopy` traits (`FromBytes`, `IntoBytes`, `Immutable`,
`KnownLayout`) into the module namespace rather than utilizing verbose fully
qualified macro paths. This keeps derives clean and concise:

```rust
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

#[derive(Copy, Clone, Debug, Default, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct GeneralPacketDescriptor { ... }
```

### Using Macros for MMIO
Never perform manual bitwise operations (e.g., `val |= 1 << 5;`) on raw integers
when accessing MMIO registers. Instead, use the `mmio::register!` and
`mmio::register_block!` macros.

```rust
register! {
    RtcCtrl, u32, 0x00, RW, {
        pub bool, osc_sel, set_osc_sel: 8;
        pub bool, enable, set_enable: 12;
    }
}
```

These macros generate safe accessors for registers and fields. Update fields via
`regs.ctrl_mut().update(|r| { r.set_enable(true); });`. If using generated
register blocks, you may need to wrap `VmoMapping` in `MmioRegion<VmoMemory>` to
satisfy trait bounds.

### Platform Device and MMIO Access
When using the platform device protocol to access MMIO:
- **Use `pdev` library**: Prefer using the `pdev` library
  (`//sdk/lib/driver/platform-device/rust:pdev`) instead of manual FIDL calls
  and manual VMO mapping.
- **Convenience**: `pdev::PlatformDevice` handles fetching MMIO resources and
  mapping them into `MmioRegion<VmoMemory>` automatically via `map_mmio_by_id`
  or `map_mmio_by_name`.

```rust
use pdev::PlatformDevice;

// In start method:
let pdev = context.incoming.service_marker(fidl_fuchsia_hardware_platform_device::ServiceMarker).connect()?.connect_to_device()?;
let mmio = pdev.map_mmio_by_id(0).await?;
```

### Non-MMIO Bitfields
For data structures that are not memory-mapped registers, use the `bitfield`
crate's `bitfield!` macro.

```rust
use bitfield::bitfield;

bitfield! {
    pub struct Descriptor(u32);
    impl Debug;
    pub bool, valid, set_valid: 0;
    pub u8, priority, set_priority: 7, 4;
}
```

## 7. Concurrency, Async Patterns, and Locks

Modern Fuchsia drivers should avoid heavy synchronization like `Mutex` when
possible, preferring actor pattern when it makes sense.

### Locks
Prefer `Mutex` and `RwLock` from the `fuchsia_sync` crate, not `std::sync`.
* **Minimize Lock Scope**: Keep the duration of lock acquisition as short as
  possible. Avoid holding Mutex guards across `.await` points. Prefer
  encapsulating locked operations within synchronous helper methods on the
  protected resource (e.g., the register block struct) to ensure locks are
  released immediately.

### Patterns to avoid Mutex/Arc:
- **Task-Local State**: Own the state in a local task spawned on the driver's
  scope.
- **Channels for Connection Handoff**: Use an `mpsc::unbounded` channel to send
  connections from `ServiceFs` to your local task.
- **Concurrent Processing**: Use `rx.for_each_concurrent` on the channel
  receiver to process multiple connections simultaneously on the same local
  task.
- **State Ownership in Tasks**: For interrupt-driven drivers, consider moving
  ownership of the mutable state directly into the long-running async task that
  processes interrupts. This eliminates the need for locks.

### General Async and Concurrency Guidelines:
* **Concurrency (`fasync::Scope`)**: Do not detach tasks using `.detach()`.
  Instead, initialize a `fuchsia_async::Scope` within the driver's start method
  and use it to spawn concurrent work. The driver must retain ownership of the
  `Scope`.
* **Sequence client responses carefully:** In actor or async handlers, send
  response acknowledgements to clients only *after* the hardware or state
  changes have been successfully committed and verified.
* **Constructor spawning:** It is acceptable and often preferred for
  constructors (e.g., `new` or `start`) to take an async scope/spawner and spawn
  necessary asynchronous tasks directly, rather than returning them to the
  caller to spawn.
* **Fuchsia Multiserver:** Prefer using standard Fuchsia multiserver dispatching
  mechanisms (such as `fuchsia_component::server::*`) to handle multiple
  concurrent clients with less manual effort, rather than implementing custom
  dispatchers or actors unless strictly necessary.
* **Document concurrency risks:** Add comments explaining potential deadlock
  scenarios, such as when channel capacities are reached in actor models.

### Two approaches based on FIDL bindings:
1.  **With `fidl_next`**: Pass `ServerEnd` over the channel. The handler task
    constructs the server and runs it.
2.  **With Standard `fidl`**: Pass the `RequestStream` over the channel. The
    handler task processes the stream directly.

Both approaches allow you to share a plain reference `&RefCell<State>` among all
connection handlers, avoiding both `Mutex` and `Rc`!

## 8. Safe Interrupt Handling

When waiting on interrupts in Rust:
- **Avoid `zx::Handle` for Interrupts**: Do not store interrupts as raw
  `zx::Handle`. This forces the use of `unsafe` when waiting on them.
- **Generic `InterruptKind`**: Make your device struct generic over `K:
  zx::InterruptKind` (e.g., `Device<K>`). This allows storing `zx::Interrupt<K>`
  directly and using `fuchsia_async::OnInterrupt` safely in both production and
  tests.
- **Async Wait Loop**: Instead of spawning a dedicated thread with
  `std::thread::spawn` and blocking on `irq.wait()`, use `fasync::OnInterrupt`
  to create a stream of interrupts and process them in an async task.

Example:
```rust
        let irq = self.tsensor_irq.duplicate(zx::Rights::SAME_RIGHTS)?;
        scope.spawn(async move {
            let interrupt = fasync::OnInterrupt::new(irq);
            futures::pin_mut!(interrupt);

            while running.load(Ordering::SeqCst) {
                if let Some(Ok(_)) = interrupt.next().await {
                    // Handle interrupt
                }
            }
        });
```

## 9. Node Lifecycle and NodeController

When adding a child node using the Rust wrapper `Node::add_child` or
`Node::add_owned_child`:
- **NodeController is safe to drop**: In the Rust wrappers, dropping the
  `NodeController` is safe if you do not need to control the node.
- **Node Ownership**: For `add_owned_child`, the returned `Node` handle controls
  the lifetime. If you drop the `Node` handle, the node will be removed. Always
  store the `Node` handle in your driver struct if you want the node to persist.

When adding a child node directly via the `add_child` FIDL method (without using
the Rust wrappers):
- **Keep Controller Alive**: The `NodeController` client returned by `AddChild`
  controls the lifetime of the node. If you drop this client, the node will be
  removed by the Driver Framework.
- **Store in Struct**: Always store the `NodeController` client in your driver
  struct if you want the node to persist beyond the `start` method.

## 10. Testing Drivers

When testing drivers that use `MmioRegion`:
- **Avoid Manual Mocks**: Instead of mocking read/write methods, use real VMOs
  to back the memory region.
- **VMO Injection**: In tests, create a `zx::Vmo`, map it using
  `VmoMapping::map`, and pass the resulting `MmioRegion` to the driver.
- **Verification**: You can write expected values to the VMO before calling
  driver methods, or read from the VMO after driver methods to verify they wrote
  correctly.

Example:
```rust
    let sensor_vmo = zx::Vmo::create(0x1000).unwrap();
    let sensor_region = Arc::new(Mutex::new(VmoMapping::map(0, 0x1000, sensor_vmo.duplicate(zx::Rights::SAME_RIGHTS).unwrap()).unwrap()));
    // Pass sensor_region to driver
```

## 11. Reviewing Changes

Upon finishing authoring a change to a driver written in Rust, you MUST use this
skill and the `rust_best_practices` skill to review the change.

**Consider performing the review in multiple rounds, each focusing on one
aspect:**
- **Round 1: API & FIDL**: Verify that `fidl_next` is used for FIDL operations
  instead of the legacy `fidl` crate.
- **Round 2: Hardware Access**: Verify that the `pdev` library and register
  macros are used for MMIO access. Check for magic numbers.
- **Round 3: Concurrency & Lifecycle**: Verify proper async patterns, lock
  usage, and node lifecycle management (e.g., keeping `Node` handles alive if
  needed). Ensure Mutex guards are not held across `await` points and lock
  scopes are minimized.
- **Round 4: Style & Comments**: Check for AI-targeted comments, missing
  documentation, or non-idiomatic Rust. Verify that `Option`/`Result` are used
  instead of sentinel values, early returns are used to reduce indentation,
  infallible functions do not return `Result`, and operations on struct fields
  are encapsulated as methods.
