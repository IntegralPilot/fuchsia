// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_ULIB_UART_INCLUDE_LIB_UART_ALL_H_
#define ZIRCON_SYSTEM_ULIB_UART_INCLUDE_LIB_UART_ALL_H_

#include <lib/fit/function.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zbi-format/zbi.h>
#include <stdio.h>

#include <bit>
#include <type_traits>
#include <utility>
#include <variant>

#include <hwreg/internal.h>

#include "amlogic.h"
#include "apple-s5l.h"
#include "dw8250.h"
#include "exynos-usi.h"
#include "geni.h"
#include "ns8250.h"
#include "null.h"
#include "pl011.h"
#include "uart.h"

namespace devicetree {
class PropertyDecoder;
}

namespace uart {

namespace internal {

struct DummyDriver : public null::Driver {
  using config_type = StubConfig;

  using null::Driver::Driver;
  using null::Driver::TrySelect;

  template <typename... Args>
  static std::optional<uart::Config<DummyDriver>> TryMatch(Args&&... args) {
    return std::nullopt;
  }

  void Unparse(FILE*) const { ZX_PANIC("DummyDriver should never be called!"); }
};

// std::visit is not pure-PIC-friendly.  hwreg implements a limited version
// that is pure-PIC-friendly and that works for the cases here.  Reach in and
// steal that instead of copying the implementation here, since there isn't
// any particularly good "public" place to move it into instead.
using hwreg::internal::Visit;

template <typename Driver, typename... Args>
concept MatchableDriver = requires(Args&&... args) {
  {
    Driver::TryMatch(std::forward<Args>(args)...)
  } -> std::convertible_to<std::optional<uart::Config<Driver>>>;
};

template <typename Driver, typename... Args>
concept SelectableDriver = requires(Args&&... args) {
  { Driver::TrySelect(std::forward<Args>(args)...) } -> std::convertible_to<bool>;
};

}  // namespace internal

namespace all {

// uart::all::WithAllDrivers<Template, Args...> instantiates the template class
// Template<Args..., foo::Driver, bar::Driver, ...> for all the drivers
// supported by this kernel build (foo, bar, ...).  Using a template that takes
// a template template parameter is the only real way (short of macros) to have
// a single list of the supported uart::xyz::Driver implementations.
template <template <class... Drivers> class Template, typename... Args>
using WithAllDrivers = Template<
    // Any additional template arguments precede the arguments for each driver.
    Args...,
    // A default-constructed variant gets the null driver.
    null::Driver,
    // These drivers are potentially used on all machines.
    ns8250::Mmio32Driver, ns8250::Mmio8Driver, ns8250::PxaDriver, dw8250::Driver,
#if defined(__aarch64__) || UART_ALL_DRIVERS
    amlogic::Driver, geni::Driver, pl011::Driver,
#endif
#if defined(EXPERIMENTAL_APPLE) || UART_ALL_DRIVERS
    apple_s5l::Driver,
#endif
#if defined(__aarch64__) || defined(__riscv) || UART_ALL_DRIVERS
    exynos_usi::Driver,
#endif
#if defined(__x86_64__) || defined(__i386__) || UART_ALL_DRIVERS
    ns8250::PioDriver,
#endif
    // This is never used but permits a trailing comma above.
    internal::DummyDriver>;

// The hardware support object underlying whichever KernelDriver type is the
// active variant can be extracted into this type and then used to construct a
// new uart::all::KernelDriver instantiation in a different environment.
//
// The underlying UartDriver types and ktl::variant (aka std::variant) hold
// only non-pointer data that can be transferred directly from one environment
// to another, e.g. to hand off from physboot to the kernel.
using Driver = WithAllDrivers<std::variant>;

// This represents the a configuration tagged with a driver from the set of available drivers.
// Helper methods allows us to generate a `uart::all::Driver`.
//
// The provided configuration object has the following properties:
//    * `uart_type` alias for the uart driver type.
//    * `config_type` alias for `uart_type::config_type`.
//    * `operator*()` and `operator()->` return reference and pointer to the underlying
//    `config_type` object.
//
// Note: There is no driver `state` being held in this object, just the configuration.
template <typename UartDriver = Driver>
class Config {
 public:
  // Returns a `Config` object if any supported driver provides a `TryMatch` static method that can
  // be invoked with the provided arguments `args`. Otherwise `std::nullopt` is returned.
  //
  // Note: The matching order is determined by the position in the list of drivers.
  template <typename... Args>
  static std::optional<Config> Match(Args&&... args) {
    std::optional<Config> all_configs;

    // Collapse `std::optional<T>` to `std::optional<Config<...., T, ...>`.
    auto to_all_config =
        []<typename UartType>(
            const std::optional<uart::Config<UartType>>& config) -> std::optional<Config> {
      printf("attempted match to %s", UartType::kConfigName.data());
      if (config) {
        printf(" OK\n");
        return {*config};
      }
      printf(" FAIL\n");
      return std::nullopt;
    };

    // Turns variant size into a parameter pack and invokes extracts the variant arguments of
    // `UartDriver` and attempts to match each.
    auto try_match_supported_driver = [&]<size_t... Is>(std::index_sequence<Is...> is) {
      static_assert(
          (uart::internal::MatchableDriver<std::variant_alternative_t<Is, UartDriver>, Args...> &&
           ...));
      // Expand the parameter pack and abuse `std::optional` boolean evaluation, leading to chaining
      // matches until the first time a Driver in the list returns a non-nullopt
      // `std::optional<uart::Config<Driver>>` object.
      ((all_configs =
            to_all_config(std::variant_alternative_t<Is, UartDriver>::TryMatch(args...))) ||
       ...);
    };

    try_match_supported_driver(std::make_index_sequence<std::variant_size_v<UartDriver>>());

    return all_configs;
  }

