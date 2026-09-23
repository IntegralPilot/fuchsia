// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/cbuf.h>
#include <lib/debuglog.h>
#include <lib/page/size.h>
#include <lib/root_resource_filter.h>
#include <lib/uart/all.h>
#include <lib/uart/null.h>
#include <lib/uart/qemu.h>
#include <lib/uart/uart.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zircon-internal/macros.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/time.h>
#include <stdint.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <cassert>
#include <type_traits>

#include <arch/arch_interrupt.h>
#include <dev/init.h>
#include <dev/interrupt.h>
#include <kernel/deadline.h>
#include <kernel/spinlock.h>
#include <kernel/timer.h>
#include <ktl/optional.h>
#include <ktl/type_traits.h>
#include <ktl/variant.h>
#include <lockdep/guard.h>
#include <platform/debug.h>
#include <platform/uart.h>

#include "platform.h"

namespace {

// No locks will be acquired.
struct NullLockPolicy {};

bool is_tx_irq_enabled = false;

template <typename LockPolicy, typename Guard>
using GuardSelector =
    std::conditional_t<std::is_same_v<LockPolicy, NullLockPolicy>, NullGuard, Guard>;

// Implements SyncPolicy as defined in <lib/uart/sync.h>
struct UartSyncPolicy {
  // While tempting, do *not* use a templated `using` statement here.  In some
  // places in code, we will need to deduce (via argument passing) what
  // `MemberOf` is.  Attempting to do this when we declare lock as a using alias
  // is not going to work in this situation.  The deduction is going to execute
  // against the underlying type, not the `using` statement.  So the final type
  // (in this case, a LockDep instance) would need to be a template with exactly
  // one argument which is identical to MemberOf.  In the case of LockDep locks,
  // this is *not* the case.
  //
  // Declaring this as a single-argument templated struct with no members which
  // inherits from the underlying LockDep type works around this issue.  Guards
  // and other things will auto-upcast to the proper underlying LockDep type,
  // and the only constructor needed/supported is the default constructor, so we
  // don't need to write a forwarding constructor to the underlying type.
  template <typename MemberOf>
  struct Lock : public DECLARE_SPINLOCK_WITH_TYPE(MemberOf, MonitoredSpinLock) {};

  template <typename LockPolicy>
  using Guard = GuardSelector<LockPolicy, Guard<MonitoredSpinLock, LockPolicy>>;

  class Waiter {
   public:
    enum class Blocking {
      // `Wait` is allowed to block callers, e.g. wait on an event.
      kYes,
      // `Wait` is not allowed to block callers, should spin instead.
      kNo,
    };

    template <typename Guard, typename T>
    void Wait(Guard& guard, T&& enable_tx_interrupt, Blocking blocking) TA_REQ(guard) {
      if (blocking == Blocking::kYes && is_tx_irq_enabled) {
        enable_tx_interrupt();
        guard.CallUnlocked([this]() { tx_fifo_not_full_.Wait(); });
      } else {
        // Drop the spinlock while spinning.
        guard.CallUnlocked([]() { arch::Yield(); });
      }
    }

    void Wake() { tx_fifo_not_full_.Signal(); }

   private:
    AutounsignalEvent tx_fifo_not_full_{true};
  };

  using DefaultLockPolicy = NullLockPolicy;

  template <typename LockType>
  static void AssertHeld(LockType& lock) TA_ASSERT(lock) TA_ASSERT(lock.lock()) {
    lock.lock().AssertHeld();
  }
};

class KernelIrqProvider {
 public:
  KernelIrqProvider() = default;
  ~KernelIrqProvider() = default;

  // No copy, no move.
  KernelIrqProvider(const KernelIrqProvider&) = delete;
  KernelIrqProvider& operator=(const KernelIrqProvider&) = delete;
  KernelIrqProvider(KernelIrqProvider&&) = delete;
  KernelIrqProvider& operator=(KernelIrqProvider&&) = delete;

