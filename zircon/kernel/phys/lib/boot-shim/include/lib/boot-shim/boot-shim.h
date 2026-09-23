// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_BOOT_SHIM_H_
#define ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_BOOT_SHIM_H_

#include <lib/elfldltl/note.h>
#include <lib/fit/function.h>
#include <lib/zbi-format/zbi.h>
#include <stdio.h>

#include <array>
#include <concepts>
#include <optional>
#include <span>
#include <string_view>
#include <tuple>
#include <type_traits>
#include <utility>

#include <fbl/alloc_checker.h>

#include "item-base.h"

namespace boot_shim {

// Proxy for allocator. This allocator represents the following interface:
//
//   void* Allocator(size_t byte_count, size_t alignment);
//
// On success returns a non NULL pointer to an aligned memory block of at least |byte_count| bytes.
// On failure |nullptr| is returned.
using Allocator = fit::inline_function<void*(size_t, size_t, fbl::AllocChecker&), 32>;

// See the BootShim template class, below.
class BootShimBase : public ItemBase {
 public:
  constexpr BootShimBase(const BootShimBase&) = default;

  explicit constexpr BootShimBase(const char* shim_name, FILE* log = stdout)
      : shim_name_(shim_name), log_(log) {}

  const char* shim_name() const { return shim_name_; }

  FILE* log() const { return log_; }

  bool Check(const char* what, fit::result<std::string_view> result) const;

  bool Check(const char* what, fit::result<InputZbi::Error> result) const;

  bool Check(const char* what, fit::result<InputZbi::CopyError<WritableBytes>> result) const;

  bool Check(const char* what, fit::result<DataZbi::Error> result) const;

 protected:
  using LinuxBootConfig = SingleItem<ZBI_TYPE_LINUX_BOOTCONFIG>;

  class Cmdline : public ItemBase {
   public:
    enum Index : size_t { kName, kInfo, kLegacy, kCount };

    constexpr std::string_view& operator[](Index i) { return chunks_[i]; }

    constexpr std::string_view operator[](Index i) const { return chunks_[i]; }

    void set_build_id(const elfldltl::ElfNote& build_id) { build_id_ = build_id; }

    void set_strings(std::span<std::string_view> strings) { strings_ = strings; }
    void set_cstr(std::span<const char*> cstr) { cstr_ = cstr; }

    size_t size_bytes() const;
    fit::result<DataZbi::Error> AppendItems(DataZbi& zbi) const;

   private:
    size_t Collect(std::optional<WritableBytes> payload = std::nullopt) const;

    elfldltl::ElfNote build_id_;
    std::array<std::string_view, kCount> chunks_;
    std::span<std::string_view> strings_;
    std::span<const char*> cstr_;
  };

  void Log(const Cmdline& cmdline, ByteView ramdisk) const;

 private:
  const char* shim_name_;
  FILE* log_;
};

// BootShim is a base class for implementing boot shims.  The model is that the
// shim starts up, collects data from whatever legacy sources it has, then
// ingests the ZBI, then appends "bootloader-provided" items to the data ZBI.
// This class manages the data collection and tracks what items to append.
//
// Each template argument is a class implementing the boot_shim::ItemBase API.
// Each "item" can produce zero, one, or more ZBI items at runtime.
//
// In several shims, everything must be figured out so that the final image
// sizes are all known before any memory allocation can be done.  So first a
// data collection pass stores everything it needs in each item object.  (The
// BootShim base class does not do this part except for the CMDLINE item.)
//
// Then BootShim::size_bytes() on BootShim sums Items::size_bytes() so the shim
// can reserve memory for the data ZBI.  Once the shim has ingested the input
// ZBI and set up memory allocation it can set up the data ZBI with as much
// extra capacity as size_bytes() requested.  Then BootShim::AppendItems
// iterates across Items::AppendItems calls.  The shim is now ready to boot.
template <class... Items>
  requires(sizeof...(Items) > 0)
class BootShim : public BootShimBase {
 private:
  using ItemsTuple = std::tuple<Cmdline, LinuxBootConfig, Items...>;

  template <typename F, typename T>
  static constexpr bool kCanApply = false;

  template <typename F, typename... Args>
  static constexpr bool kCanApply<F, std::tuple<Args...>> = std::is_invocable_v<F, Args...>;

