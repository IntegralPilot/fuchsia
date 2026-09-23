---
name: migrate-cpp-to-rust-driver
description: >
  Migrate a Fuchsia C++ driver to a full Rust driver on Driver Framework v2
  (DFv2). Use when porting an existing C++ driver's complete behavior to Rust
  -- bind rules, MMIO/IRQ/pdev access, FIDL services, and internal logic --
  landing on the fdf_component Driver trait, fidl_next, fuchsia_async, the
  mmio crate, and the log crate. This is the end-to-end rewrite. For only
  generating the coexisting -rust target, boilerplate, and .cml without
  porting logic, use migrate-cpp-to-rust-scaffolding; for idiomatic Rust
  patterns once ported, see rust-driver-best-practices.
---

# Migration Guide: C++ Driver to Rust Driver (DFv2)

This guide outlines the steps to migrate a C++ driver to a Rust driver in
Fuchsia, targeting the Driver Framework v2 (DFv2).

For a comprehensive list of best practices, idiomatic patterns, and library
usage in Rust drivers (e.g., concurrency, MMIO access, services, and testing),
see the **`rust_driver_best_practices`** skill.

## 1. Understand the Source Driver
Analyze the existing C++ driver to understand:
- Its bind rules (what parents it binds to).
- The resources it accesses (MMIO, IRQs, etc.).
- The services it offers and consumes via FIDL.
- Its internal state and logic.

## 2. Scaffold the Rust Driver
Rather than writing `BUILD.gn` rules, manifests, and Rust boilerplate from
scratch, use the **`migrate-cpp-to-rust-scaffolding`** skill to automate the
initial setup.

This sets up a coexisting Rust driver target (e.g., `my-driver-rust`),
boilerplate source code, Component Manifest (`.cml`), and integration into
`all_drivers_list.txt`.

Refer to the `migrate-cpp-to-rust-scaffolding` skill for:
- Stating the automated `scaffold_migration.py` command.
- Post-scaffolding manual steps (such as adding the driver `info` metadata
  file).
- Implementing advanced parameterization for hot swapping between the C++ and
  Rust implementations under a feature flag (e.g. `use_my_driver_rust`).

## 3. Handle Binding
Ensure the bind rules are correctly referenced in the `fuchsia_driver_component`
target. The bind rules themselves (`.bind` files) might not need to change much,
but the `BUILD.gn` target must link them.

### Type Mismatches in Bind Rules
If a bind rule compares a property with a value from a bind library (e.g.,
`fuchsia.test.BIND_PROTOCOL.DEVICE`), ensure the value type matches in the
driver. If the bind library defines it as `extend uint`, it is an integer, and
you must use `NodePropertyValue::IntValue(0x50)` (or the appropriate value) in
Rust, even if it looks like a string or enum in C++ helpers or bind rules.
Mismatches will cause `Comparing different value types` errors in
`driver_index`.

### Enum Values in Properties
When setting properties or bind rules with enums (e.g., from a bind library),
use `NodePropertyValue::EnumValue` and ensure the string matches the fully
qualified enum name in the bind library (e.g.,
`fuchsia.nodegroupbind.test.TEST_BIND_PROPERTY.DRIVER_LEFT`). Be careful with
casing, as bind libraries typically use uppercase for enum values.

## 4. Implement the Driver Trait
In your Rust source (e.g., `src/lib.rs`), use the `fdf_component` library to
define the driver.

```rust
use fdf_component::{Driver, DriverContext, Node, driver_register};
use zx::Status;

struct MyDriver {
    node: Node,
    // Add driver state here
}

driver_register!(MyDriver);

impl Driver for MyDriver {
    const NAME: &str = "my_driver";

    async fn start(mut context: DriverContext) -> Result<Self, Status> {
        let node = context.take_node()?;

        // Initialize driver, connect to services, etc.

        Ok(MyDriver { node })
    }

    async fn stop(&self) {
        // Cleanup resources
    }
}
```

## 5. Port Functionality
Translate C++ logic to Rust, leveraging the `rust_driver_best_practices` skill
for idiomatic patterns.

- **Asynchronous Operations**: Use `fuchsia_async` and `fasync::Scope`. Avoid
  detaching tasks.
- **FIDL**: Use `fidl_next` for modern asynchronous abstractions.
- **Logging**: Use the standard `log` crate (`info!`, `error!`, etc.).
- **Hardware Access**: Use the `mmio` crate macros (`register!`,
  `register_block!`) and the `pdev` library to map MMIO. Avoid manual
  `read_volatile`/`write_volatile`.
- **Interrupts via GPIO**: If the original C++ driver obtained and configured
  interrupts via the GPIO protocol (e.g., `gpio_->ConfigureInterrupt`), ensure
  the Rust driver matches this behavior rather than falling back to
  `pdev.get_interrupt_by_id`. Use `fasync::OnInterrupt` to wait on interrupts
  asynchronously.
- **Concurrency**: Avoid `Mutex` when possible. Prefer owning state in async
  tasks or communicating via channels.
- **Connecting to Services**: Use `service_marker` on `context.incoming`.

**See the `rust_driver_best_practices` skill for detailed code examples and best
practices for the above concepts.**

## 6. Update Component Manifest (.cml)
Ensure the manifest reflects the Rust driver's needs, including services it uses
and exposes. It should match the new component name and package path defined in
your `BUILD.gn`.

## 7. Verification
- Build the driver: `fx build`.
- Run tests: `fx test <driver_test_package>`.
- Use `ffx driver list` and `ffx driver dump` on a running system to verify the
  driver loads and binds correctly.

## Further Reading

* [Driver File Structure](/src/devices/skills/driver_file_structure/SKILL.md)
  - Standard Fuchsia driver component directory layout, file naming, and build
    target conventions.
