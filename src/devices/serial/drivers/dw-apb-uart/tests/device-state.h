// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SERIAL_DRIVERS_DW_APB_UART_TESTS_DEVICE_STATE_H_
#define SRC_DEVICES_SERIAL_DRIVERS_DW_APB_UART_TESTS_DEVICE_STATE_H_

#include <lib/uart/dw8250.h>
#include <lib/zx/interrupt.h>

#include <atomic>
#include <cstdint>
#include <mutex>
#include <span>
#include <vector>

#include <fake-mmio-reg/fake-mmio-reg.h>

class DeviceState {
 public:
  struct RegisterWrite {
    size_t offset;
    uint32_t value;
  };

  DeviceState() : region_(sizeof(uint32_t), uart::dw8250::kIoSlots) {
    // Note: FakeMmioRegRegion::operator[] takes the byte offset as an argument,
    // not the register index. It internally divides the offset by reg_size (4)
    // to map to the internal register array. For example, region_[0x0c] targets
    // index 3, which is the LCR. Passing index 3 directly (region_[3]) would
    // instead incorrectly target index 0 (THR/RBR).

    // Component Parameter Register (CPR) at 0xf4
    region_[0xf4].SetReadCallback([this]() {
      if (!uart_add_encoded_params_) {
        return uint64_t{0};
      }
      auto cpr = uart::dw8250::ComponentParameterRegister::Get().FromValue(0);
      cpr.set_uart_add_encoded_params(true)
          .set_fifo_stat(fifo_stat_)
          .set_thre_mode(thre_mode_)
          .set_fifo_mode(fifo_mode_)
          .set_additional_feat(additional_feat_);
      return static_cast<uint64_t>(cpr.reg_value());
    });

    // Fractional Divisor Latch (DLF) at 0xc0
    region_[0xc0].SetWriteCallback([this](uint64_t value) {
      dlf_ = value;
      write_log_.push_back(RegisterWrite{.offset = 0xc0, .value = static_cast<uint32_t>(value)});
    });
    region_[0xc0].SetReadCallback([this]() { return dlf_; });

    // Line Control Register (LCR) at 0x0c
    region_[0x0c].SetWriteCallback([this](uint64_t value) {
      if (busy_cycles_ > 0 || always_busy_) {
        busy_detect_irq_pending_ = true;
      }
      lcr_ = value;
      write_log_.push_back(RegisterWrite{.offset = 0x0c, .value = static_cast<uint32_t>(value)});
    });
    region_[0x0c].SetReadCallback([this]() { return lcr_; });

    // FIFO Control Register (FCR) (WO) and Interrupt Ident Register (IIR) (RO) at 0x08
    region_[0x08].SetWriteCallback([this](uint64_t value) {
      fcr_ = value;
      auto fcr = uart::dw8250::FifoControlRegister::Get().FromValue(static_cast<uint32_t>(value));
      fifos_enabled_ = fcr.fifo_enable();
      if (fcr.rx_fifo_reset()) {
        std::lock_guard<std::mutex> lock(rx_mutex_);
        rx_buf_.clear();
        rx_pos_ = 0;
      }
      if (fcr.tx_fifo_reset()) {
        std::lock_guard<std::mutex> lock(tx_mutex_);
        tx_buf_.clear();
      }
      write_log_.push_back(RegisterWrite{.offset = 0x08, .value = static_cast<uint32_t>(value)});
    });

    region_[0x08].SetReadCallback([this]() {
      auto iir = uart::dw8250::InterruptIdentRegister::Get().FromValue(0);
      if (fifos_enabled_) {
        iir.set_fifos_enabled(0b11);
      }
      bool rx_line_enabled = (ier_ & 0x04) != 0;
      bool rx_available_enabled = (ier_ & 0x01) != 0;
      bool tx_empty_enabled = (ier_ & 0x02) != 0;
      bool modem_status_enabled = (ier_ & 0x08) != 0;

      // Synopsys DW_apb_uart (and standard 16550) interrupt priority arbitration (Databook Table
      // 5-10):
      // - Priority 1 (0x06): Receiver Line Status (OE, PE, FE, BI errors).
      // - Priority 2 (0x0C & 0x04): RX FIFO Servicing. In hardware, Character Timeout (0x0C: data
      // in
      //   FIFO below trigger level for 4 character frames without read/write) and Received Data
      //   Available (0x04: FIFO trigger level reached) share the second priority level. They are
      //   mutually exclusive in hardware for a given FIFO state; in this mock we evaluate Character
      //   Timeout first when the test explicitly sets `char_timeout_`.
      // - Priority 3 (0x02): Transmit Holding Register Empty (THRE).
      // - Priority 4 (0x00): Modem Status (CTS, DSR, RI, CD).
      // - Priority 5 (0x07): DesignWare Busy Detect (write to LCR while UART is busy).
      bool has_rx_data = false;
      {
        std::lock_guard<std::mutex> lock(rx_mutex_);
        has_rx_data = (rx_pos_ < rx_buf_.size());
      }
      bool is_tx_empty = false;
      {
        std::lock_guard<std::mutex> lock(tx_mutex_);
        is_tx_empty = tx_buf_.empty();
      }

      if (rx_line_enabled && line_status_error_ != 0) {
        // Priority 1: Receiver Line Status (0x06)
        iir.set_interrupt_id(uart::dw8250::InterruptType::kRxLineStatus);
      } else if (rx_available_enabled && char_timeout_ && has_rx_data) {
        // Priority 2 (Timeout): Character Timeout (0x0C)
        // In hardware, this triggers when at least 1 character is in the RX FIFO (below trigger
        // level) and no characters have arrived or been read for 4 character frame intervals.
        iir.set_interrupt_id(uart::dw8250::InterruptType::kCharTimeout);
      } else if (rx_available_enabled && has_rx_data) {
        // Priority 2 (Data): Received Data Available (0x04)
        // In hardware, this triggers when the number of characters in the RX FIFO reaches the
        // programmed FCR trigger threshold (e.g., 1 char, 1/4 full, 1/2 full, 2 less than full).
        iir.set_interrupt_id(uart::dw8250::InterruptType::kRxDataAvailable);
      } else if (tx_empty_enabled && is_tx_empty && tx_empty_irq_pending_) {
        // Priority 3: Transmit Holding Register Empty (0x02)
        iir.set_interrupt_id(uart::dw8250::InterruptType::kTxEmpty);
        tx_empty_irq_pending_ = false;
      } else if (modem_status_irq_pending_ || (modem_status_enabled && (msr_ & 0x0F) != 0)) {
        // Priority 4: MODEM Status (0x00)
        iir.set_interrupt_id(uart::dw8250::InterruptType::kModemStatus);
      } else if (busy_detect_irq_pending_) {
        // Priority 5 (DesignWare): Busy Detect (0x07)
        iir.set_interrupt_id(uart::dw8250::InterruptType::kDwBusyDetect);
      } else {
        iir.set_interrupt_id(uart::dw8250::InterruptType::kNone);
      }
      return iir.reg_value();
    });

    // Interrupt Enable Register (IER) / Divisor Latch Upper (DLH) at 0x04
    region_[0x04].SetWriteCallback([this](uint64_t value) {
      if (lcr_ & 0x80) {  // DLAB=1
        dlh_ = value;
        write_log_.push_back(RegisterWrite{.offset = 0x04, .value = static_cast<uint32_t>(value)});
      } else {
        ier_ = value;
        write_log_.push_back(RegisterWrite{.offset = 0x04, .value = static_cast<uint32_t>(value)});
        bool has_rx_data = false;
        {
          std::lock_guard<std::mutex> lock(rx_mutex_);
          has_rx_data = (rx_pos_ < rx_buf_.size());
        }
        bool is_tx_empty = false;
        {
          std::lock_guard<std::mutex> lock(tx_mutex_);
          is_tx_empty = tx_buf_.empty();
        }
        bool rx_active = (ier_ & 0x01) && has_rx_data;
        bool line_active = (ier_ & 0x04) && (line_status_error_ != 0);
        bool tx_active = (ier_ & 0x02) && tx_empty_irq_pending_ && is_tx_empty;
        bool modem_active = (ier_ & 0x08) && (modem_status_irq_pending_ || ((msr_ & 0x0F) != 0));
        if (rx_active || line_active || tx_active || modem_active) {
          TriggerInterrupt();
        }
      }
    });
    region_[0x04].SetReadCallback([this]() {
      if (lcr_ & 0x80) {  // DLAB=1
        return dlh_;
      }
      return ier_;
    });

    // Modem Control Register (MCR) at 0x10
    region_[0x10].SetWriteCallback([this](uint64_t value) {
      mcr_ = value;
      write_log_.push_back(RegisterWrite{.offset = 0x10, .value = static_cast<uint32_t>(value)});
    });
    region_[0x10].SetReadCallback([this]() { return mcr_; });

    // Line Status Register (LSR) at 0x14
    region_[0x14].SetReadCallback([this]() {
      auto lsr = uart::dw8250::LineStatusRegister::Get().FromValue(0);
      {
        std::lock_guard<std::mutex> lock(rx_mutex_);
        lsr.set_data_ready(rx_pos_ < rx_buf_.size());
      }
      {
        std::lock_guard<std::mutex> lock(tx_mutex_);
        lsr.set_tx_register_empty(tx_buf_.empty());
        lsr.set_tx_empty(tx_buf_.empty());
      }
      if (line_status_error_ & (1 << 1)) {
        lsr.set_overrun_error(true);
      }
      if (line_status_error_ & (1 << 2)) {
        lsr.set_parity_error(true);
      }
      if (line_status_error_ & (1 << 3)) {
        lsr.set_framing_error(true);
      }
      if (line_status_error_ & (1 << 4)) {
        lsr.set_break_interrupt(true);
      }
      if (line_status_error_ & (1 << 7)) {
        lsr.set_error_in_rx_fifo(true);
      }
      // Reading LSR clears line status error flags
      line_status_error_ = 0;
      return lsr.reg_value();
    });

    // Modem Status Register (MSR) at 0x18
    region_[0x18].SetReadCallback([this]() {
      auto msr =
          uart::dw8250::ModemStatusRegister::Get().FromValue(static_cast<uint32_t>(msr_.load()));
      modem_status_irq_pending_ = false;
      msr_.fetch_and(~0x0Full);  // Reading MSR clears delta bits 3:0
      return msr.reg_value();
    });

    // Uart Status Register (USR) at 0x7c
    region_[0x7c].SetReadCallback([this]() {
      auto usr = uart::dw8250::UartStatusRegister::Get().FromValue(0);
      {
        std::lock_guard<std::mutex> lock(tx_mutex_);
        usr.set_transmit_fifo_not_full(tx_buf_.size() < 16);
      }
      {
        std::lock_guard<std::mutex> lock(rx_mutex_);
        usr.set_receive_fifo_not_empty(rx_pos_ < rx_buf_.size());
        usr.set_receive_fifo_full((rx_buf_.size() - rx_pos_) >= 16);
      }

      bool is_busy = false;
      if (busy_cycles_ > 0) {
        is_busy = true;
        busy_cycles_--;
      } else if (always_busy_) {
        is_busy = true;
      }
      usr.set_uart_busy(is_busy);

      // Reading USR clears Busy Detect interrupt
      busy_detect_irq_pending_ = false;
      return usr.reg_value();
    });

    // Transmit FIFO Level (TFL) at 0x80
    region_[0x80].SetReadCallback([this]() {
      auto tfl = uart::dw8250::TransmitFifoLevelRegister::Get().FromValue(0);
      std::lock_guard<std::mutex> lock(tx_mutex_);
      tfl.set_level(static_cast<uint32_t>(tx_buf_.size()));
      return tfl.reg_value();
    });

    // Receive FIFO Level (RFL) at 0x84
    region_[0x84].SetReadCallback([this]() {
      auto rfl = uart::dw8250::ReceiveFifoLevelRegister::Get().FromValue(0);
      std::lock_guard<std::mutex> lock(rx_mutex_);
      rfl.set_level(static_cast<uint32_t>(rx_buf_.size() - rx_pos_));
      return rfl.reg_value();
    });

    // Transmit Holding Register (THR) / Receiver Buffer Register (RBR) / Divisor Latch Lower (DLL)
    // at 0x00
    region_[0x00].SetWriteCallback([this](uint64_t value) {
      if (lcr_ & 0x80) {  // DLAB=1
        dll_ = value;
        write_log_.push_back(RegisterWrite{.offset = 0x00, .value = static_cast<uint32_t>(value)});
      } else {
        {
          std::lock_guard<std::mutex> lock(tx_mutex_);
          tx_buf_.push_back(static_cast<uint8_t>(value));
        }
        write_log_.push_back(RegisterWrite{.offset = 0x00, .value = static_cast<uint32_t>(value)});
      }
    });
    region_[0x00].SetReadCallback([this]() {
      if (lcr_ & 0x80) {  // DLAB=1
        return dll_;
      }
      // Note: EnableInner() and Config() intentionally perform placeholder
      // reads of RBR to flush stale bytes from hardware on startup or baud
      // rate changes when the RX FIFO is empty. Returning 0 emulates reading an
      // empty hardware FIFO without failing tests.
      uint8_t value = 0;
      {
        std::lock_guard<std::mutex> lock(rx_mutex_);
        if (rx_pos_ >= rx_buf_.size()) {
          return static_cast<uint64_t>(0);
        }
        value = rx_buf_[rx_pos_++];
      }
      return static_cast<uint64_t>(value);
    });
  }

