// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_UART_EXYNOS_USI_H_
#define LIB_UART_EXYNOS_USI_H_

#include <lib/zbi-format/driver-config.h>
#include <lib/zbi-format/zbi.h>

#include <optional>

#include <hwreg/bitfields.h>

#include "interrupt.h"
#include "uart.h"

namespace uart::exynos_usi {

inline constexpr uint32_t kMaxFifoDepth = 256;

// line control register
// ULCON
struct LineControlRegister : public hwreg::RegisterBase<LineControlRegister, uint32_t> {
  // 31:6 Reserved.
  DEF_FIELD(5, 3, parity_mode);
  DEF_BIT(2, num_stop_bits);
  DEF_FIELD(1, 0, word_length);

  static auto Get() { return hwreg::RegisterAddr<LineControlRegister>(0); }
};

// general control register
// UCON
struct ControlRegister : public hwreg::RegisterBase<ControlRegister, uint32_t> {
  // 31:23 Reserved.
  DEF_FIELD(22, 20, tx_dma_burst_size);
  // 19 Reserved.
  DEF_FIELD(18, 16, rx_dma_burst_size);
  DEF_FIELD(15, 12, rx_timeout_interrupt_interval);
  DEF_BIT(11, rx_timeout_empty_fifo_enable);
  DEF_BIT(10, rx_timeout_dma_suspend_enable);
  // 9 Reserved.
  DEF_BIT(8, rx_level_triggered);
  DEF_BIT(7, rx_timeout_enable);
  DEF_BIT(6, rx_error_status_interrupt_enable);
  DEF_BIT(5, loop_back_mode);
  DEF_BIT(4, send_break_signal);
  DEF_FIELD(3, 2, tx_mode);
  DEF_FIELD(1, 0, rx_mode);

  static auto Get() { return hwreg::RegisterAddr<ControlRegister>(0x4); }
};

// UMCON
struct ModemControlRegister : public hwreg::RegisterBase<ModemControlRegister, uint32_t> {
  // 31:8 Reserved.
  DEF_FIELD(7, 5, rts_trigger_level);
  DEF_BIT(4, auto_flow_control_enable);
  // 3:1 Reserved.
  DEF_BIT(0, nrts);

  static auto Get() { return hwreg::RegisterAddr<ModemControlRegister>(0xc); }
};

// Supported tx/rx modes for `ControlRegister`.
inline constexpr uint8_t kModeDisabled = 0b00;
inline constexpr uint8_t kModeIrqOrPolling = 0b01;
inline constexpr uint8_t kModeDma = 0b10;
inline constexpr uint8_t kModeReserved = 0b11;

// Fifo control register
//
// Fifo trigger levels are enumerated from 0 to 7, have a length of 3 bits, and represent
// the fraction trigger_level * fifo_depth / 8 bytes for an IRQ to be generated.
//
// UFCON
struct FifoControlRegister : public hwreg::RegisterBase<FifoControlRegister, uint32_t> {
  // 31:11 Reserved.
  DEF_FIELD(10, 8, tx_fifo_trigger_level);
  // 7 Reserved.
  DEF_FIELD(6, 4, rx_fifo_trigger_level);
  // 3 Reserved.
  DEF_BIT(2, tx_fifo_reset);
  DEF_BIT(1, rx_fifo_reset);
  DEF_BIT(0, fifo_enable);

  static auto Get() { return hwreg::RegisterAddr<FifoControlRegister>(0x8); }
};

// UERSTAT
struct RxErrorStatusRegister : public hwreg::RegisterBase<RxErrorStatusRegister, uint32_t> {
  // 31:4 Reserved.
  DEF_BIT(3, break_signal);
  DEF_BIT(2, frame_error);
  DEF_BIT(1, parity_error);
  DEF_BIT(0, overrun_error);

  static auto Get() { return hwreg::RegisterAddr<RxErrorStatusRegister>(0x14); }
};

// UFSTAT
struct FifoStatusRegister : public hwreg::RegisterBase<FifoStatusRegister, uint32_t> {
  // 31:25 Reserved.
  DEF_BIT(24, tx_fifo_full);
  DEF_FIELD(23, 16, tx_fifo_count);
  // 15:10 Reserved.
  DEF_BIT(9, rx_fifo_error);
  DEF_BIT(8, rx_fifo_full);
  DEF_FIELD(7, 0, rx_fifo_count);

  static auto Get() { return hwreg::RegisterAddr<FifoStatusRegister>(0x18); }
};

// UTXH
struct TxBufferRegister : public hwreg::RegisterBase<TxBufferRegister, uint32_t> {
  // 31:8 Reserved.
  DEF_FIELD(7, 0, data);