  template <template <typename> typename Predicate, typename ItemTuple>
  static constexpr auto SelectItems(ItemTuple& item_tuple) {
    auto on = [](auto&... items) { return SelectItemsImpl<Predicate>(items...); };
    return std::apply(on, item_tuple);
  }

 public:
  // Move-only, not default-constructible.
  BootShim() = delete;
  BootShim(const BootShim&) = delete;
  constexpr BootShim(BootShim&&) noexcept = default;
  constexpr BootShim& operator=(const BootShim&) = delete;
  constexpr BootShim& operator=(BootShim&&) noexcept = default;

  // The caller must supply the shim's own program name as a string constant.
  // This string is used in log messages and in "bootloader.name=...".  In real
  // phys shims, this is always Symbolize::kProgramName_ and stdout is the only
  // FILE* there is.  Other log streams can be used in testing.
  explicit constexpr BootShim(const char* shim_name, FILE* log = stdout)
      : BootShimBase(shim_name, log) {
    Get<Cmdline>()[Cmdline::kName] = shim_name;
  }

  // These fluent setters contribute to the built-in ZBI_TYPE_CMDLINE item,
  // along with the mandatory shim name from the constructor.

  constexpr BootShim& set_info(std::string_view str) {
    Get<Cmdline>()[Cmdline::kInfo] = str;
    return *this;
  }

  constexpr BootShim& set_build_id(const elfldltl::ElfNote& build_id) {
    Get<Cmdline>().set_build_id(build_id);
    return *this;
  }

  constexpr BootShim& set_cmdline(std::string_view str) {
    // Remove any incoming trailing NULs, just in case.
    if (auto pos = str.find_last_not_of('\0'); pos != std::string_view::npos) {
      Get<Cmdline>()[Cmdline::kLegacy] = str.substr(0, pos + 1);
    }
    return *this;
  }

  constexpr BootShim& set_cmdline_strings(std::span<std::string_view> strings) {
    Get<Cmdline>().set_strings(strings);
    return *this;
  }

  constexpr BootShim& set_cmdline_cstrings(std::span<const char*> cstrings) {
    Get<Cmdline>().set_cstr(cstrings);
    return *this;
  }

  constexpr std::string_view legacy_cmdline() const { return Get<Cmdline>()[Cmdline::kLegacy]; }

  constexpr BootShim& set_linux_boot_config(std::span<const std::byte> boot_config) {
    Get<LinuxBootConfig>().set_payload(boot_config);
    return *this;
  }

  // Log how things look after calling set_* methods.
  void Log(ByteView ramdisk) const { BootShimBase::Log(Get<Cmdline>(), ramdisk); }

  // Return the total size (upper bound) of additional data ZBI items.
  constexpr size_t size_bytes() const {
    return OnItems([](auto&... items) { return (items.size_bytes() + ...); });
  }

  // Append additional items to the data ZBI.  The caller ensures there is as
  // much spare capacity as size_bytes() previously returned.
  fit::result<DataZbi::Error> AppendItems(DataZbi& zbi) {
    fit::result<DataZbi::Error> result = fit::ok();
    auto append = [&zbi, &result](auto& item) {
      result = item.AppendItems(zbi);
      return result.is_error();
    };
    auto res = AnyItem(append) ? result : fit::ok();

    return res;
  }

  // Get the item object of a particular type (among Items).
  template <typename T>
  constexpr T& Get() {
    static_assert(std::is_same_v<T, Cmdline> || std::is_same_v<T, LinuxBootConfig> ||
                  (std::is_same_v<T, Items> || ...));
    return std::get<T>(items_);
  }
  template <typename T>
  constexpr const T& Get() const {
    static_assert(std::is_same_v<T, Cmdline> || std::is_same_v<T, LinuxBootConfig> ||
                  (std::is_same_v<T, Items> || ...));
    return std::get<T>(items_);
  }

  // This calls item.Init(args..., shim_name(), log()).
  template <class Item, typename... Args>
  decltype(auto) InitItem(Item& item, Args&&... args) {
    return item.Init(std::forward<Args>(args)..., shim_name(), log());
  }

  // This calls the Get<Item>() item method Init(args..., shim_name(), log()).
  template <class Item, typename... Args>
  decltype(auto) InitGetItem(Args&&... args) {
    return InitItem(Get<Item>(), std::forward<Args>(args)...);
  }

