// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree-boot-shim.h>
#include <lib/devicetree/devicetree.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>

#include <array>
#include <optional>
#include <string_view>

#ifndef ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_WATCHDOG_H_
#define ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_WATCHDOG_H_

namespace boot_shim {

template <typename T>
concept WatchdogMmioHelper = requires(T t, uint64_t base_addr, uint32_t contents) {
  { T::Read(base_addr) } -> std::convertible_to<uint64_t>;
  { T::Write(contents, base_addr) };
};

// A `Watchdog` Item must provide the following API:
template <typename T, typename MmioHelper>
concept Watchdog = WatchdogMmioHelper<MmioHelper> &&
                   requires(std::decay_t<T> t, const devicetree::PropertyDecoder& decoder,
                            const DevicetreeBootShimMmioObserver* mmio_observer) {
                     // `Watchdog::kCompatibleDevices` as a collection of `std::string_view`.
                     // This represents the list of device nodes compatible string the items is
                     // matching against.
                     { decltype(t)::kCompatibleDevices } -> std::ranges::range;
                     {
                       *decltype(t)::kCompatibleDevices.begin()
                     } -> std::convertible_to<std::string_view>;

                     // This function is provided with a decoder (references a node) whose
                     // compatible string contain a device name in the `kCompatibleDevices`
                     // collection. If the node properties are enough to generate a valid
                     // configuration then a `zbi_dcfg_generic_32_watchdog_t` instance is returned.
                     // Otherwise, returns `std::nullopt`.
                     {
                       decltype(t)::template MaybeCreate<MmioHelper>(decoder, mmio_observer)
                     } -> std::convertible_to<std::optional<zbi_dcfg_generic32_watchdog_t>>;
                   };

struct QualcomMsmWatchdog {
  // List of devicetree compatible strings that this `Watchdog` item should be matched to.
  static constexpr auto kCompatibleDevices = std::to_array<std::string_view>({"qcom,msm-watchdog"});

  static constexpr uint32_t kResetOffset = 0x4;
  static constexpr uint32_t kEnableOffset = 0x8;
  static constexpr uint32_t kBarkTimeOffset = 0x10;
  static constexpr uint32_t kBiteTimeOffset = 0x14;

  // Information extracted from a devicetree node.
  struct DecodedNode {
    // Base address of the watchdog banked registers, where to apply the offsets.
    uint64_t addr;
    // Size of the register bank.
    uint64_t size;
    // How often it should be pet in MS.
    uint32_t pet_time_ms;
    // If not pet, will issue system reset after this time in MS.
    uint32_t bark_time_ms;
  };

  // Returns true if decoder references a valid device node describing the watchdog device node and
  // `payload` is properly filled. Otherwise, returns false.
  template <typename MmioHelper>
  static std::optional<zbi_dcfg_generic32_watchdog_t> MaybeCreate(
      const devicetree::PropertyDecoder& decoder,
      const DevicetreeBootShimMmioObserver* mmio_observer) {
    std::optional<DecodedNode> decoded_node = DecodeNode(decoder);
    if (!decoded_node) {
      return std::nullopt;
    }

    (*mmio_observer)(MmioRange{
        .address = decoded_node->addr,
        .size = static_cast<size_t>(decoded_node->size),
    });

    return zbi_dcfg_generic32_watchdog_t{
        .pet_action =
            {
                .addr = decoded_node->addr + kResetOffset,
                .clr_mask = 0xffffffff,
                .set_mask = 0x1,
            },
        .enable_action =
            {
                .addr = decoded_node->addr + kEnableOffset,
                .clr_mask = 0xffffffff,
                .set_mask = 0x1,
            },
        .disable_action =
            {
                .addr = decoded_node->addr + kEnableOffset,
                .clr_mask = 0xffffffff,
                .set_mask = 0x0,
            },
        .watchdog_period_nsec = zx::msec(decoded_node->pet_time_ms).to_nsecs(),
        .flags = 0,
    };
  }

  static std::optional<DecodedNode> DecodeNode(const devicetree::PropertyDecoder& decoder) {
    auto [reg_names_prop, reg_prop, pet_time_prop, bark_time_prop] =
        decoder.FindProperties("reg-names", "reg", "qcom,pet-time", "qcom,bark-time");

    if (!reg_names_prop || !reg_prop || !pet_time_prop) {
      return std::nullopt;
    }

    auto reg_names = reg_names_prop->AsStringList();
    auto reg = reg_prop->AsReg(decoder);
    auto pet_time = pet_time_prop->AsUint32();
    auto bark_time = bark_time_prop->AsUint32();

    if (!pet_time || !reg || !bark_time) {
      return std::nullopt;
    }

    if (reg->size() > 1 && !reg_names) {
      return std::nullopt;
    }

    size_t base_index = 0;
    if (reg_names) {
      for (auto name : *reg_names) {
        if (name == "wdt-base") {
          break;
        }
        base_index++;
      }
    }

    // Index refers to the reg property array.
    if (base_index >= reg->size()) {
      return std::nullopt;
    }

    auto base_reg = (*reg)[base_index];
    std::optional<uint64_t> base_addr = base_reg.address();
    std::optional<uint64_t> base_size = base_reg.size();

    if (!base_addr || !base_size) {
      return std::nullopt;
    }

    return DecodedNode{
        .addr = *base_addr,
        .size = *base_size,
        .pet_time_ms = *pet_time,
        .bark_time_ms = *bark_time,
    };
  }
};

template <template <typename...> class Template, WatchdogMmioHelper MmioHelper>
using WithAllWatchdogs = Template<MmioHelper, QualcomMsmWatchdog>;

}  // namespace boot_shim

#endif  // ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_WATCHDOG_H_