  // Returns an empty `Config` object if any supported driver provides a `TrySelect` static method
  // that succeeds when invoked with `args`. Otherwise `std::nullopt` is returned. This allows
  // separating driver type selection from the actual configuration, or partially filling the
  // configuration.
  //
  // Note: The matching order is determined by the position in the list of the driver.
  template <typename... Args>
  static std::optional<Config> Select(Args&&... args) {
    std::optional<Config> all_configs;
    auto try_one = [&]<typename SupportedDriver>(std::optional<Config>& config) -> bool {
      if (SupportedDriver::TrySelect(std::forward<Args>(args)...)) {
        config = uart::Config<SupportedDriver>{};
      }
      return config.has_value();
    };

    // Turns variant size into a parameter pack and invokes extracts the variant arguments of
    // `UartDriver` and attempts to match each.
    auto try_select_supported_driver = [&]<size_t... Is>(std::index_sequence<Is...> is) {
      static_assert(
          (uart::internal::SelectableDriver<std::variant_alternative_t<Is, UartDriver>, Args...> &&
           ...));
      (try_one.template operator()<std::variant_alternative_t<Is, UartDriver>>(all_configs) || ...);
    };
    try_select_supported_driver(std::make_index_sequence<std::variant_size_v<UartDriver>>());

    return all_configs;
  }

  Config() = default;
  Config(const Config&) = default;
  Config(Config&&) = default;

  // Constructor to obtain configurations from a Uart.
  template <typename Uart>
  explicit(false) Config(const uart::Config<Uart>& config) : configs_(config) {}
  template <typename Uart>
  explicit Config(const Uart& uart) : configs_(uart::Config<Uart>(uart.config())) {}
  template <typename Uart, template <typename, IoRegisterType> class IoProvider, typename Sync,
            class IrqProvider>
  explicit Config(const uart::KernelDriver<Uart, IoProvider, Sync, IrqProvider>& uart)
      : configs_(uart::Config<Uart>(uart.config())) {}

  Config& operator=(const Config&) = default;
  Config& operator=(Config&&) = default;

  template <typename Uart, template <typename, IoRegisterType> class IoProvider, typename Sync,
            class IrqProvider>
  Config& operator=(const uart::KernelDriver<Uart, IoProvider, Sync, IrqProvider>& driver) {
    configs_ = uart::Config<Uart>(driver.config());
    return *this;
  }

  template <typename T>
  Config& operator=(const T& uart) {
    configs_ = uart::Config<T>(uart.config());
    return *this;
  }

  template <typename T>
  Config& operator=(const uart::Config<T>& uart_config) {
    configs_ = uart_config;
    return *this;
  }

  // Visitor to access the active configuration object.
  template <typename T>
  void Visit(T&& visitor) {
    internal::Visit(std::forward<T>(visitor), configs_);
  }

  template <typename T>
  void Visit(T&& visitor) const {
    internal::Visit(std::forward<T>(visitor), configs_);
  }

 private:
  template <typename T>
  struct Variant;

  template <typename... Args>
  struct Variant<std::variant<Args...>> {
    using type = std::variant<uart::Config<Args>...>;
  };

  typename Variant<UartDriver>::type configs_;
};

// Instantiates `uart::all::Driver` with `config`.
template <typename UartDriver = uart::all::Driver>
UartDriver MakeDriver(const uart::all::Config<UartDriver>& config) {
  UartDriver driver;
  config.Visit([&driver]<typename T>(const T& uart_config) {
    driver.template emplace<typename T::uart_type>(*uart_config);
  });
  return driver;
}

template <typename UartDriver = uart::all::Driver>
uart::all::Config<UartDriver> GetConfig(const uart::all::Driver& driver) {
  uart::all::Config<UartDriver> cfg;
  uart::internal::Visit([&cfg](const auto& driver) { cfg = driver; }, driver);
  return cfg;
}

// uart::all::KernelDriver is a variant across all the KernelDriver types.
template <template <typename, IoRegisterType> class IoProvider, typename Sync, class IrqProvider,
          typename UartDriver = Driver>
class KernelDriver {
 public:
  using uart_type = UartDriver;

  // In default-constructed state, it's the null driver.
  KernelDriver() = default;