  static auto Get() { return hwreg::RegisterAddr<TxBufferRegister>(0x20); }
};

// URXH
struct RxBufferRegister : public hwreg::RegisterBase<RxBufferRegister, uint32_t> {
  // 31:8 Reserved.
  DEF_FIELD(7, 0, data);

  static auto Get() { return hwreg::RegisterAddr<RxBufferRegister>(0x24); }
};

// UINTP
struct InterruptPendingRegister : public hwreg::RegisterBase<InterruptPendingRegister, uint32_t> {
  // 31:4 Reserved.
  DEF_BIT(3, cts);
  DEF_BIT(2, tx);
  DEF_BIT(1, error);
  DEF_BIT(0, rx);

  static auto Get() { return hwreg::RegisterAddr<InterruptPendingRegister>(0x30); }
};

// UINTS
struct InterruptSourceRegister : public hwreg::RegisterBase<InterruptSourceRegister, uint32_t> {
  // 31:4 Reserved.
  DEF_BIT(3, source_cts);
  DEF_BIT(2, source_tx);
  DEF_BIT(1, source_error);
  DEF_BIT(0, source_rx);

  static auto Get() { return hwreg::RegisterAddr<InterruptSourceRegister>(0x34); }
};

// UINTM
struct InterruptMaskRegister : public hwreg::RegisterBase<InterruptMaskRegister, uint32_t> {
  // 31:4 Reserved.
  DEF_BIT(3, mask_cts);
  DEF_BIT(2, mask_tx);
  DEF_BIT(1, mask_error);
  DEF_BIT(0, mask_rx);

  static auto Get() { return hwreg::RegisterAddr<InterruptMaskRegister>(0x38); }
};

// Universal Serial Interface Registers (Usi*Register)

// Expect the USI to be in UART mode. This Register is here for completion, but it should not be
// medled with.
// USI_CONF
struct UsiConfigRegister : public hwreg::RegisterBase<UsiConfigRegister, uint32_t> {
  // 31:3 Reserved.
  DEF_BIT(2, config_i2c);
  DEF_BIT(1, config_spi);
  DEF_BIT(0, config_uart);

  static auto Get() { return hwreg::RegisterAddr<UsiConfigRegister>(0xc0); }
};

// USI_CON
struct UsiControlRegister : public hwreg::RegisterBase<UsiControlRegister, uint32_t> {
  // 31:1 Reserved.
  DEF_BIT(0, reset);

  static auto Get() { return hwreg::RegisterAddr<UsiControlRegister>(0xc4); }
};

// USI_OPTION
struct UsiOptionRegister : public hwreg::RegisterBase<UsiOptionRegister, uint32_t> {
  // 31:9 Reserved.
  DEF_BIT(8, uart_high_speed_mode);
  // 7 Reserved.
  // 6:5 SPI mode related, unused.
  DEF_BIT(4, uart_legacy_auto_flow_control);
  // 3 Reserved.
  // Hardware Auto Clock Gating -  HWACG
  // Use clock_stop_on = 1 and clock_req_on = 0 for UART mode.
  DEF_BIT(2, hwacg_clock_stop_on);
  DEF_BIT(1, hwacg_clock_req_on);
  // 0 usi_master. Whether SPI or I2C mode are in master mode.
  static auto Get() { return hwreg::RegisterAddr<UsiOptionRegister>(0xc8); }
};

// USI - FIFO Depth Register(read-only).
// FIFO_DEPTH
struct UsiFifoDepthRegister : public hwreg::RegisterBase<UsiFifoDepthRegister, uint32_t> {
  // 31:25 Reserved.
  DEF_FIELD(24, 16, tx_fifo_depth);
  // 15:9 Reserved.
  DEF_FIELD(8, 0, rx_fifo_depth);

