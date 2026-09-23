// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_DEVICETREE_BOOT_SHIM_H_
#define ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_DEVICETREE_BOOT_SHIM_H_

#include <lib/devicetree/devicetree.h>
#include <lib/devicetree/matcher.h>
#include <lib/fit/function.h>
#include <lib/fit/result.h>
#include <lib/uart/all.h>
#include <lib/zbitl/storage-traits.h>

#include <type_traits>
#include <utility>

#include <fbl/alloc_checker.h>
#include <fbl/macros.h>

#include "boot-shim.h"

namespace boot_shim {

using uart::MmioRange;

template <typename T, typename Shim>
concept DevicetreeItem =
    devicetree::Matcher<T> && std::is_base_of_v<ItemBase, T> && requires(T& t, Shim shim) {
      // `Shim::Init` method will call each of these methods, before it
      // attempts to match items in the devicetree.
      //
      // When successful must return true, otherwise must return false.
      { t.Init(shim) };
    };

constexpr MmioRange MmioRangeFrom(const devicetree::RegPropertyElement& reg) {
  return {.address = reg.address().value_or(0),
          .size = static_cast<size_t>(reg.size().value_or(0))};
}

// Provides an observer for MMIO Ranges. Devicetree Items that will provide configuration for
// kernel drivers, that will be interacted through MMIO must notify through this observer.
//
using DevicetreeBootShimMmioObserver = fit::inline_function<void(const MmioRange&)>;

// A DevicetreeBootShim represents a collection of items, which look into the devicetree itself
// to gather information to produce ZBI items.
//
// A devicetree item requires inspecting the devicetree. In addition to the API requirements from a
// BootShim's item it must fulfill a devicetree Matcher API. |DevicetreeItemBase| provides the
// expected API for the |DevicetreeBootShim|'s items.
//
// Prefer inheriting from |DevicetreeItemBase| when possible.
template <typename... Items>
  requires(sizeof...(Items) > 0)
class DevicetreeBootShim : public BootShim<Items...> {
 private:
  using Base = BootShim<Items...>;

 public:
  static_assert((DevicetreeItem<Items, DevicetreeBootShim> || ...),
                "at least one boot_shim::DevicetreeBootShim<...> template"
                " parameter must be a DevicetreeItem");

  explicit DevicetreeBootShim(const char* name, devicetree::Devicetree dt, FILE* log = stdout)
      : Base(name, log), dt_(dt) {}

  // Initializes all devicetree boot shim items.  As part of the initialization
  // each matcher's `Init(shim_name, log)` is called, followed by a single
  // invocation of `devicetree::Match| allowing each provided matcher to
  // collect information from the devicetree.
  bool Init() {
    auto match_with = [this](auto&... items)
      requires(sizeof...(items) > 0)
    {
      (items.Init(*this), ...);
      return devicetree::Match(dt_, items...);
    };

    return this->template OnSelectItems<DevicetreeShimItem>(match_with);
  }

  const devicetree::Devicetree& devicetree() const { return dt_; }

  const Allocator& allocator() const {
    ZX_ASSERT(allocator_);
    return allocator_;
  }
  const DevicetreeBootShimMmioObserver& mmio_observer() const {
    ZX_ASSERT(mmio_observer_);
    return mmio_observer_;
  }

  void set_allocator(Allocator&& allocator) { allocator_ = std::move(allocator); }

  // Optional: Set a callback for MMIO Ranges of interest for each |Item|
  // of the shim.
  void set_mmio_observer(DevicetreeBootShimMmioObserver&& observer) {
    mmio_observer_ = std::move(observer);
  }

 private:
  // Alias capturing the `Shim` type for filtering items.
  template <typename T>
  using DevicetreeShimItem = std::bool_constant<DevicetreeItem<T, DevicetreeBootShim>>;

  devicetree::Devicetree dt_;
  Allocator allocator_ = nullptr;
  DevicetreeBootShimMmioObserver mmio_observer_ = [](const MmioRange&) {};
};

}  // namespace boot_shim

#endif  // ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_DEVICETREE_BOOT_SHIM_H_
