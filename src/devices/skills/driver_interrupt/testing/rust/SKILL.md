---
name: driver-interrupt-testing-rust
description: >
  Test IRQ / interrupt handling in a Rust DFv2 driver. Use when a unit test
  must create a zx::VirtualInterrupt (create_virtual), hand a duplicated
  zx::Interrupt to the driver, fire it with .trigger(), and assert the driver
  acked by awaiting Signals::VIRTUAL_INTERRUPT_UNTRIGGERED with
  fuchsia_async::OnSignals. For implementing interrupts in the driver use the
  Rust interrupt implementation skill; for C++ tests use the C++ interrupt
  testing skill.
---

# Driver Interrupt Testing (Rust)

## Dependencies

Testing with
[`fuchsia_async::OnSignals`](/src/lib/fuchsia-async/src/handle/zircon/on_signals.rs)
requires `fuchsia-async`:

**GN:**
```gn
deps = [
  "//src/lib/fuchsia-async",
]
```

**Bazel:**
```bazel
deps = [
  "@fuchsia_sdk//pkg/fuchsia-async",
]
```

## Provide an Interrupt

The test should create a virtual interrupt.
```rust
let virtual_interrupt = zx::VirtualInterrupt::create_virtual()?;
```
Duplicate it and convert it to a real interrupt for the driver:
```rust
let duplicate = virtual_interrupt.duplicate_handle(Rights::SAME_RIGHTS)?;
let real_interrupt = zx::Interrupt::from(duplicate.into_handle());
```

## Trigger an Interrupt

The test can trigger a virtual interrupt like so:
```rust
virtual_interrupt.trigger(zx::Instant::from_nanos(0))?;
```

## Verify Interrupt Acknowledgment

Wait for the `zx::Signals::VIRTUAL_INTERRUPT_UNTRIGGERED` signal using
`fuchsia_async::OnSignals`.

## Review Example Test Setup

```rust
use fuchsia_async::OnSignals;
use zx::{Interrupt, Signals, VirtualInterrupt};

// In test setup:
let virtual_interrupt = VirtualInterrupt::create_virtual()?;
let duplicate = virtual_interrupt.duplicate_handle(Rights::SAME_RIGHTS)?;
let real_interrupt = Interrupt::from(duplicate.into_handle());
// Send `real_interrupt` to driver...

// To trigger:
virtual_interrupt.trigger(zx::Instant::from_nanos(0))?;

// To verify ack:
OnSignals::new(&virtual_interrupt, Signals::VIRTUAL_INTERRUPT_UNTRIGGERED).await?;
```

## Common Pitfalls

* **Not driving the executor**: Virtual interrupts and `OnSignals` rely on the
  async executor. If your test triggers an interrupt but doesn't run the
  executor (e.g., by awaiting or using `RunLoopUntilIdle`), the signal handler
  will never execute.

## Further Reading

* [Handle Interrupts in a
  Driver](/docs/development/drivers/developer_guide/handle-interrupts-in-a-driver.md)
  - Comprehensive Fuchsia developer guide covering implementation and testing in
    both C++ and Rust.
* [Interrupts Reference](/docs/reference/kernel_objects/interrupts.md) - Kernel
  object reference detailing Zircon interrupts and the
  `ZX_VIRTUAL_INTERRUPT_UNTRIGGERED` signal.
* For guidance on implementing interrupts, see the [Driver Interrupt
  Implementation
  (Rust)](/src/devices/skills/driver_interrupt/implementation/rust/SKILL.md)
  skill.