  static auto Get() { return hwreg::RegisterAddr<UsiFifoDepthRegister>(0xdc); }
};

// The number of `IoSlots` used by this driver, determined by the last accessed register, see
// `UsiFifoDepthRegister`. For unscaled MMIO, this corresponds to the size of the MMIO region
// from a provided base address.
static constexpr size_t kIoSlots = 0xdc + sizeof(uint32_t);

struct Driver : public DriverBase<Driver, ZBI_KERNEL_DRIVER_EXYNOS_USI_UART, zbi_dcfg_simple_t,
                                  IoRegisterType::kMmio8, kIoSlots> {
  using Base = DriverBase<Driver, ZBI_KERNEL_DRIVER_EXYNOS_USI_UART, zbi_dcfg_simple_t,
                          IoRegisterType::kMmio8, kIoSlots>;

 public:
  static constexpr std::string_view kConfigName = "exynos_usi";

  template <typename... Args>
  explicit Driver(Args&&... args) : Base(std::forward<Args>(args)...) {}

  template <class IoProvider>
  void Init(IoProvider& io) {
    // Do a very basic setup to ensure the RX and TX path is enabled and interrupts
    // are masked.

    // Read the fifo depth
    auto fifo_depth = UsiFifoDepthRegister::Get().ReadFrom(io.io());
    rx_fifo_depth_ = fifo_depth.rx_fifo_depth();
    tx_fifo_depth_ = fifo_depth.tx_fifo_depth();

    // Prevent the clock from being gated during low power states.
    UsiOptionRegister::Get()
        .FromValue(0)
        .set_hwacg_clock_req_on(true)
        .set_hwacg_clock_stop_on(false)
        .WriteTo(io.io());

    // Disable H/W Flow Control and drive nRTS low.
    ModemControlRegister::Get()
        .ReadFrom(io.io())
        .set_auto_flow_control_enable(false)
        .set_nrts(false)
        .WriteTo(io.io());

    // Mask all IRQs
    InterruptMaskRegister::Get()
        .FromValue(0)
        .set_mask_cts(true)
        .set_mask_tx(true)
        .set_mask_error(true)
        .set_mask_rx(true)
        .WriteTo(io.io());

    // Disable fifo
    FifoControlRegister::Get()
        .FromValue(0)
        // TX when below 2/8 of the TX fifo depth.
        .set_tx_fifo_trigger_level(2)
        // RX IRQ for every byte in the queue.
        .set_rx_fifo_trigger_level(0)
        .set_fifo_enable(false)
        .set_tx_fifo_reset(true)
        .set_rx_fifo_reset(true)
        .WriteTo(io.io());

    // Reset operation is completed when both bits are cleared.
    auto fcr = FifoControlRegister::Get().ReadFrom(io.io());
    while (fcr.tx_fifo_reset() || fcr.rx_fifo_reset()) {
      arch::Yield();
      fcr.ReadFrom(io.io());
    }
    fcr.set_fifo_enable(true).WriteTo(io.io());

    // Enable Rx/Tx.
    ControlRegister::Get()
        .FromValue(0)
        .set_tx_mode(kModeIrqOrPolling)
        .set_rx_mode(kModeIrqOrPolling)
        .WriteTo(io.io());
  }

  template <class IoProvider>
  bool TxReady(IoProvider& io) {
    return !FifoStatusRegister::Get().ReadFrom(io.io()).tx_fifo_full();
  }

  template <class IoProvider, typename It1, typename It2>
  auto Write(IoProvider& io, bool, It1 it, const It2& end) {
    TxBufferRegister::Get().FromValue(0).set_data(*it).WriteTo(io.io());
    return ++it;
  }

  template <class IoProvider>
  std::optional<uint8_t> Read(IoProvider& io) {
    auto fsr = FifoStatusRegister::Get().ReadFrom(io.io());
    if (fsr.rx_fifo_count() == 0) {
      return {};
    }
    // In order to discard any error, this register must be read anyway.
    // The error action will determine whether we should discard the data
    // or return it.
    auto rxdhr = RxBufferRegister::Get().ReadFrom(io.io());
    switch (RxErrorAction(io)) {
      case ErrorAction::kNoAction:
      case ErrorAction::kPreserveData:
        return rxdhr.data();
      case ErrorAction::kDiscardData:
        return std::nullopt;
    }
    __UNREACHABLE;
  }

  template <class IoProvider>
  void EnableTxInterrupt(IoProvider& io, bool enable = true) {
    InterruptMaskRegister::Get().ReadFrom(io.io()).set_mask_tx(!enable).WriteTo(io.io());
  }

  template <class IoProvider>
  void EnableRxInterrupt(IoProvider& io, bool enable = true) {
    InterruptMaskRegister::Get().ReadFrom(io.io()).set_mask_rx(!enable).WriteTo(io.io());
  }

  template <typename IoProvider, typename IrqProvider>
  void InitInterrupt(IoProvider& io, IrqProvider& irq) {
    ControlRegister::Get()
        .ReadFrom(io.io())
        // Default(3): 32 frame bit time.
        .set_rx_timeout_interrupt_interval(3)
        .set_rx_timeout_enable(true)
        .set_rx_timeout_empty_fifo_enable(false)
        .set_rx_error_status_interrupt_enable(false)
        .set_rx_level_triggered(cfg_.flags & ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED ? true
                                                                                         : false)
        .WriteTo(io.io());

    irq.SetInterruptsEnabled(true);
    EnableRxInterrupt(io);
  }

  template <class IoProvider, class Lock, class Waiter, class Tx, class Rx>
  void Interrupt(IoProvider& io, Lock& lock, Waiter& waiter, Tx&& tx, Rx&& rx) {
    size_t drained_rx = 0;
    // Stop gap of number of drained characters within a single IRQ.
    // The actual bound is 2 * `kMaxFifoDepth`, that is entering the loop
    // while having read `kMaxFifoDepth` - 1, and draining a full fifo.
    while (drained_rx < kMaxFifoDepth) {
      bool rx_disabled = false;
      auto fsr = FifoStatusRegister::Get().ReadFrom(io.io());
      auto ipr = InterruptPendingRegister::Get().ReadFrom(io.io());
      auto ack = InterruptPendingRegister::Get().FromValue(0);
      // No characters to read guarantees that this will be the last loop within this IRQ.
      // Additionally `drained_rx` cannot be higher than `kMaxFifoDepth` so the operation below
      // cannot overflow.
      drained_rx += fsr.rx_fifo_count() == 0 ? kMaxFifoDepth : fsr.rx_fifo_count();
      for (size_t i = 0; ipr.rx() && i < fsr.rx_fifo_count() && !rx_disabled; ++i) {
        // There is a parallel error fifo for each read character, we must
        // pop the error for the next character whenever we read it, and choose
        // appropriate action accordingly.
        ack.set_rx(true);
        auto rxdhr = RxBufferRegister::Get().ReadFrom(io.io());
        auto rx_irq = RxInterrupt(
            lock,  //
            [&]() { return rxdhr.data(); },
            [&]() {
              // If the buffer is full, disable the receive interrupt instead
              // and stop checking.
              EnableRxInterrupt(io, false);
              rx_disabled = true;
            });
        switch (RxErrorAction(io)) {
          case ErrorAction::kPreserveData:
            ack.set_error(true);
            __FALLTHROUGH;
          case ErrorAction::kNoAction:
            rx(rx_irq);
            break;
          case ErrorAction::kDiscardData:
            ack.set_error(true);
            continue;
        };
      }

      // Check Tx.
      if (ipr.tx() && !fsr.tx_fifo_full()) {
        ack.set_tx(true);
        auto tx_irq = TxInterrupt(lock, waiter, [&]() { EnableTxInterrupt(io, false); });
        tx(tx_irq);
      }

      // Commit handled IRQ signals to Interrupt Pending Register. By writing
      // 1 to each handled field, the IRQ is acknowledged and the signal is cleared.
      ack.WriteTo(io.io());
    }
  }

  template <class IoProvider>
  void SetLineControl(IoProvider& io, std::optional<DataBits> data_bits,
                      std::optional<Parity> parity, std::optional<StopBits> stop_bits) {
    auto lcr = LineControlRegister::Get().ReadFrom(io.io());
    uint8_t parity_mode = 0;
    switch (parity.value_or(Parity::kNone)) {
      case Parity::kNone:
        parity_mode = 0b000;
        break;
      case Parity::kEven:
        parity_mode = 0b101;
        break;
      case Parity::kOdd:
        parity_mode = 0b100;
        break;
    }

    uint8_t stop_bit_mode = stop_bits.value_or(StopBits::k1) == StopBits::k1 ? 0 : 1;

    uint8_t word_length_mode = 0b11;
    switch (data_bits.value_or(DataBits::k8)) {
      case DataBits::k5:
        // 0
        word_length_mode = 0b00;
        break;
      case DataBits::k6:
        // 1
        word_length_mode = 0b01;
        break;
      case DataBits::k7:
        // 2
        word_length_mode = 0b10;
        break;
      case DataBits::k8:
        // 3
        word_length_mode = 0b11;
        break;
    }

    lcr.set_parity_mode(parity_mode)
        .set_num_stop_bits(stop_bit_mode)
        .set_word_length(word_length_mode)
        .WriteTo(io.io());
  }

 private:
  enum class ErrorAction : uint8_t {
    // No error is detected and data is ok.
    kNoAction,
    // Error is detected and data is corrupted. (e.g. frame or parity errors.)
    kDiscardData,
    // Error is detected but data is ok. (overrun error or break signal.)
    kPreserveData,
  };

  template <class IoProvider>
  ErrorAction RxErrorAction(IoProvider& io) {
    auto esr = RxErrorStatusRegister::Get().ReadFrom(io.io());
    if (esr.frame_error() || esr.parity_error()) {
      return ErrorAction::kDiscardData;
    }

    if (esr.overrun_error() || esr.break_signal()) {
      return ErrorAction::kPreserveData;
    }

    return ErrorAction::kNoAction;
  }

  uint32_t rx_fifo_depth_ = 0;
  uint32_t tx_fifo_depth_ = 0;
};

}  // namespace uart::exynos_usi

#endif  // LIB_UART_EXYNOS_USI_H_