  void Init(int irq_num) { irq_num_ = irq_num; }
  void SetInterruptsEnabled(bool enabled) {
    if (enabled) {
      unmask_interrupt(irq_num_);
    } else {
      mask_interrupt(irq_num_);
    }
  }

 private:
  int irq_num_{0};
};

uart::all::KernelDriver<PlatformUartIoProvider, UartSyncPolicy, KernelIrqProvider> gUart;

// Initialized by UartInitLate, provides buffered output of the uart,
// which helps readers catch up.
// Also provides synchronization mechanisms for character availability.
Cbuf rx_queue;

// Size of the rx queue. The bigger the buffer, the bigger the window for
// the reader to catch up. Useful when the incoming data is bursty.
constexpr size_t kRxQueueSize = 1024;

// When Polling is enabled, this will fire the polling callback for draining UART's RX Queue.
Timer gUartPollTimer;

constexpr zx_duration_mono_t kPollingPeriod = ZX_MSEC(10);
constexpr TimerSlack kPollingSlack = {ZX_MSEC(10), TIMER_SLACK_CENTER};

// Callback used by |gUartPollTimer| when deadline is met. See |Timer| interface for more
// information.
template <bool DrainUart>
void UartPoll(Timer* uart_timer, zx_instant_mono_t now, void* arg) {
  uart_timer->Set(Deadline(zx_time_add_duration(now, kPollingPeriod), kPollingSlack),
                  &UartPoll<true>, nullptr);
  if constexpr (DrainUart) {
    gUart.Visit([&](auto& driver) {
      // Drain until there is nothing else in the RX Queue of the device.
      while (auto c = driver.Read()) {
        rx_queue.WriteChar(*c);
      }
    });
  }
}

struct IrqConfig {
  interrupt_trigger_mode trigger;
  interrupt_polarity polarity;
};

ktl::optional<IrqConfig> GetIrqConfigFromFlags(uint32_t uart_flags) {
  if (uart_flags == 0) {
    return ktl::nullopt;
  }

  if ((uart_flags & (ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                     ZBI_KERNEL_DRIVER_IRQ_FLAGS_EDGE_TRIGGERED)) == 0) {
    return ktl::nullopt;
  }

  if ((uart_flags & (ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH |
                     ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_LOW)) == 0) {
    return ktl::nullopt;
  }

  // In order to configure the IRQ all information, trigger and polarity must be provided.
  // Otherwise the step must be omitted and let defaults take over.
  IrqConfig config = {};
  config.trigger = uart_flags & ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED
                       ? interrupt_trigger_mode::LEVEL
                       : interrupt_trigger_mode::EDGE;
  config.polarity = uart_flags & ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH
                        ? interrupt_polarity::HIGH
                        : interrupt_polarity::LOW;
  return config;
}

}  // namespace

bool platform_serial_enabled(void) { return !!gUart; }

void platform_serial_prepare_for_suspend() {
  if (!platform_serial_enabled()) {
    return;
  }

  if (BootOptions::Get()->allow_debug_uart_suspend) {
    gUart.Visit([&]<typename DriverType>(DriverType& driver) { driver.PrepareForSuspend(); });
  }
}

void platform_serial_wakeup_from_suspend() {
  if (!platform_serial_enabled()) {
    return;
  }

  if (BootOptions::Get()->allow_debug_uart_suspend) {
    gUart.Visit([&]<typename DriverType>(DriverType& driver) { driver.WakeupFromSuspend(); });
  }
}

void UartDriverHandoffEarly(const uart::all::Driver& serial) {
  gUart = serial;
  if constexpr (DPRINTF_ENABLED_FOR_LEVEL(INFO)) {
    if (gUart) {
      ktl::array<char, 128> buffer = {};
      StringFile file{buffer};
      fprintf(&file, "UART: Selected driver kernel.serial=");
      gUart.Unparse(&file);
      dprintf(INFO, "%.*s\n", static_cast<int>(file.as_string_view().size()),
              file.as_string_view().data());
    }
  }
}

