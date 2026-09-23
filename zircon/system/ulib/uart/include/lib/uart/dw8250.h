// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_UART_DW8250_H_
#define LIB_UART_DW8250_H_

#include <lib/acpi_lite/debug_port.h>
#include <lib/stdcompat/array.h>
#include <lib/zbi-format/driver-config.h>
#include <zircon/limits.h>

#include <array>
#include <bit>
#include <optional>
#include <string_view>

#include <hwreg/bitfields.h>

#include "interrupt.h"
#include "uart.h"

// Designware 8250, a modified and distant relative of an 8250.

namespace uart::dw8250 {

constexpr uint32_t kDefaultBaudRate = 115200;
constexpr uint32_t kMaxBaudRate = 115200;
constexpr uint8_t kFifoDepthDw8250Minimum = 16;
// A conservative limit to prevent infinite hangs if the hardware busy bit gets stuck.
// 1,000,000 iterations with pause/yield instructions provide a sufficient timeout
// (e.g. at least several milliseconds), which is much longer than the time required
// to transmit a single character at standard baud rates.
constexpr size_t kWaitBusyLimit = 1'000'000;

enum class InterruptType : uint8_t {
  kModemStatus = 0b0000,
  kNone = 0b0001,
  kTxEmpty = 0b0010,
  kRxDataAvailable = 0b0100,
  kRxLineStatus = 0b0110,
  kDwBusyDetect = 0b0111,
  kCharTimeout = 0b1100,
};

class RxBufferRegister : public hwreg::RegisterBase<RxBufferRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 8);
  DEF_FIELD(7, 0, data);
  static auto Get() { return hwreg::RegisterAddr<RxBufferRegister>(0); }
};

class TxBufferRegister : public hwreg::RegisterBase<TxBufferRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 8);
  DEF_FIELD(7, 0, data);
  static auto Get() { return hwreg::RegisterAddr<TxBufferRegister>(0); }
};

class InterruptEnableRegister : public hwreg::RegisterBase<InterruptEnableRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 8);
  DEF_BIT(7, programmable_thre_interrupt_mode_enable);
  DEF_RSVDZ_FIELD(6, 5);
  DEF_BIT(4, lsr_clear_status_on_read);
  DEF_BIT(3, modem_status);
  DEF_BIT(2, line_status);
  DEF_BIT(1, tx_empty);
  DEF_BIT(0, rx_available);
  static auto Get() { return hwreg::RegisterAddr<SelfType>(1); }
};

class InterruptIdentRegister : public hwreg::RegisterBase<InterruptIdentRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 8);
  DEF_FIELD(7, 6, fifos_enabled);
  DEF_RSVDZ_FIELD(5, 4);
  DEF_ENUM_FIELD(InterruptType, 3, 0, interrupt_id);
  static auto Get() { return hwreg::RegisterAddr<InterruptIdentRegister>(2); }
};

class FifoControlRegister : public hwreg::RegisterBase<FifoControlRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 8);
  DEF_FIELD(7, 6, receiver_trigger);
  DEF_FIELD(5, 4, transmit_trigger);
  DEF_BIT(3, dma_mode);
  DEF_BIT(2, tx_fifo_reset);
  DEF_BIT(1, rx_fifo_reset);
  DEF_BIT(0, fifo_enable);

  static constexpr uint8_t kReceiveTriggerLevel1Char = 0b00;
  static constexpr uint8_t kReceiveTriggerLevelQuarter = 0b01;
  static constexpr uint8_t kReceiveTriggerLevelHalf = 0b10;
  static constexpr uint8_t kReceiveTriggerLevel2LessThanFull = 0b11;

  static constexpr uint8_t kTransmitTriggerLevelEmpty = 0b00;
  static constexpr uint8_t kTransmitTriggerLevel2Char = 0b01;
  static constexpr uint8_t kTransmitTriggerLevelQuarter = 0b01;
  static constexpr uint8_t kTransmitTriggerLevelHalf = 0b01;

  static auto Get() { return hwreg::RegisterAddr<SelfType>(2); }
};

class LineControlRegister : public hwreg::RegisterBase<LineControlRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 8);
  DEF_BIT(7, divisor_latch_access);
  DEF_BIT(6, break_control);
  DEF_BIT(5, stick_parity);
  DEF_BIT(4, even_parity);
  DEF_BIT(3, parity_enable);
  DEF_BIT(2, stop_bits);
  DEF_FIELD(1, 0, word_length);

  static constexpr uint8_t kWordLength5 = 0b00;
  static constexpr uint8_t kWordLength6 = 0b01;
  static constexpr uint8_t kWordLength7 = 0b10;
  static constexpr uint8_t kWordLength8 = 0b11;

  static constexpr uint8_t kStopBits1 = 0b0;
  static constexpr uint8_t kStopBits2 = 0b1;

  static auto Get() { return hwreg::RegisterAddr<LineControlRegister>(3); }
};