  void set_irq_signaller(zx::interrupt signaller) { irq_signaller_ = std::move(signaller); }

  fdf::MmioBuffer GetMmio() { return region_.GetMmioBuffer(); }

  void TriggerInterrupt() {
    if (irq_signaller_.is_valid()) {
      irq_signaller_.trigger(0, zx::time_boot());
    }
  }

  void Inject(std::span<const uint8_t> buffer, uint8_t line_status_error = 0) {
    {
      std::lock_guard<std::mutex> lock(rx_mutex_);
      rx_buf_.assign(buffer.begin(), buffer.end());
      rx_pos_ = 0;
    }
    line_status_error_ = line_status_error;
    bool is_tx_empty = false;
    {
      std::lock_guard<std::mutex> lock(tx_mutex_);
      is_tx_empty = tx_buf_.empty();
    }
    if (buffer.empty() && is_tx_empty) {
      tx_empty_irq_pending_ = true;
    }
    TriggerInterrupt();
  }

  void Inject() { Inject(std::span<const uint8_t>{}, 0); }

  // Atomically asserts multiple concurrent interrupt conditions (line status error,
  // RX data available, modem status delta, and busy detect) and fires a single hardware
  // interrupt trigger. This avoids race conditions where separate method calls trigger
  // intermediate interrupts that can be dropped while the driver is actively servicing
  // the first condition.
  void AssertAllInterruptConditions(std::span<const uint8_t> buffer, uint8_t line_status_error,
                                    uint8_t msr_val, bool busy_detect) {
    {
      std::lock_guard<std::mutex> lock(rx_mutex_);
      rx_buf_.assign(buffer.begin(), buffer.end());
      rx_pos_ = 0;
    }
    line_status_error_ = line_status_error;
    msr_ = msr_val;
    modem_status_irq_pending_ = (msr_val != 0);
    busy_detect_irq_pending_ = busy_detect;
    TriggerInterrupt();
  }

