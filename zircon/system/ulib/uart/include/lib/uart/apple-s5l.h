// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_UART_APPLE_S5L_H_
#define LIB_UART_APPLE_S5L_H_

#include <lib/zbi-format/driver-config.h>

#include <array>
#include <optional>
#include <string_view>

#include <hwreg/bitfields.h>

#include "interrupt.h"
#include "uart.h"

namespace uart::apple_s5l {

struct LineControlRegister : hwreg::RegisterBase<LineControlRegister, uint32_t> {
  DEF_FIELD(5, 3, parity);
  DEF_BIT(2, two_stop_bits);
  DEF_FIELD(1, 0, data_bits);
  static auto Get() { return hwreg::RegisterAddr<LineControlRegister>(0x00); }
};

struct ControlRegister : hwreg::RegisterBase<ControlRegister, uint32_t> {
  DEF_BIT(13, tx_threshold_irq_enable);
  DEF_BIT(12, rx_threshold_irq_enable);
  DEF_BIT(9, rx_timeout_irq_enable);
  DEF_FIELD(3, 2, tx_mode);
  DEF_FIELD(1, 0, rx_mode);
  static auto Get() { return hwreg::RegisterAddr<ControlRegister>(0x04); }
};

struct StatusRegister : hwreg::RegisterBase<StatusRegister, uint32_t> {
  DEF_BIT(9, rx_timeout_irq);
  DEF_BIT(5, tx_threshold_irq);
  DEF_BIT(4, rx_threshold_irq);
  DEF_BIT(2, tx_empty);
  DEF_BIT(1, tx_buffer_empty);
  DEF_BIT(0, rx_ready);
  static auto Get() { return hwreg::RegisterAddr<StatusRegister>(0x10); }
};

struct TxRegister : hwreg::RegisterBase<TxRegister, uint32_t> {
  DEF_FIELD(7, 0, data);
  static auto Get() { return hwreg::RegisterAddr<TxRegister>(0x20); }
};

struct RxRegister : hwreg::RegisterBase<RxRegister, uint32_t> {
  DEF_FIELD(7, 0, data);
  static auto Get() { return hwreg::RegisterAddr<RxRegister>(0x24); }
};

// Retains the baud rate established by firmware.
class Driver : public DriverBase<Driver, ZBI_KERNEL_DRIVER_APPLE_S5L_UART, zbi_dcfg_simple_t,
                                 IoRegisterType::kMmio8, 0x28> {
 public:
  using Base = DriverBase<Driver, ZBI_KERNEL_DRIVER_APPLE_S5L_UART, zbi_dcfg_simple_t,
                          IoRegisterType::kMmio8, 0x28>;
  using Base::Base;

  static constexpr std::string_view kConfigName = "apple_s5l";
  static constexpr std::array<std::string_view, 1> kDevicetreeBindings = {"apple,s5l-uart"};

  template <class IoProvider>
  void Init(IoProvider& io) {
    ControlRegister::Get()
        .ReadFrom(io.io())
        .set_tx_threshold_irq_enable(false)
        .set_rx_threshold_irq_enable(false)
        .set_rx_timeout_irq_enable(false)
        .set_tx_mode(1)
        .set_rx_mode(1)
        .WriteTo(io.io());
  }

  template <class IoProvider>
  bool TxReady(IoProvider& io) {
    return StatusRegister::Get().ReadFrom(io.io()).tx_buffer_empty();
  }

  template <class IoProvider, class It, class End>
  It Write(IoProvider& io, bool, It it, const End& end) {
    if (it != end) {
      TxRegister::Get().FromValue(0).set_data(static_cast<uint8_t>(*it++)).WriteTo(io.io());
    }
    return it;
  }

  template <class IoProvider>
  std::optional<uint8_t> Read(IoProvider& io) {
    if (!StatusRegister::Get().ReadFrom(io.io()).rx_ready()) {
      return std::nullopt;
    }
    return RxRegister::Get().ReadFrom(io.io()).data();
  }

  template <class IoProvider>
  void SetLineControl(IoProvider& io, std::optional<DataBits> data_bits,
                      std::optional<Parity> parity, std::optional<StopBits> stop_bits) {
    auto control = LineControlRegister::Get().ReadFrom(io.io());
    if (data_bits) {
      control.set_data_bits(static_cast<uint32_t>(*data_bits));
    }
    if (parity) {
      switch (*parity) {
        case Parity::kNone:
          control.set_parity(0);
          break;
        case Parity::kOdd:
          control.set_parity(4);
          break;
        case Parity::kEven:
          control.set_parity(5);
          break;
      }
    }
    if (stop_bits) {
      control.set_two_stop_bits(*stop_bits == StopBits::k2);
    }
    control.WriteTo(io.io());
  }

  template <class IoProvider, class IrqProvider>
  void InitInterrupt(IoProvider& io, IrqProvider& irq) {
    StatusRegister::Get().FromValue(kInterruptBits).WriteTo(io.io());
    irq.SetInterruptsEnabled(true);
    EnableRxInterrupt(io);
  }

  template <class IoProvider>
  void EnableTxInterrupt(IoProvider& io, bool enable = true) {
    ControlRegister::Get().ReadFrom(io.io()).set_tx_threshold_irq_enable(enable).WriteTo(io.io());
  }

  template <class IoProvider>
  void EnableRxInterrupt(IoProvider& io, bool enable = true) {
    ControlRegister::Get()
        .ReadFrom(io.io())
        .set_rx_threshold_irq_enable(enable)
        .set_rx_timeout_irq_enable(enable)
        .WriteTo(io.io());
  }

  template <class IoProvider, class Lock, class Waiter, class Tx, class Rx>
  void Interrupt(IoProvider& io, Lock& lock, Waiter& waiter, Tx&& tx, Rx&& rx) {
    auto status = StatusRegister::Get().ReadFrom(io.io());
    const bool tx_ready = status.tx_threshold_irq() && status.tx_buffer_empty();
    // Only the interrupt flags are W1C; never echo the live FIFO status bits.
    StatusRegister::Get().FromValue(status.reg_value() & kInterruptBits).WriteTo(io.io());

    if (status.rx_threshold_irq() || status.rx_timeout_irq()) {
      bool disabled = false;
      // Bound each invocation even if the sender continuously supplies data.
      for (size_t count = 0; count < 256 && status.rx_ready() && !disabled; ++count) {
        auto rx_irq = RxInterrupt(
            lock, [&]() { return RxRegister::Get().ReadFrom(io.io()).data(); },
            [&]() {
              EnableRxInterrupt(io, false);
              disabled = true;
            });
        rx(rx_irq);
        if (!disabled) {
          status.ReadFrom(io.io());
        }
      }
    }
    if (tx_ready) {
      auto tx_irq = TxInterrupt(lock, waiter, [&]() { EnableTxInterrupt(io, false); });
      tx(tx_irq);
    }
  }

 private:
  static constexpr uint32_t kInterruptBits = (1u << 9) | (1u << 5) | (1u << 4);
};

}  // namespace uart::apple_s5l

#endif  // LIB_UART_APPLE_S5L_H_