  // It can be copy-constructed from one of the supported uart::xyz::Driver
  // types to hand off the hardware state from a different instantiation.
  template <typename T>
  explicit KernelDriver(const T& uart) : variant_(uart) {}

  // ...or from another all::KernelDriver::uart() result.
  explicit KernelDriver(const uart_type& uart) { *this = uart; }

  explicit KernelDriver(const Config<UartDriver>& config) {
    config.Visit([this]<typename ConfigType>(const ConfigType& driver_config) {
      variant_.template emplace<OneDriver<typename ConfigType::uart_type>>(*driver_config);
    });
  }

  // Assignment is another way to reinitialize the configuration.
  KernelDriver& operator=(const uart_type& uart) {
    internal::Visit(
        [this](auto&& uart) {
          using ThisUart = std::decay_t<decltype(uart)>;
          variant_.template emplace<OneDriver<ThisUart>>(uart);
        },
        uart);
    return *this;
  }

  Config<UartDriver> config() const {
    Config<UartDriver> conf;
    Visit([&conf](const auto& driver) { conf = driver; });
    return conf;
  }

  std::optional<MmioRange> maybe_mmio_range() const {
    std::optional<MmioRange> range;
    Visit([&range](const auto& driver) { range = MaybeMmioRange(driver); });
    return range;
  }

  // Write out a string that Parse() can read back to recreate the driver
  // state.  This doesn't preserve the driver state, only the configuration.
  void Unparse(FILE* out = stdout) const {
    Visit([out](const auto& active) { active.Unparse(out); });
  }

  // Apply f to selected driver.
  template <typename T, typename... Args>
  void Visit(T&& f, Args... args) {
    internal::Visit(VariantVisitor<T>{std::forward<T>(f)}, variant_, std::forward<Args>(args)...);
  }

  // Apply f to selected driver.
  template <typename T, typename... Args>
  void Visit(T&& f, Args... args) const {
    internal::Visit(VariantVisitor<T>{std::forward<T>(f)}, variant_, std::forward<Args>(args)...);
  }

  // Takes ownership of the underlying hardware management and state. This object will be left
  // in an invalid state, and should be reinitialized before interacting with it.
  uart_type TakeUart() && {
    uart_type driver;
    Visit([&driver](auto&& active) { driver = std::move(active).TakeUart(); });
    // moved-from state.
    variant_.template emplace<std::monostate>();
    return driver;
  }

  // Returns true if the active `uart::KernelDriver<...>` is backed by `TargetUartDriver`.
  template <typename TargetUartDriver>
  constexpr bool holds_alternative() const {
    return std::holds_alternative<OneDriver<TargetUartDriver>>(variant_);
  }

  // Returns true if a valid non-null driver variant is active.
  explicit constexpr operator bool() const {
    return !(holds_alternative<uart::null::Driver>() ||
             // For completeness.
             holds_alternative<internal::DummyDriver>() ||
             std::holds_alternative<std::monostate>(variant_));
  }

 private:
  template <class Uart>
  using OneDriver = uart::KernelDriver<Uart, IoProvider, Sync, IrqProvider>;
  template <class... Uart>
  using Variant = std::variant<OneDriver<Uart>..., std::monostate>;

  template <typename T>
  struct OneDriverVariant;

  template <typename... Args>
  struct OneDriverVariant<std::variant<Args...>> {
    using type = Variant<Args...>;
  };

  static constexpr auto MaybeMmioRange(const NonMmioDriver auto& driver) { return std::nullopt; }

  static constexpr auto MaybeMmioRange(const MmioDriver auto& driver) {
    return driver.mmio_range();
  }

  template <typename... Args>
  bool TryMatch(Args&&... args) {
    std::optional<uart::all::Config<UartDriver>> config =
        uart::all::Config<UartDriver>::Match(std::forward<Args>(args)...);
    if (config) {
      config->Visit([this]<typename UartType>(uart::Config<UartType>& config) {
        variant_.template emplace<OneDriver<UartType>>(config);
      });
    }
    return config.has_value();
  }

  // The VariantVisitor can be called exactly once, so its call operator has
  // only the rvalue overload.  When called on the std::monostate, it always
  // crashes rather than invoking the delegate object.
  template <typename Delegate>
  struct VariantVisitor {
    template <typename T, typename... Args>
    void operator()(T&& alternative, Args&&... args) && {
      if constexpr (std::is_same_v<std::decay_t<T>, std::monostate>) {
        // We cannot use panic macros in this spot, since that would end up
        // calling printf, which may end up visiting the same uart, whose
        // invalid state (std::monostate) triggered this.  Even this will cause
        // a cascade of machine exceptions.
        __builtin_abort();
      } else {
        std::invoke(std::move(delegate), std::forward<T>(alternative), std::forward<Args>(args)...);
      }
    }

    Delegate delegate;
  };

  typename OneDriverVariant<UartDriver>::type variant_;
};

}  // namespace all
}  // namespace uart

#endif  // ZIRCON_SYSTEM_ULIB_UART_INCLUDE_LIB_UART_ALL_H_