  std::vector<uint8_t> TxBuf() {
    std::vector<uint8_t> buf;
    {
      std::lock_guard<std::mutex> lock(tx_mutex_);
      buf.swap(tx_buf_);
    }
    tx_empty_irq_pending_ = true;
    return buf;
  }

  size_t TxBufSize() const {
    std::lock_guard<std::mutex> lock(tx_mutex_);
    return tx_buf_.size();
  }

  uint32_t lcr() const { return static_cast<uint32_t>(lcr_); }
  uint32_t ier() const { return static_cast<uint32_t>(ier_); }
  uint32_t mcr() const { return static_cast<uint32_t>(mcr_); }
  uint32_t fcr() const { return static_cast<uint32_t>(fcr_); }
  uint32_t msr() const { return static_cast<uint32_t>(msr_.load()); }
  uint32_t dll() const { return static_cast<uint32_t>(dll_); }
  uint32_t dlh() const { return static_cast<uint32_t>(dlh_); }
  uint32_t dlf() const { return static_cast<uint32_t>(dlf_); }

  bool fifos_enabled() const { return fifos_enabled_; }
  uint8_t line_status_error() const { return line_status_error_.load(); }
  bool busy_detect_irq_pending() const { return busy_detect_irq_pending_.load(); }
  bool modem_status_irq_pending() const { return modem_status_irq_pending_.load(); }
  bool char_timeout() const { return char_timeout_.load(); }

