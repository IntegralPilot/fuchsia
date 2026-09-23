// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PLATFORM_INCLUDE_PLATFORM_UART_H_
#define ZIRCON_KERNEL_PLATFORM_INCLUDE_PLATFORM_UART_H_

#include <lib/uart/null.h>
#include <lib/uart/uart.h>
#include <lib/zbi-format/driver-config.h>

#include <ktl/optional.h>
#include <phys/handoff.h>

// Returns the |irq| number to be used for registering an IRQ Handler if such |irq_num| can be
// translated.
// Returns |nullopt| if there is no irq wired to the provided |irq_num|, if the provided
// |irq_num| has a platform specific meaning.
ktl::optional<uint32_t> PlatformUartGetIrqNumber(uint32_t irq_num);

// ulib/uart IoProvider implementation for the kernel.
template <typename Config, uart::IoRegisterType>
class PlatformUartIoProvider;

// No-op for stub drivers.
template <typename Config>
class PlatformUartIoProvider<Config, uart::IoRegisterType::kNone>
    : public uart::BasicIoProvider<Config, uart::IoRegisterType::kNone> {
 public:
  using Base = uart::BasicIoProvider<Config, uart::IoRegisterType::kNone>;

  constexpr PlatformUartIoProvider(const Config& config, size_t io_slots)
      : Base(config, io_slots) {}
};

// MMIO Driver specialization.
template <uart::IoRegisterType IoType>
class PlatformUartIoProvider<zbi_dcfg_simple_t, IoType>
    : public uart::BasicIoProvider<zbi_dcfg_simple_t, IoType> {
 public:
  using Base = uart::BasicIoProvider<zbi_dcfg_simple_t, IoType>;

  using Base::Base;
  PlatformUartIoProvider(const zbi_dcfg_simple_t& config, size_t io_slots)
      : Base(config, io_slots, gPhysHandoff->uart_mmio.data()) {
    ZX_DEBUG_ASSERT(config.mmio_phys == gPhysHandoff->uart_mmio.paddr());
  }
};

#if defined(__x86_64__) || defined(__i386__)
// PIO Driver implementation.
template <>
class PlatformUartIoProvider<zbi_dcfg_simple_pio_t, uart::IoRegisterType::kPio>
    : public uart::BasicIoProvider<zbi_dcfg_simple_pio_t, uart::IoRegisterType::kPio> {
 public:
  using Base = uart::BasicIoProvider<zbi_dcfg_simple_pio_t, uart::IoRegisterType::kPio>;
  using Base::Base;
  PlatformUartIoProvider(const zbi_dcfg_simple_pio_t& config, uint16_t io_slots)
      : Base(config, io_slots) {}
};
#endif

#endif  // ZIRCON_KERNEL_PLATFORM_INCLUDE_PLATFORM_UART_H_