  // Returns callback(Items&...).
  template <std::invocable<Cmdline&, Items&...> T>
  constexpr decltype(auto) OnItems(T&& callback) {
    return std::apply(std::forward<T>(callback), items_);
  }
  template <std::invocable<Cmdline&, Items&...> T>
  constexpr decltype(auto) OnItems(T&& callback) const {
    return std::apply(std::forward<T>(callback), items_);
  }

  // Returns the result of `callback(item_0, ... , item_n)`, where each item
  // used as argument corresponds to the shim items where
  // `Predicate<decltype(item_i)>::value` is true.  The relative order between
  // items is preserved.
  //
  // Example:
  // // Assuming the items are:
  // std::tuple<T0, T1, T2, T3> items_;
  //
  // And Predicate<T>::value for each of item types are respectively:
  // `{T0: false, T1: true, T2: true, T3: false}` then the generated call
  // is `callback(T1&, T2&)` and equivalent to
  // `callback(std::get<1>(items_), std::get<2>(items)`.
  template <template <typename> typename Predicate, typename T>
    requires(kCanApply<T, decltype(SelectItems<Predicate>(std::declval<ItemsTuple&>()))>)
  constexpr decltype(auto) OnSelectItems(T&& callback) {
    return std::apply(std::forward<T>(callback), SelectItems<Predicate>(items_));
  }
  template <template <typename> typename Predicate, typename T>
    requires(kCanApply<T, decltype(SelectItems<Predicate>(std::declval<const ItemsTuple&>()))>)
  constexpr decltype(auto) OnSelectItems(T&& callback) const {
    return std::apply(std::forward<T>(callback), SelectItems<Predicate>(items_));
  }

  // Calls callback(item) for each of Items.
  // If Base is given, the items not derived from Base are skipped.
  template <typename Base = void>
  constexpr void ForEachItem(auto&& callback) {
    OnItems([&](auto&... item) { (IfBase<Base>(item, callback), ...); });
  }
  template <typename Base = void>
  constexpr void ForEachItem(auto&& callback) const {
    OnItems([&](auto&... item) { (IfBase<Base>(item, callback), ...); });
  }

  // Returns callback(item) && ... for each of Items.
  // If Base is given, the items not derived from Base are skipped.
  template <typename Base = void>
  constexpr bool EveryItem(auto&& callback) {
    return OnItems([&](auto&... item) { return (IfBase<Base, true>(item, callback), ...); });
  }
  template <typename Base = void>
  constexpr bool EveryItem(auto&& callback) const {
    return OnItems([&](auto&... item) { return (IfBase<Base, true>(item, callback), ...); });
  }

  // Returns callback(item) || ... for each of Items.
  // If Base is given, the items not derived from Base are skipped.
  template <typename Base = void>
  constexpr bool AnyItem(auto&& callback) {
    return OnItems([&](auto&... item) { return (IfBase<Base, false>(item, callback), ...); });
  }
  template <typename Base = void>
  constexpr bool AnyItem(auto&& callback) const {
    return OnItems([&](auto&... item) { return (IfBase<Base, false>(item, callback), ...); });
  }

  // This takes any number of callbacks invocable as void(Item&) for some Item
  // type.  For each item, each callback that is invocable with that item type
  // will be called.  This makes it easy to just list type-specific callbacks
  // together in one call:
  // ```
  // shim.OnInvocableItems([](ItemA& a) { ... }, [](ItemB& b) { ... });
  // ```
  // Note that it's no error if a given callback is not invocable on any
  // item, or even if no callback is invocable on any item.  So when using
  // this, runtime tests are required to ensure that each callback has the
  // correct signature to get called when it's meant to be.
  template <typename... T>
  constexpr void OnInvocableItems(T&&... callbacks) {
    return OnItems(OnInvocableCallback(callbacks...));
  }
  template <typename... T>
  constexpr void OnInvocableItems(T&&... callbacks) const {
    return OnItems(OnInvocableCallback(callbacks...));
  }

 private:
  template <typename Base, class Item>
  static constexpr bool kHasBase = std::is_base_of_v<Base, Item>;

  template <class Item>
  static constexpr bool kHasBase<Item, Item> = true;

