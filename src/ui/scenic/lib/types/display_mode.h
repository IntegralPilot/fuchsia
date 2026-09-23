// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_TYPES_DISPLAY_MODE_H_
#define SRC_UI_SCENIC_LIB_TYPES_DISPLAY_MODE_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>

#include "src/ui/scenic/lib/types/extent2.h"
#include "src/ui/scenic/lib/types/util/hash_combine.h"

namespace types {

class DisplayMode {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // Returns true iff the args can be used to construct a valid DisplayMode.
  // If `should_assert` is true, invalid args will trigger a FX_DCHECK.
  //
  // Validity constraints:
  // - `active_area` extent must not be empty.
  [[nodiscard]] static constexpr bool IsValid(const ConstructorArgs& args,
                                              bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(
      const fuchsia_hardware_display_types::wire::Mode& fidl, bool should_assert = false);

  // Constructor.  All arguments must be valid; use `IsValid()` to validate if you're not sure.
  [[nodiscard]] static constexpr DisplayMode From(
      const fuchsia_hardware_display_types::wire::Mode& fidl_mode);

  // Constructor that enables the designated initializer syntax.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr DisplayMode(const ConstructorArgs& args);

  // Empty mode.  Allows usage as key in std C++ containers.
  constexpr DisplayMode();

  constexpr DisplayMode(const DisplayMode&) noexcept = default;
  constexpr DisplayMode(DisplayMode&&) noexcept = default;
  constexpr DisplayMode& operator=(const DisplayMode&) noexcept = default;
  constexpr DisplayMode& operator=(DisplayMode&&) noexcept = default;
  ~DisplayMode() = default;

  friend constexpr bool operator==(const DisplayMode& lhs, const DisplayMode& rhs);
  friend constexpr bool operator!=(const DisplayMode& lhs, const DisplayMode& rhs);

  fuchsia_hardware_display_types::wire::Mode ToWire() const;

  constexpr const Extent2& active_area() const { return active_area_; }
  constexpr const uint32_t& refresh_rate_millihertz() const { return refresh_rate_millihertz_; }
  constexpr const uint32_t& mode_flags() const { return mode_flags_; }

 private:
  struct ConstructorArgs {
    Extent2 active_area;
    uint32_t refresh_rate_millihertz;
    uint32_t mode_flags;
  };

  Extent2 active_area_;
  uint32_t refresh_rate_millihertz_;
  // Reserved, must be 0 for now.
  uint32_t mode_flags_;
};

// static
constexpr bool DisplayMode::IsValid(const ConstructorArgs& args, bool should_assert) {
  if (args.active_area.IsEmpty()) {
    FX_DCHECK(!should_assert) << "active_area must not be empty: " << args.active_area;
    return false;
  }
  if (args.refresh_rate_millihertz == 0) {
    FX_DCHECK(!should_assert) << "refresh_rate_millihertz must be positive: "
                              << args.refresh_rate_millihertz;
    return false;
  }
  if (args.mode_flags != 0) {
    FX_DCHECK(!should_assert) << "mode_flags must be zero: " << std::hex << args.mode_flags
                              << std::dec;
    return false;
  }
  return true;
}

// static
constexpr bool DisplayMode::IsValid(const fuchsia_hardware_display_types::wire::Mode& fidl_mode,
                                    bool should_assert) {
  if (!Extent2::IsValid(fidl_mode.active_area, should_assert)) {
    return false;
  }
  return IsValid(ConstructorArgs{
      .active_area = Extent2::From(fidl_mode.active_area),
      .refresh_rate_millihertz = fidl_mode.refresh_rate_millihertz,
      .mode_flags = static_cast<uint32_t>(fidl_mode.flags),
  });
}

// static
constexpr DisplayMode DisplayMode::From(
    const fuchsia_hardware_display_types::wire::Mode& fidl_mode) {
  // No need for `IsValid()` here; this will be handled by the designated initializer constructor.
  return DisplayMode({
      .active_area = Extent2::From(fidl_mode.active_area),
      .refresh_rate_millihertz = fidl_mode.refresh_rate_millihertz,
      .mode_flags = static_cast<uint32_t>(fidl_mode.flags),
  });
}

constexpr DisplayMode::DisplayMode(const DisplayMode::ConstructorArgs& args)
    : active_area_(args.active_area),
      refresh_rate_millihertz_(args.refresh_rate_millihertz),
      mode_flags_(args.mode_flags) {
  auto _ = IsValid(args, true);
}

constexpr DisplayMode::DisplayMode()
    : active_area_({.width = 0, .height = 0}), refresh_rate_millihertz_(0), mode_flags_(0) {}

constexpr bool operator==(const DisplayMode& lhs, const DisplayMode& rhs) {
  return lhs.active_area_ == rhs.active_area_ &&
         lhs.refresh_rate_millihertz_ == rhs.refresh_rate_millihertz_ &&
         lhs.mode_flags_ == rhs.mode_flags_;
}

constexpr bool operator!=(const DisplayMode& lhs, const DisplayMode& rhs) { return !(lhs == rhs); }

inline fuchsia_hardware_display_types::wire::Mode DisplayMode::ToWire() const {
  return fuchsia_hardware_display_types::wire::Mode{
      .active_area = active_area_.ToWire(),
      .refresh_rate_millihertz = refresh_rate_millihertz_,
      .flags = fuchsia_hardware_display_types::wire::ModeFlags(mode_flags_),

  };
}

inline std::ostream& operator<<(std::ostream& str, const DisplayMode& dm) {
  str << "{active_area=" << dm.active_area()
      << ", refresh_rate_millihertz=" << dm.refresh_rate_millihertz() << ", flags=0x" << std::hex
      << dm.mode_flags() << std::dec << "}";
  return str;
}

}  // namespace types

namespace std {

template <>
struct hash<types::DisplayMode> {
  std::size_t operator()(const types::DisplayMode& dm) const {
    // Random seed (`openssl rand -hex 8`) avoids collisions with types with the same memory layout.
    std::size_t seed = 0xb3605c2575872132;
    types::hash_combine(seed, dm.active_area());
    types::hash_combine(seed, dm.refresh_rate_millihertz());
    types::hash_combine(seed, dm.mode_flags());
    return seed;
  }
};

}  // namespace std

#endif  // SRC_UI_SCENIC_LIB_TYPES_DISPLAY_MODE_H_