void UartDriverHandoffPostVm(const uart::all::Driver& serial) {}

void UartDriverHandoffLate(const uart::all::Driver& serial) {
  // This buffer is needed even when serial is disabled, to prevent uninitialized
  // access to it.
  rx_queue.Initialize(kRxQueueSize, malloc(kRxQueueSize));

  if (!platform_serial_enabled()) {
    return;
  }

  // Check for interrupt support or explicitly polling uart and reserve the IO regions, preventing
  // userspace from claiming them.
  ktl::optional<uint32_t> uart_irq;
  bool polling_mode = false;
  gUart.Visit([&]<typename DriverType>(DriverType& driver) {
    using uart_type = typename DriverType::uart_type;
    using cfg_type = typename uart_type::config_type;
    if constexpr (uart::MmioDriver<uart_type>) {
      uart_irq = PlatformUartGetIrqNumber(driver.config().irq);
      uart::MmioRange aligned_mmio_range = driver.mmio_range().AlignedTo(kPageSize);
      root_resource_filter_add_deny_region(aligned_mmio_range.address, aligned_mmio_range.size,
                                           ZX_RSRC_KIND_MMIO);
    } else if constexpr (uart::PioDriver<uart_type>) {
      uart_irq = PlatformUartGetIrqNumber(driver.config().irq);
      uart::IoPortRange ioports = driver.ioports();
      root_resource_filter_add_deny_region(ioports.base, ioports.count, ZX_RSRC_KIND_IOPORT);
    } else {  // Only |uart::null::Driver| is expected to have a different configuration type.
      constexpr auto kIsNullDriver = ktl::is_same_v<uart_type, uart::null::Driver>;
      ZX_ASSERT_MSG(kIsNullDriver, "Unexpected UART Configuration.");
      // No IRQ Handler for null driver.
      return;
    }

    // Check for polling mode.
    if (!uart_irq || BootOptions::Get()->debug_uart_poll) {
      // Start the polling without performing any drain.
      UartPoll</*DrainUart=*/false>(&gUartPollTimer, current_mono_time(), nullptr);
      dprintf(INFO, "UART: POLLING mode enabled.\n");
      polling_mode = true;
      return;
    }

    static constexpr auto rx_irq_handler = [](auto& rx_interrupt) {
      // This check needs to be performed under a lock, such that we prevent operation
      // interleaving that would leave us in a blocked state.
      //
      // E.g.
      // Assume a simple MT scenario with one reader R and one writer R:
      //
      // * W: Observes the buffer is full.
      // * R: Reads a character. The buffer is now empty.
      // * R: Unmasks RX.
      // * W: Masks RX.
      //
      //  At this point, we have an empty buffer and RX interrupts are masked -
      //  we're stuck! Thus, to avoid this, we acquire the spinlock before
      //  checking if the buffer is full, and release after (conditionally)
      //  masking RX interrupts. By pairing this with the acquisition of the
      //  same lock around unmasking RX interrupts, we prevent the writer above
      //  from being interrupted by a read-and-unmask.
      char c;
      {
        Guard<MonitoredSpinLock, NoIrqSave> lock(&rx_interrupt.lock(), SOURCE_TAG);
        if (rx_queue.Full()) {
          // disables RX interrupts.
          rx_interrupt.DisableInterrupt();
          return;
        }
        c = static_cast<char>(rx_interrupt.ReadChar());
      }
      rx_queue.WriteChar(c);
    };

    static constexpr auto tx_irq_handler = [](auto& tx_interrupt) {
      // Mask the TX interrupt before signalling any blocked thread as there may
      // be a race between masking TX here below and unmasking by the blocked
      // thread.
      {
        Guard<MonitoredSpinLock, NoIrqSave> lock(&tx_interrupt.lock(), SOURCE_TAG);
        tx_interrupt.DisableInterrupt();
      }

      // Do not signal the event while holding the sync capability, this could lead
      // to invalid lock dependencies.
      tx_interrupt.Notify();
    };

    if constexpr (ktl::is_same_v<cfg_type, zbi_dcfg_simple_t>) {
      // Configure the interrupt if available.
      auto irq_config = GetIrqConfigFromFlags(driver.config().flags);
      if (irq_config) {
        zx_status_t irq_config_result =
            configure_interrupt(*uart_irq, irq_config->trigger, irq_config->polarity);
        DEBUG_ASSERT(irq_config_result == ZX_OK);
      }
    }

    // Register IRQ Handler.
    auto irq_handler_helper = [](void* cookie) {
      auto* drv = static_cast<DriverType*>(cookie);
      drv->Interrupt(tx_irq_handler, rx_irq_handler);
    };
    zx_status_t irq_register_result =
        register_permanent_int_handler(*uart_irq, interrupt_handler_t{&driver, irq_handler_helper});
    DEBUG_ASSERT(irq_register_result == ZX_OK);

    // Init UART Interrupts.
    driver.InitInterrupt(*uart_irq);
  });

  if (!polling_mode) {
    dprintf(INFO, "UART: IRQ driven RX: enabled\n");

    is_tx_irq_enabled = !dlog_bypass();
    dprintf(INFO, "UART: IRQ driven TX: %s\n", is_tx_irq_enabled ? "enabled" : "disabled");
  }
}