  template <class Item>
  static constexpr bool kHasBase<void, Item> = true;

  template <typename Base, bool IfNot = false, class Item, typename Callback>
  static constexpr bool IfBase(Item& item, Callback&& callback) {
    return kHasBase<Base, Item> ? std::forward<Callback>(callback)(item) : IfNot;
  }

  template <template <typename> typename Predicate, typename Item, typename... Unfiltered>
  static constexpr auto SelectItemsImpl(Item& item, Unfiltered&... unfiltered) {
    return SelectItemsImpl<Predicate>(std::tuple<>(), item, unfiltered...);
  }

  template <template <typename> typename Predicate, typename... Filtered, typename Item,
            typename... Unfiltered>
  static constexpr auto SelectItemsImpl(std::tuple<Filtered&...> filtered, Item& item,
                                        Unfiltered&... unfiltered) {
    if constexpr (Predicate<Item>::value) {
      auto new_filtered = std::tuple_cat(std::move(filtered), std::forward_as_tuple(item));
      if constexpr (sizeof...(Unfiltered) == 0) {
        return new_filtered;
      } else {
        return SelectItemsImpl<Predicate>(std::move(new_filtered), unfiltered...);
      }
    } else {
      if constexpr (sizeof...(Unfiltered) == 0) {
        return filtered;
      } else {
        return SelectItemsImpl<Predicate>(std::move(filtered), unfiltered...);
      }
    }
  }

  template <typename Callback, class Item>
  static constexpr void CallIfInvocable(Callback& callback, Item& item) {
    if constexpr (std::is_invocable_v<Callback&, Item&>) {
      callback(item);
    }
  }

  // This returns a lambda that captures the callbacks by reference.
  static constexpr auto OnInvocableCallback(auto&... callbacks) {
    return [on_item = [&](auto& item) {  // Try each callback on this item.
      (CallIfInvocable(callbacks, item), ...);
    }](auto&... item) {  // Try all the callbacks on each item.
      (on_item(item), ...);
    };
  }

  ItemsTuple items_;
};

// Helper struct used to generate a `BootShim` item with a set of `Items`.
//
// Use `Shim` helper alias to instiatiate `BootShim` templates.
//
// This helper struct provides internal helpers to allow adding or removing items
// from this set.
template <typename... Items>
struct StandardBootShimItems {
 private:
  template <typename Item>
  static constexpr bool kItemInPack = (std::is_same_v<Item, Items> || ...);

 public:
  using type = StandardBootShimItems;

  // Instantiates the `ShimType` template with `Items` resulting in `ShimType<Items...>`.
  template <template <typename...> typename ShimType>
  using Shim = ShimType<Items...>;

  // `Add` will generate a `struct` with a `type` member whose type is `StandardBootShim`
  // instantiated with the the union of `Items` and `ExtraItems`.
  //
  // `ExtraItems` must not contain any item already present in `Items`, that is, they must
  // not overlap.
  template <typename... ExtraItems>
    requires(sizeof...(ExtraItems) > 0 && (!kItemInPack<ExtraItems> && ...))
  struct Add {
    using type = StandardBootShimItems<Items..., ExtraItems...>;
  };

  // `Remove` will generate a `struct` with a `type` member whose type is `StandardBootShim`
  // instantiated with the the difference of `Items` and `ExtraItems`, that is all the items in
  // `Items` that are not preent in `RemoveItems`.
  //
  // Every item in `ExtraItems` must be present in `Items`, that is, `ExtraItems` is a subset of
  // `Items`.
  template <typename... RemoveItems>
    requires(sizeof...(RemoveItems) > 0 && (kItemInPack<RemoveItems> && ...))
  struct Remove {
   private:
    template <typename Item>
    static constexpr bool kShouldFilterItem = (std::is_same_v<Item, RemoveItems> || ...);

    template <typename... Args>
    struct ShimType;

    template <typename... Args>
    struct ShimType<std::tuple<Args...>> {
      using type = StandardBootShimItems<Args...>;
    };

   public:
    using type = ShimType<decltype(std::tuple_cat(
        std::declval<std::conditional_t<kShouldFilterItem<Items>, std::tuple<>,
                                        std::tuple<Items>>>()...))>::type;
  };
};

}  // namespace boot_shim

#endif  // ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_BOOT_SHIM_H_
