// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_UART_H_
#define ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_UART_H_

#include <lib/uart/all.h>

#include <type_traits>

#include "item-base.h"

namespace boot_shim {

// This can supply a ZBI_TYPE_KERNEL_DRIVER item based on the UART driver configuration.
template <typename AllConfigs = uart::all::Config<>>
class UartItem : public boot_shim::ItemBase {
 public:
  void Init(const AllConfigs& config) { config_ = config; }

  constexpr size_t size_bytes() const { return ItemSize(zbi_dcfg_size()); }

  fit::result<DataZbi::Error> AppendItems(DataZbi& zbi) const {
    fit::result<UartItem::DataZbi::Error> result = fit::ok();
    config_.Visit([&]<typename UartDriver>(const uart::Config<UartDriver>& config) {
      using config_type = typename UartDriver::config_type;
      if constexpr (!std::is_same_v<uart::StubConfig, config_type>) {
        if (auto append_result = zbi.Append(
                {
                    .type = UartDriver::kType,
                    .length = static_cast<uint32_t>(sizeof(config_type)),
                    .extra = UartDriver::kExtra,
                },
                config.as_bytes());
            append_result.is_error()) {
          result = append_result.take_error();
        }
      }
    });
    return result;
  }

 private:
  constexpr size_t zbi_dcfg_size() const {
    size_t size = 0;
    config_.Visit([&]<typename UartDriver>(const uart::Config<UartDriver>& config) {
      size = sizeof(typename UartDriver::config_type);
    });
    return size;
  }

  AllConfigs config_;
};

}  // namespace boot_shim

#endif  // ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_UART_H_