  const std::vector<RegisterWrite>& write_log() const { return write_log_; }
  void ClearWriteLog() { write_log_.clear(); }

  void set_additional_feat(bool val) { additional_feat_ = val; }
  void set_uart_add_encoded_params(bool val) { uart_add_encoded_params_ = val; }
  void set_thre_mode(bool val) { thre_mode_ = val; }
  void set_fifo_stat(bool val) { fifo_stat_ = val; }
  void set_fifo_mode(uint32_t val) { fifo_mode_ = val; }

  void SetLineStatusError(uint8_t error_mask) {
    line_status_error_ = error_mask;
    TriggerInterrupt();
  }

  void SetCharTimeout(bool val) {
    char_timeout_ = val;
    bool has_rx_data = false;
    {
      std::lock_guard<std::mutex> lock(rx_mutex_);
      has_rx_data = (rx_pos_ < rx_buf_.size());
    }
    if (val && has_rx_data) {
      TriggerInterrupt();
    }
  }

  void SetBusyDetect(bool val) {
    busy_detect_irq_pending_ = val;
    if (val) {
      TriggerInterrupt();
    }
  }

  void SetModemStatus(uint8_t msr_val) {
    msr_ = msr_val;
    modem_status_irq_pending_ = true;
    TriggerInterrupt();
  }