class ModemControlRegister : public hwreg::RegisterBase<ModemControlRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 7);
  DEF_BIT(6, irda_sir_mode_enable);
  DEF_BIT(5, automatic_flow_control_enable);
  DEF_BIT(4, loop);
  DEF_BIT(3, auxiliary_out_2);
  DEF_BIT(2, auxiliary_out_1);
  DEF_BIT(1, request_to_send);
  DEF_BIT(0, data_terminal_ready);
  static auto Get() { return hwreg::RegisterAddr<ModemControlRegister>(4); }
};

class LineStatusRegister : public hwreg::RegisterBase<LineStatusRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 10);
  DEF_BIT(9, address_received);
  DEF_BIT(8, receive_fifo_error);
  DEF_BIT(7, error_in_rx_fifo);
  DEF_BIT(6, tx_empty);
  DEF_BIT(5, tx_register_empty);
  DEF_BIT(4, break_interrupt);
  DEF_BIT(3, framing_error);
  DEF_BIT(2, parity_error);
  DEF_BIT(1, overrun_error);
  DEF_BIT(0, data_ready);
  static auto Get() { return hwreg::RegisterAddr<LineStatusRegister>(5); }
};

class ModemStatusRegister : public hwreg::RegisterBase<ModemStatusRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 8);
  DEF_BIT(7, data_carrier_detect);
  DEF_BIT(6, ring_indicator);
  DEF_BIT(5, data_set_ready);
  DEF_BIT(4, clear_to_send);
  DEF_BIT(3, delta_data_carrier_detect);
  DEF_BIT(2, trailing_edge_ring_indicator);
  DEF_BIT(1, delta_data_set_ready);
  DEF_BIT(0, delta_clear_to_send);
  static auto Get() { return hwreg::RegisterAddr<ModemStatusRegister>(6); }
};

class ScratchRegister : public hwreg::RegisterBase<ScratchRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 8);
  DEF_FIELD(7, 0, data);
  static auto Get() { return hwreg::RegisterAddr<ScratchRegister>(7); }
};

class DivisorLatchLowerRegister : public hwreg::RegisterBase<DivisorLatchLowerRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 8);
  DEF_FIELD(7, 0, data);
  static auto Get() { return hwreg::RegisterAddr<DivisorLatchLowerRegister>(0); }
};

class DivisorLatchUpperRegister : public hwreg::RegisterBase<DivisorLatchUpperRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 8);
  DEF_FIELD(7, 0, data);
  static auto Get() { return hwreg::RegisterAddr<DivisorLatchUpperRegister>(1); }
};

// DW8250 value add registers:

class UartStatusRegister : public hwreg::RegisterBase<UartStatusRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 5);
  // Bits 4...1 are optional, present if CPR.FIFO_STAT is set.
  DEF_BIT(4, receive_fifo_full);
  DEF_BIT(3, receive_fifo_not_empty);
  DEF_BIT(2, transmit_fifo_empty);
  DEF_BIT(1, transmit_fifo_not_full);
  DEF_BIT(0, uart_busy);
  static auto Get() { return hwreg::RegisterAddr<UartStatusRegister>(0x7c / 4); }
};

class TransmitFifoLevelRegister : public hwreg::RegisterBase<TransmitFifoLevelRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 12);
  // 12 bits: log2 + 1 of max fifo level 2048
  DEF_FIELD(11, 0, level);
  static auto Get() { return hwreg::RegisterAddr<TransmitFifoLevelRegister>(0x80 / 4); }
};

class ReceiveFifoLevelRegister : public hwreg::RegisterBase<ReceiveFifoLevelRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 12);
  // 12 bits: log2 + 1 of max fifo level 2048
  DEF_FIELD(11, 0, level);
  static auto Get() { return hwreg::RegisterAddr<ReceiveFifoLevelRegister>(0x84 / 4); }
};

