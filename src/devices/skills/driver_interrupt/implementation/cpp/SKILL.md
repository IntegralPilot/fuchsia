---
name: driver-interrupt-impl-cpp
description: >
  Implement IRQ / interrupt handling in a C++ DFv2 driver. Use when a C++
  driver must acquire a zx::interrupt (from pdev GetInterrupt or
  fuchsia.hardware.gpio/Gpio.GetInterrupt), listen with async::IrqMethod on
  its dispatcher, and acknowledge with interrupt_.ack() (using fit::defer to
  re-arm) -- including when interrupts stop firing after the first because ack
  was missed. For writing interrupt tests use the C++ interrupt testing skill;
  for Rust drivers use the Rust interrupt implementation skill.
---

# Driver Interrupt Implementation (C++) (DFv2)

## Dependencies

To use [async::IrqMethod](/sdk/lib/async/include/lib/async/cpp/irq.h),
[zx::interrupt](/zircon/system/ulib/zx/include/lib/zx/interrupt.h), and
[fit::defer](/sdk/lib/fit/include/lib/fit/defer.h), add the following to your
build file (though some may be included transitively by other driver
dependencies):

**GN:**
```gn
deps = [
  "//sdk/lib/async:async-cpp",  # For `async::IrqMethod`
  "//sdk/lib/fit",              # For `fit::defer`
  "//zircon/system/ulib/zx",    # For `zx::interrupt`
]
```

**Bazel:**
```bazel
deps = [
    "@fuchsia_sdk//pkg/async-cpp",  # For `async::IrqMethod`
    "@fuchsia_sdk//pkg/fit",        # For `fit::defer`
    "@fuchsia_sdk//pkg/zx",         # For `zx::interrupt`
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
necessary. The `zx::interrupt` wrapper manages the handle's lifecycle; its
destructor automatically closes the handle when the object goes out of scope.

## Listen to an Interrupt

* Use `async::IrqMethod` to listen to interrupts.
* It executes a callback on a dispatcher (usually the driver's dispatcher).
* Ensure the callback does not block the dispatcher excessively if it's
  synchronized.
* Use `fit::defer` to ensure the interrupt is acknowledged (`interrupt_.ack()`)
  even if errors occur, to re-arm the interrupt.

## Implement an Interrupt Handler

```cpp
// Contains `driver_base2.h`.
#include <lib/driver/component/cpp/driver_base2.h>

// Contains `async::IrqMethod`.
#include <lib/async/cpp/irq.h>

// Contains `fit::defer`.
#include <lib/fit/defer.h>

// Contains `zx::interrupt`.
#include <lib/zx/interrupt.h>

class MyDriver : public fdf::DriverBase2 {
 public:
  zx::result<> Start(fdf::DriverContext context) override {
    // ... Connect to FIDL service and get interrupt handle ...
    // interrupt_ = std::move(interrupt->value()->interrupt);

    interrupt_handler_.set_object(interrupt_.get());
    zx_status_t status = interrupt_handler_.Begin(dispatcher());
    if (status != ZX_OK) {
      return zx::error(status);
    }

    return zx::ok();
  }

 private:
  void HandleInterrupt(async_dispatcher_t* dispatcher, async::IrqBase* irq,
                       zx_status_t status, const zx_packet_interrupt_t* interrupt_packet) {
    if (status != ZX_OK) {
      return;
    }

    // Use defer to ensure the interrupt is acknowledged on all exit paths.
    // Failing to ack will prevent future interrupts from firing.
    auto ack_interrupt = fit::defer([this] {
      interrupt_.ack();
    });

    // Perform work in response to triggered interrupt.
  }

  zx::interrupt interrupt_;
  async::IrqMethod<MyDriver, &MyDriver::HandleInterrupt> interrupt_handler_{this};
};
```

## Common Pitfalls

* **Forgetting to acknowledge the interrupt**: Failing to call
  `interrupt_.ack()` will prevent the interrupt from triggering again. Use
  `fit::defer` as shown in the example to avoid this.
* **Blocking the dispatcher**: The interrupt handler runs on the dispatcher
  passed to `Begin()`. If the handler blocks or performs heavy computation, it
  will starve other tasks on that dispatcher. Offload heavy work to a separate
  thread or use asynchronous primitives if necessary.

## Further Reading

* [Handle Interrupts in a
  Driver](/docs/development/drivers/developer_guide/handle-interrupts-in-a-driver.md)
  - Comprehensive Fuchsia developer guide covering implementation and testing in
    both C++ and Rust.
* [Interrupts Reference](/docs/reference/kernel_objects/interrupts.md) - Kernel
  object reference detailing Zircon interrupts and the
  `ZX_VIRTUAL_INTERRUPT_UNTRIGGERED` signal.
* For guidance on testing interrupts, see the [Driver Interrupt Testing
  (C++)](/src/devices/skills/driver_interrupt/testing/cpp/SKILL.md) skill.
