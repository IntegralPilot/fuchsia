---
name: driver-interrupt-impl-rust
description: >
  Implement IRQ / interrupt handling in a Rust DFv2 driver. Use when a Rust
  driver must acquire a zx::Interrupt, build a stream with
  fuchsia_async::OnInterrupt, drive it on a Task::local, and acknowledge via
  the inner handle's .ack() (AsRef) -- including when interrupts stop after
  the first because ack or the stored Task was dropped. For writing interrupt
  tests use the Rust interrupt testing skill; for C++ drivers use the C++
  interrupt implementation skill.
---

# Driver Interrupt Implementation (Rust)

## Dependencies

To use
[fuchsia_async::OnInterrupt](/src/lib/fuchsia-async/src/handle/zircon/on_interrupt.rs),
add the following to your build file:

**GN:**
```gn
deps = [
  "//src/lib/fuchsia-async",
  "//third_party/rust_crates:futures",
]
```

**Bazel:**
```bazel
deps = [
  "@fuchsia_sdk//pkg/fuchsia-async",
  "//third_party/rust_crates/vendor:futures",
]
```

## Acquire an Interrupt

Drivers typically acquire an interrupt object from a FIDL service.

#### **If** the interrupt is acquired from a standard FIDL service (e.g., GPIO):

A driver might request a GPIO interrupt via
[`fuchsia.hardware.gpio/Gpio.GetInterrupt`](/sdk/fidl/fuchsia.hardware.gpio/gpio.fidl).

#### **Otherwise** (If the interrupt is defined via devicetree for a platform device):

The driver typically uses the Platform Device service to acquire it via methods
like `GetInterruptById` or `GetInterruptByName` on the
[`fuchsia.hardware.platform.device/Device`](/sdk/fidl/fuchsia.hardware.platform.device/platform-device.fidl)
protocol.

**Lifecycle and Cleanup**: No manual cleanup of the interrupt handle is
necessary. The `zx::Interrupt` type implements the `Drop` trait, which
automatically closes the underlying Zircon handle when the object goes out of
scope.

## Listen to an Interrupt

* Use `fuchsia_async::OnInterrupt` to create a stream from the interrupt handle.
* Process the stream in a local task (using `Task::local`). This is preferred in
  drivers to avoid `Send` constraints on non-thread-safe state like MMIO
  regions.
* Acknowledge the interrupt using the inner `zx::Interrupt` handle (accessible
  via `AsRef`) after handling.

## Implement an Interrupt Handler

```rust
use fdf_component::{Driver, DriverContext};
use fuchsia_async::{OnInterrupt, Task};
use futures::StreamExt;
use zx::{Interrupt, Status};

pub struct MyDriver {
  interrupt_handler: Task<()>,
}

impl Driver for MyDriver {
    async fn start(context: DriverContext) -> Result<Self, Status> {
        let interrupt: Interrupt = todo!();
        // Use Box::pin because OnInterrupt is not Unpin.
        let mut interrupt_stream = Box::pin(OnInterrupt::new(interrupt));
        // Use Task::local as driver hosts typically run on a single-threaded executor.
        let interrupt_handler = Task::local(async move {
          while let Some(Ok(_time)) = interrupt_stream.as_mut().next().await {
            if let Err(e) = Self::handle_interrupt() {
                log::error!("Failed to handle interrupt: {e:?}");
            }
            // Access inner Interrupt via AsRef trait.
            if let Err(e) = std::pin::Pin::get_ref(interrupt_stream.as_ref()).as_ref().ack() {
                log::error!("Failed to ack interrupt: {e:?}");
            }
          }
        });

        Ok(Self { interrupt_handler })
    }

    async fn handle_interrupt() -> Result<(), Status> {
        todo!();
    }
}
```

## Common Pitfalls

* **Forgetting to acknowledge the interrupt**: Failing to acknowledge the
  underlying interrupt handle (by accessing the inner `Interrupt` handle via
  `AsRef` and calling `.ack()`) will prevent the interrupt from triggering
  again. Ensure you call this after processing each event.
* **Task cancellation**: The interrupt stream handler runs in a spawned task. If
  the `Task` handle is dropped, the task is canceled and the driver will stop
  receiving interrupts. Ensure the `Task` is stored in your driver struct to
  keep it alive.

## Further Reading

* [Interrupt Handling Example (Rust)](/examples/drivers/interrupt_handling/rust)
  - Example driver demonstrating interrupt handling in Rust.
* [Handle Interrupts in a
  Driver](/docs/development/drivers/developer_guide/handle-interrupts-in-a-driver.md)
  - Comprehensive Fuchsia developer guide covering implementation and testing in
    both C++ and Rust.
* [Interrupts Reference](/docs/reference/kernel_objects/interrupts.md) - Kernel
  object reference detailing Zircon interrupts and the
  `ZX_VIRTUAL_INTERRUPT_UNTRIGGERED` signal.
* For guidance on testing interrupts, see the [Driver Interrupt Testing
  (Rust)](/src/devices/skills/driver_interrupt/testing/rust/SKILL.md) skill.