class ComponentParameterRegister
    : public hwreg::RegisterBase<ComponentParameterRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 24);
  DEF_FIELD(23, 16, fifo_mode);  // size of the fifo
  DEF_RSVDZ_FIELD(15, 14);
  DEF_BIT(13, dma_extra);
  DEF_BIT(12, uart_add_encoded_params);
  DEF_BIT(11, shadow);
  DEF_BIT(10, fifo_stat);
  DEF_BIT(9, fifo_access);
  DEF_BIT(8, additional_feat);
  DEF_BIT(7, sir_lp_mode);
  DEF_BIT(6, sir_mode);
  DEF_BIT(5, thre_mode);
  DEF_BIT(4, afce_mode);
  DEF_RSVDZ_FIELD(3, 2);
  DEF_FIELD(1, 0, apb_data_width);

  static auto Get() { return hwreg::RegisterAddr<ComponentParameterRegister>(0xf4 / 4); }
};

// The scaled number of `IoSlots` used by this driver, which corresponds to the mmio window.
// For Scaled MMIO, this corresponds to the number of unscaled registers that need to be
// accessed by the implementation. The MMIO region size can be obtained by scaling the register
// by their access width(`sizeof(uint32_t)`).
inline constexpr uint32_t kIoSlots = 0x100;

