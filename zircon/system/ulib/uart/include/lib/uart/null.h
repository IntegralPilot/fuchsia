// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_UART_NULL_H_
#define LIB_UART_NULL_H_

// uart::null::Driver is a bit bucket.
// It also serves to demonstrate the API required by uart::KernelDriver.

#include <lib/zbi-format/zbi.h>
#include <zircon/assert.h>

#include <array>
#include <string_view>

#include "uart.h"

namespace devicetree {
class PropertyDecoder;
}

namespace uart::null {

struct Driver {
  using config_type = StubConfig;

  static constexpr std::array<std::string_view, 0> kDevicetreeBindings = {};
  static constexpr std::string_view kConfigName = "none";
  static constexpr IoRegisterType kIoType = IoRegisterType::kNone;
  static constexpr uint32_t kType = 0;
  static constexpr uint32_t kExtra = 0;

  static std::optional<uart::Config<Driver>> TryMatch(const zbi_header_t& header, const void*) {
    return std::nullopt;
  }

  static std::optional<uart::Config<Driver>> TryMatch(
      const acpi_lite::AcpiDebugPortDescriptor& debug_port) {
    return std::nullopt;
  }

  static std::optional<uart::Config<Driver>> TryMatch(std::string_view str) {
    if (str == kConfigName) {
      return Config<Driver>{};
    }
    return std::nullopt;
  }

  static bool TrySelect(const devicetree::PropertyDecoder& decoder) { return false; }

  Driver() = default;
  explicit Driver(const config_type& config) {}

  template <std::derived_from<Driver> T>
  explicit Driver(const Config<T>& tagged_config) {}

  constexpr bool operator==(const Driver& other) const { return true; }
  constexpr bool operator!=(const Driver& other) const { return false; }

  void FillItem(void*) const { ZX_PANIC("should never be called"); }

  // API to match devicetree node compatible list.
  static bool MatchDevicetree(const devicetree::PropertyDecoder&) { return false; }

  void Unparse(FILE* out) const {
    fprintf(out, "%.*s", static_cast<int>(kConfigName.size()), kConfigName.data());
  }

  // uart::KernelDriver UartDriver API
  //
  // Each method is a template parameterized by an IoProvider type for
  // accessing the hardware registers so that real Driver types can be used
  // with hwreg::Mock in tests independent of actual hardware access.  The
  // null Driver never uses the `io` arguments.

  template <typename IoProvider>
  void Init(IoProvider& io) {}

  template <class IoProvider>
  void SetLineControl(IoProvider& io, std::optional<DataBits> data_bits,
                      std::optional<Parity> parity, std::optional<StopBits> stop_bits) {}

  // Return true if Write can make forward progress right now.
  template <typename IoProvider>
  bool TxReady(IoProvider& io) {
    return true;
  }

  // This is called only when TxReady() has just returned true.  Advance
  // the iterator at least one and as many as is convenient but not past
  // end, outputting each character before advancing.
  template <typename IoProvider, typename It1, typename It2>
  auto Write(IoProvider& io, bool, It1 it, const It2& end) {
    return end;
  }

  // Poll for an incoming character and return one if there is one.
  template <typename IoProvider>
  std::optional<uint8_t> Read(IoProvider& io) {
    return {};
  }

  // Enable transmit interrupts so Interrupt will be called when TxReady().
  template <typename IoProvider>
  void EnableTxInterrupt(IoProvider& io) {}

  // Enable receive interrupts so Interrupt will be called when RxReady().
  template <typename IoProvider>
  void EnableRxInterrupt(IoProvider& io) {}

  // Set the UART up to deliver interrupts.  This is called after Init.
  template <typename IoProvider, typename IrqProvider>
  void InitInterrupt(IoProvider& io, IrqProvider& irq) {}

  template <typename IoProvider, typename Lock, typename Waiter, typename Tx, typename Rx>
  void Interrupt(IoProvider&, Lock&, Waiter&, Tx&&, Rx&&) {}

  template <typename IoProvider, typename IrqProvider>
  void PrepareForSuspend(IoProvider& io, IrqProvider& irq) {}

  template <typename IoProvider, typename IrqProvider>
  void WakeupFromSuspend(IoProvider& io, IrqProvider& irq) {}

  // This tells the IoProvider what device resources to provide.
  constexpr config_type config() const { return {}; }
  constexpr size_t io_slots() const { return 0; }
};

}  // namespace uart::null

#endif  // LIB_UART_NULL_H_
