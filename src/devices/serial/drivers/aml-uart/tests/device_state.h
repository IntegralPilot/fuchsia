// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.serialimpl/cpp/driver/fidl.h>
#include <lib/zx/interrupt.h>

#include <fake-mmio-reg/fake-mmio-reg.h>

#include "src/devices/serial/drivers/aml-uart/registers.h"

constexpr size_t kDataLen = 32;

class DeviceState {
 public:
  DeviceState() : region_(sizeof(uint32_t), 6) {
    // Control register
    region_[2 * sizeof(uint32_t)].SetWriteCallback([=](uint64_t value) {
      control_reg_ = value;
      auto ctrl = Control();
      if (ctrl.rst_rx()) {
        reset_rx_ = true;
      }
      if (ctrl.rst_tx()) {
        reset_tx_ = true;
      }
    });
    region_[2 * sizeof(uint32_t)].SetReadCallback([=]() { return control_reg_; });
    // Status register
    region_[3 * sizeof(uint32_t)].SetWriteCallback([=](uint64_t value) { status_reg_ = value; });
    region_[3 * sizeof(uint32_t)].SetReadCallback([=]() {
      auto status = Status();
      status.set_rx_empty(!rx_len_);
      return status.reg_value();
    });
    // TFIFO
    region_[0 * sizeof(uint32_t)].SetWriteCallback(
        [=](uint64_t value) { tx_buf_.push_back(static_cast<uint8_t>(value)); });
    // RFIFO
    region_[1 * sizeof(uint32_t)].SetReadCallback([=]() {
      uint8_t value = *rx_buf_;
      rx_buf_++;
      rx_len_--;
      return value;
    });
    // Reg5
    region_[5 * sizeof(uint32_t)].SetWriteCallback([=](uint64_t value) { reg5_ = value; });
    region_[5 * sizeof(uint32_t)].SetReadCallback([=]() { return reg5_; });
  }

  void set_irq_signaller(zx::unowned_interrupt signaller) { irq_signaller_ = std::move(signaller); }

  fdf::MmioBuffer GetMmio() { return region_.GetMmioBuffer(); }

  bool PortResetRX() {
    bool reset = reset_rx_;
    reset_rx_ = false;
    return reset;
  }

  bool PortResetTX() {
    bool reset = reset_tx_;
    reset_tx_ = false;
    return reset;
  }

  void Inject(const void* buffer, size_t size) {
    rx_buf_ = static_cast<const unsigned char*>(buffer);
    rx_len_ = size;
    irq_signaller_->trigger(0, zx::time_boot());
  }

  serial::Status Status() {
    return serial::Status::Get().FromValue(static_cast<uint32_t>(status_reg_));
  }

  serial::Control Control() {
    return serial::Control::Get().FromValue(static_cast<uint32_t>(control_reg_));
  }

  serial::Reg5 Reg5() { return serial::Reg5::Get().FromValue(static_cast<uint32_t>(reg5_)); }

  uint32_t StopBits() {
    switch (Control().stop_len()) {
      case 0:
        return fuchsia_hardware_serialimpl::kSerialStopBits1;
      case 1:
        return fuchsia_hardware_serialimpl::kSerialStopBits2;
      default:
        return 255;
    }
  }

  uint32_t DataBits() {
    switch (Control().xmit_len()) {
      case 0:
        return fuchsia_hardware_serialimpl::kSerialDataBits8;
      case 1:
        return fuchsia_hardware_serialimpl::kSerialDataBits7;
      case 2:
        return fuchsia_hardware_serialimpl::kSerialDataBits6;
      case 3:
        return fuchsia_hardware_serialimpl::kSerialDataBits5;
      default:
        return 255;
    }
  }

  uint32_t Parity() {
    switch (Control().parity()) {
      case 0:
        return fuchsia_hardware_serialimpl::kSerialParityNone;
      case 2:
        return fuchsia_hardware_serialimpl::kSerialParityEven;
      case 3:
        return fuchsia_hardware_serialimpl::kSerialParityOdd;
      default:
        return 255;
    }
  }
  bool FlowControl() { return !Control().two_wire(); }

  std::vector<uint8_t> TxBuf() {
    std::vector<uint8_t> buf;
    buf.swap(tx_buf_);
    return buf;
  }

 private:
  std::vector<uint8_t> tx_buf_;
  const uint8_t* rx_buf_ = nullptr;
  size_t rx_len_ = 0;
  bool reset_tx_ = false;
  bool reset_rx_ = false;
  uint64_t reg5_ = 0;
  uint64_t control_reg_ = 0;
  uint64_t status_reg_ = 0;
  ddk_fake::FakeMmioRegRegion region_;
  zx::unowned_interrupt irq_signaller_;
};
