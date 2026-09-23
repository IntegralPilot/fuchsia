---
name: driver-interrupt-testing-cpp
description: >
  Test IRQ / interrupt handling in a C++ DFv2 driver. Use when a unit test
  must create a virtual interrupt (ZX_INTERRUPT_VIRTUAL), inject a duplicate
  into the driver via a fake FIDL service, fire it with interrupt.trigger(),
  and assert the driver acked by waiting on ZX_VIRTUAL_INTERRUPT_UNTRIGGERED
  with async::WaitMethod inside an fdf_testing::Environment. For implementing
  interrupts in the driver use the C++ interrupt implementation skill; for
  Rust tests use the Rust interrupt testing skill.
---

# Driver Interrupt Testing (C++) (DFv2)

## Dependencies

Testing with [`async::WaitMethod`](/sdk/lib/async/include/lib/async/cpp/wait.h)
and using [`zx::interrupt`](/zircon/system/ulib/zx/include/lib/zx/interrupt.h)
requires `async-cpp` and `zx`:

**GN:**
```gn
deps = [
  "//sdk/lib/async:async-cpp",  # For `async::WaitMethod`
  "//zircon/system/ulib/zx",    # For `zx::interrupt`
]
```

**Bazel:**
```bazel
deps = [
    "@fuchsia_sdk//pkg/async-cpp",  # For `async::WaitMethod`
    "@fuchsia_sdk//pkg/zx",         # For `zx::interrupt`
]
```

## Provide an Interrupt

The test should create a virtual interrupt to provide to the driver.
```cpp
zx::interrupt interrupt;
ASSERT_EQ(
  zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt),
  ZX_OK);
```
Duplicate this interrupt and send the duplicate to the driver (usually via a
fake FIDL service).
```cpp
zx::interrupt duplicate;
ASSERT_EQ(interrupt.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate), ZX_OK);
```

## Trigger an Interrupt

The test can trigger a virtual interrupt like so:
```cpp
ASSERT_EQ(
  interrupt.trigger(0, zx::clock::get_boot()),
  ZX_OK);
```

## Verify Interrupt Acknowledgment

To verify that the driver has acknowledged the interrupt (by calling
`zx_interrupt_ack`), the test can listen for the
`ZX_VIRTUAL_INTERRUPT_UNTRIGGERED` signal.

When the driver acknowledges the interrupt, the system asserts this signal on
the virtual interrupt object. The test can use `async::WaitMethod` to
asynchronously wait for this signal on its handle of the interrupt.

## Review Example Test Setup

```cpp
// Contains `async::WaitMethod`.
#include <lib/async/cpp/wait.h>

// Contains `zx::interrupt`.
#include <lib/zx/interrupt.h>

// Contains `ZX_INTERRUPT_VIRTUAL`.
#include <zircon/types.h>

class MyDriverEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &interrupt_);

    zx::interrupt duplicate;
    interrupt_.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate);
    // Send duplicate to driver...

    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    interrupt_ack_handler_.set_object(interrupt_.get());
    interrupt_ack_handler_.Begin(dispatcher);

    return zx::ok();
  }

 private:
  void HandleInterruptAck(async_dispatcher_t* dispatcher, async::WaitBase* wait,
                          zx_status_t status, const zx_packet_signal_t* signal) {
    if (status != ZX_OK) {
      FAIL();
    }

    // Re-arm the listener.
    wait->Begin(dispatcher);
  }

  zx::interrupt interrupt_;
  async::WaitMethod<MyDriverEnvironment, &MyDriverEnvironment::HandleInterruptAck>
    interrupt_ack_handler_{this, ZX_HANDLE_INVALID, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED, ZX_WAIT_ASYNC_EDGE};
};
```

## Common Pitfalls

* **Forgetting to re-arm the wait:** In the handler callback (e.g.,
  `HandleInterruptAck`), you must call `wait->Begin(dispatcher)` to continue
  listening for subsequent acknowledgments. If you forget this, the test will
  only detect the first interrupt acknowledgment.
* **Not running the dispatcher:** Virtual interrupts and `async::Wait` rely on
  the async dispatcher. If your test triggers an interrupt but doesn't run the
  dispatcher (e.g., via `RunLoopUntilIdle` or similar), the handler will never
  be called.

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
  (C++)](/src/devices/skills/driver_interrupt/implementation/cpp/SKILL.md)
  skill.