// This provides the actual driver logic common to MMIO and PIO variants.
struct Driver : public DriverBase<Driver, ZBI_KERNEL_DRIVER_DW8250_UART, zbi_dcfg_simple_t,
                                  IoRegisterType::kMmio32, kIoSlots> {
  using Base = DriverBase<Driver, ZBI_KERNEL_DRIVER_DW8250_UART, zbi_dcfg_simple_t,
                          IoRegisterType::kMmio32, kIoSlots>;

  static constexpr std::string_view kConfigName = "dw8250";

  static constexpr auto kDevicetreeBindings =
      cpp20::to_array<std::string_view>({"snps,dw-apb-uart", "goog,goog-dw-apb-uart"});

  template <typename... Args>
  explicit Driver(Args&&... args) : Base(std::forward<Args>(args)...) {}

  static bool TrySelect(const devicetree::PropertyDecoder& decoder) {
    // Check that the compatible property contains a compatible devicetree binding.
    if (!Base::TrySelect(decoder)) {
      return false;
    }

    auto [io_width_prop, reg_shift_prop] = decoder.FindProperties("reg-io-width", "reg-shift");

    std::optional<uint32_t> io_width = io_width_prop ? io_width_prop->AsUint32() : std::nullopt;
    std::optional<uint32_t> reg_shift = reg_shift_prop ? reg_shift_prop->AsUint32() : std::nullopt;

    // Must provide io-width and reg-shift of 32 bits.
    return io_width == 4 && reg_shift == 2;
  }

  template <class IoProvider>
  void Init(IoProvider& io) {
    // Get basic config done so that tx functions.

    // Read the configuration register.
    auto cpr = ComponentParameterRegister::Get().ReadFrom(io.io());

    // If add encoded params is present, the CPR is a valid register.
    if (cpr.uart_add_encoded_params()) {
      fifo_depth_ = kFifoDepthDw8250Minimum;
      // 16...2048 byte fifos
      if (std::has_single_bit(cpr.fifo_mode())) {
        fifo_depth_ = 16 * cpr.fifo_mode();
      }

      // THRE mode allows programming the transmit interrupt fifo threshold to something other
      // than zero. It additionally changes the interpretation of LSR.THRE to whether or not the
      // transmit FIFO is full or not full, which is generally more useful.
      thre_mode_ = cpr.thre_mode();

      // FIFO status allows us to directly read how many bytes are in both the transmit and receive
      // FIFOs at any given point in time, which is maximally useful for efficiently filling and
      // reading them.
      fifo_stat_ = cpr.fifo_stat();
    }

    // Disable all interrupts.
    auto ier = InterruptEnableRegister::Get().FromValue(0);
    ier.set_rx_available(false);
    ier.set_tx_empty(false);
    ier.set_line_status(false);
    ier.set_modem_status(false);

    // If present, set programmable THRE mode.
    if (thre_mode_) {
      ier.set_programmable_thre_interrupt_mode_enable(true);
    }
    ier.WriteTo(io.io());

    // Clear and set up the FIFO
    auto fcr = FifoControlRegister::Get().FromValue(0);
    fcr.set_fifo_enable(true);
    fcr.set_rx_fifo_reset(true);
    fcr.set_tx_fifo_reset(true);
    if (thre_mode_) {
      fcr.set_transmit_trigger(FifoControlRegister::kTransmitTriggerLevel2Char);
    }
    fcr.set_receiver_trigger(FifoControlRegister::kReceiveTriggerLevel1Char);
    fcr.WriteTo(io.io());

    // Drive flow control bits high since we don't actively manage them.
    auto mcr = ModemControlRegister::Get().FromValue(0);
    mcr.set_data_terminal_ready(true).set_request_to_send(true).WriteTo(io.io());
  }

  template <class IoProvider>
  void SetLineControl(IoProvider& io, std::optional<DataBits> data_bits,
                      std::optional<Parity> parity, std::optional<StopBits> stop_bits) {
    constexpr uint32_t kDivisor = kMaxBaudRate / kDefaultBaudRate;

    // Wait for the USR[0] bit to clear.
    // Note that if this times out, DLAB has not been set yet in this call.
    // If DLAB was already set by a previous failed call, writing to LCR here
    // to clear it would be ignored anyway since the UART is still busy.
    if (!WaitDuringBusy(io)) {
      return;
    }

    LineControlRegister::Get().FromValue(0).set_divisor_latch_access(true).WriteTo(io.io());

    DivisorLatchLowerRegister::Get()
        .FromValue(0)
        .set_data(static_cast<uint32_t>(kDivisor))
        .WriteTo(io.io());
    DivisorLatchUpperRegister::Get()
        .FromValue(0)
        .set_data(static_cast<uint32_t>(kDivisor >> 8))
        .WriteTo(io.io());

    auto lcr = LineControlRegister::Get().FromValue(0).set_divisor_latch_access(false);

    if (data_bits) {
      const uint8_t word_length = [bits = *data_bits]() {
        switch (bits) {
          case DataBits::k5:
            return LineControlRegister::kWordLength5;
          case DataBits::k6:
            return LineControlRegister::kWordLength6;
          case DataBits::k7:
            return LineControlRegister::kWordLength7;
          case DataBits::k8:
            return LineControlRegister::kWordLength8;
        }
        ZX_PANIC("Unknown value for DataBits enum class (%u)", static_cast<unsigned int>(bits));
      }();
      lcr.set_word_length(word_length);
    }

    if (parity) {
      lcr.set_parity_enable(*parity != Parity::kNone).set_even_parity(*parity == Parity::kEven);
    }

    if (stop_bits) {
      const uint8_t num_stop_bits = [bits = *stop_bits]() {
        switch (bits) {
          case StopBits::k1:
            return LineControlRegister::kStopBits1;
          case StopBits::k2:
            return LineControlRegister::kStopBits2;
        }
        ZX_PANIC("Unknown value for StopBits enum class (%u)", static_cast<unsigned int>(bits));
      }();
      lcr.set_stop_bits(num_stop_bits);
    }

    // Best-effort attempt to clear DLAB. Note that if the wait times out, the subsequent
    // write to LCR will likely be ignored by the hardware because it remains busy. This
    // leaves DLAB=1, which effectively breaks future RX/TX and interrupt configuration.
    // Since we cannot easily log a warning or panic in this low-level library (which is
    // used in early boot/kernel where standard logging is unavailable, and printing to
    // the console would recurse on the dead UART anyway), we proceed with a best-effort
    // write and exit.
    (void)WaitDuringBusy(io);
    lcr.WriteTo(io.io());
  }

  template <class IoProvider>
  bool TxReady(IoProvider& io) {
    if (likely(fifo_stat_)) {
      // If fifo status is enabled, we can see if the tx fifo has any space by looking at
      // USR.TFNF.
      return UartStatusRegister::Get().ReadFrom(io.io()).transmit_fifo_not_full();
    }
    if (thre_mode_) {
      // When programmable THRE mode is enabled, LSR.THRE represents whether or not the TX FIFO
      // is full or not. 1 == full, 0 == not full. Return true if it's not full.
      return !LineStatusRegister::Get().ReadFrom(io.io()).tx_register_empty();
    }
    // The legacy meaning of this bit 1 == FIFO empty, 0 == not empty.
    return LineStatusRegister::Get().ReadFrom(io.io()).tx_register_empty();
  }

  template <class IoProvider, typename It1, typename It2>
  auto Write(IoProvider& io, bool, It1 it, const It2& end) {
    auto tx = TxBufferRegister::Get().FromValue(0);
    if (likely(fifo_stat_)) {
      // With fifo status, we can read the current level of the fifo and write as
      // many characters as will fit.
      auto tfl = TransmitFifoLevelRegister::Get().ReadFrom(io.io());
      auto space = fifo_depth_ - tfl.level();
      do {
        tx.set_data(*it).WriteTo(io.io());
      } while (++it != end && --space > 0);
    } else if (thre_mode_) {
      // With programmable THRE mode we can loop here until the TX is filled by checking
      // the status of LSR.THRE.
      do {
        tx.set_data(*it).WriteTo(io.io());
      } while (++it != end && !LineStatusRegister::Get().ReadFrom(io.io()).tx_register_empty());
    } else {
      // The FIFO is empty now and we know the size, so fill it completely.
      auto space = fifo_depth_;
      do {
        tx.set_data(*it).WriteTo(io.io());
      } while (++it != end && --space > 0);
    }
    return it;
  }

  template <class IoProvider>
  std::optional<uint8_t> Read(IoProvider& io) {
    if (LineStatusRegister::Get().ReadFrom(io.io()).data_ready()) {
      return RxBufferRegister::Get().ReadFrom(io.io()).data();
    }
    return {};
  }

  template <class IoProvider>
  void EnableTxInterrupt(IoProvider& io, bool enable = true) {
    auto ier = InterruptEnableRegister::Get().ReadFrom(io.io());
    ier.set_tx_empty(enable).WriteTo(io.io());
  }

  template <class IoProvider>
  void EnableRxInterrupt(IoProvider& io, bool enable = true) {
    auto ier = InterruptEnableRegister::Get().ReadFrom(io.io());
    ier.set_rx_available(enable).set_line_status(enable).WriteTo(io.io());
  }

  template <typename IoProvider, typename IrqProvider>
  void InitInterrupt(IoProvider& io, IrqProvider& irq) {
    // Enable receive interrupts.
    EnableRxInterrupt(io);

    // Since these are level triggered interrupts nominally, it's safe and correct to
    // enable the interrupt after configuring the hardware, since no interrupt edges can
    // be lost.
    irq.SetInterruptsEnabled(true);
  }

  template <class IoProvider, typename Lock, typename Waiter, typename Tx, typename Rx>
  void Interrupt(IoProvider& io, Lock& lock, Waiter& waiter, Tx&& tx, Rx&& rx) {
    auto iir = InterruptIdentRegister::Get();
    InterruptType id;
    do {
      id = iir.ReadFrom(io.io()).interrupt_id();
      switch (id) {
        case InterruptType::kNone:
          // Will break out of the loop below.
          break;
        case InterruptType::kRxLineStatus: {
          // Reading LSR will clear kRxLineStatus signal.
          LineStatusRegister::Get().ReadFrom(io.io());
          break;
        }
        case InterruptType::kRxDataAvailable:
        case InterruptType::kCharTimeout: {
          // Drain RX while the line status bit is ready.
          // possible optimization: be a bit more intelligent if the fifo status feature is
          // available and loop while reading the amount of bytes in the fifo.
          bool should_drain_rx = true;
          auto lsr = LineStatusRegister::Get().ReadFrom(io.io());
          for (; should_drain_rx && lsr.data_ready();
               lsr = LineStatusRegister::Get().ReadFrom(io.io())) {
            auto rx_irq = RxInterrupt(
                lock,  //
                [&]() { return RxBufferRegister::Get().ReadFrom(io.io()).data(); },
                [&]() {
                  // If the buffer is full, disable the receive interrupt instead and
                  // exit the loop.
                  EnableRxInterrupt(io, false);
                  should_drain_rx = false;
                });
            rx(rx_irq);
          }
          break;
        }
        case InterruptType::kTxEmpty: {
          // Either the TX fifo is empty or in TEMT mode it is below the TX fifo threshold.
          auto tx_irq = TxInterrupt(lock, waiter, [&]() { EnableTxInterrupt(io, false); });
          tx(tx_irq);
          break;
        }
        case InterruptType::kModemStatus: {
          // Reading MSR will clear kModemStatus signal.
          ModemStatusRegister::Get().ReadFrom(io.io());
          break;
        }
        case InterruptType::kDwBusyDetect:
          // From the manual:
          // "Master has tried to write to the Line Control Register while the DW_apb_uart is busy
          // (USR[0] is set to one)." Read the UART Status Register to clear it.
          UartStatusRegister::Get().ReadFrom(io.io());
          break;
      }
    } while (id != InterruptType::kNone);
  }

  template <class IoProvider>
  bool WaitDuringBusy(IoProvider& io) {
    // Wait for the busy bit in the USR register to be clear, up to a limit.
    for (size_t i = 0; i < kWaitBusyLimit; ++i) {
      if (!UartStatusRegister::Get().ReadFrom(io.io()).uart_busy()) {
        return true;
      }
      arch::Yield();
    }
    return false;
  }

 protected:
  uint32_t fifo_depth_ = kFifoDepthDw8250Minimum;
  bool thre_mode_ = false;  // Do we have programmable THRE mode?
  bool fifo_stat_ = false;  // Do we have a FIFO status register?
};

}  // namespace uart::dw8250

#endif  // LIB_UART_DW8250_H_