  void SetBusyCycles(size_t cycles) { busy_cycles_ = cycles; }
  void SetAlwaysBusy(bool val) { always_busy_ = val; }

 private:
  mutable std::mutex tx_mutex_;
  mutable std::mutex rx_mutex_;
  std::vector<uint8_t> tx_buf_;
  std::vector<uint8_t> rx_buf_;
  size_t rx_pos_ = 0;
  uint64_t lcr_ = 0;
  uint64_t ier_ = 0;
  uint64_t mcr_ = 0;
  uint64_t fcr_ = 0;
  std::atomic<uint64_t> msr_{0};
  uint64_t dll_ = 0;
  uint64_t dlh_ = 0;
  uint64_t dlf_ = 0;

  bool fifos_enabled_ = true;
  bool uart_add_encoded_params_ = true;
  bool thre_mode_ = true;
  bool fifo_stat_ = true;
  uint32_t fifo_mode_ = 1;
  bool additional_feat_ = false;

  std::atomic<uint8_t> line_status_error_{0};
  std::atomic<bool> char_timeout_{false};
  std::atomic<bool> tx_empty_irq_pending_{true};
  std::atomic<bool> modem_status_irq_pending_{false};
  std::atomic<bool> busy_detect_irq_pending_{false};

  size_t busy_cycles_ = 0;
  bool always_busy_ = false;

  std::vector<RegisterWrite> write_log_;
  ddk_fake::FakeMmioRegRegion region_;
  zx::interrupt irq_signaller_;
};

#endif  // SRC_DEVICES_SERIAL_DRIVERS_DW_APB_UART_TESTS_DEVICE_STATE_H_