void platform_dputs_thread(const char* str, size_t len) {
  if (!platform_serial_enabled()) {
    return;
  }

  gUart.Visit([str, len](auto& driver) {
    driver.template Write<IrqSave>({str, len}, UartSyncPolicy::Waiter::Blocking::kYes);
  });
}

void platform_dputs_irq(const char* str, size_t len) {
  if (!platform_serial_enabled()) {
    return;
  }

  gUart.Visit([str, len](auto& driver) {
    driver.template Write<IrqSave>({str, len}, UartSyncPolicy::Waiter::Blocking::kNo);
  });
}

int platform_dgetc(char* c, bool wait) {
  if (!platform_serial_enabled()) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  auto read = rx_queue.ReadCharWithContext(wait);

  // 1 => Character read.
  if (read.is_ok()) {
    // This is safe because:
    //   * The RX IRQ handler is holding the UART lock while the queue is being inspected (Full) and
    //     the RX IRQ is being disabled.
    //   * The Read path, which is the only path which can transition the queue from full to not
    //     full, is not holding the the UART lock while inspecting, but the operations is deferred
    //     and acquires the lock before enabling interrupts.
    //
    // As a consequence, the IRQ RX Interrupt cannot be enabled by this path, until the RX IRQ
    // Handler, has disabled it and released the lock. This means there is no possible interleaving,
    // where both paths observe full queue, and we enable the RX IRQ followed by the IRQ RX Handler
    // disabling them.
    if (read->transitioned_from_full) {
      gUart.Visit([](auto& driver) { driver.template EnableRxInterrupt<IrqSave>(); });
    }
    *c = read->c;
    return 1;
  }

  // 0 => No character yet.
  if (read.status_value() == ZX_ERR_SHOULD_WAIT) {
    return 0;
  }

  // < 0 => Error.
  return read.status_value();
}

int platform_pgetc(char* c) {
  if (!platform_serial_enabled()) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  ktl::optional<char> read;
  gUart.Visit([&](auto& driver) { read = driver.Read(); });

  if (read) {
    *c = *read;
    return 0;
  }

  return -1;
}

void platform_pputc(char c) {
  if (!platform_serial_enabled()) {
    return;
  }

  gUart.Visit([c](auto& driver) { driver.Write({&c, 1}, UartSyncPolicy::Waiter::Blocking::kNo); });
}
