// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_BACKLIGHT_STATE_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_BACKLIGHT_STATE_H_

#include <fidl/fuchsia.hardware.backlight/cpp/wire.h>
#include <zircon/assert.h>

#include <optional>

namespace display {

// Equivalent to the FIDL type [`fuchsia.hardware.backlight/State`].
//
// See `::fuchsia_hardware_backlight::wire::State` for references.
//
// Instances are guaranteed to represent valid backlight states.
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
class BacklightState {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // True iff `fidl_state` is convertible to a valid BacklightState.
  //
  // If `use_absolute_brightness` is true, the brightness value in `fidl_state`
  // maps to `brightness_nits()` and must meet the constraints documented there.
  // If `use_absolute_brightness` is false, the brightness value in `fidl_state`
  // maps to `brightness_fraction()`, and must meet the constraints documented
  // there.
  [[nodiscard]] static constexpr bool IsValid(
      const fuchsia_hardware_backlight::wire::State& fidl_state, bool use_absolute_brightness);

  // `fidl_state` must be convertible to a valid BacklightState.
  //
  // This is not a constructor to allow designated initializer syntax. Making
  // this a constructor would introduce ambiguity when designated initializer
  // syntax is used, because `fuchsia.hardware.brightness/State` will eventually
  // be migrated to have the same field names as our supported designated
  // initializer syntax.
  //
  // TODO(https://fxbug.dev/419035481): Remove `use_absolute_brightness` when
  // the FIDL State structure becomes unambiguous.
  [[nodiscard]] static constexpr BacklightState From(
      const fuchsia_hardware_backlight::wire::State& fidl_state, bool use_absolute_brightness);

  // Constructor that enables the designated initializer syntax with containers.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr BacklightState(const BacklightState::ConstructorArgs& args);

  constexpr BacklightState(const BacklightState&) noexcept = default;
  constexpr BacklightState(BacklightState&&) noexcept = default;
  constexpr BacklightState& operator=(const BacklightState&) noexcept = default;
  constexpr BacklightState& operator=(BacklightState&&) noexcept = default;
  ~BacklightState() = default;

  friend constexpr bool operator==(const BacklightState& lhs, const BacklightState& rhs);
  friend constexpr bool operator!=(const BacklightState& lhs, const BacklightState& rhs);

  // `use_absolute_brightness` must only be true if the absolute brightness
  // information is not nullopt.
  //
  // Absolute brightness information is dropped if `use_absolute_brightness` is
  // false. Fractional brightness information is dropped if
  // `use_absolute_brightness` is true.
  //
  // TODO(https://fxbug.dev/419035481): Remove `use_absolute_brightness` when
  // the FIDL State structure becomes unambiguous.
  constexpr fuchsia_hardware_backlight::wire::State ToFidl(bool use_absolute_brightness) const;

  // True iff the backlight is turned on.
  //
  // When the backlight is turned off, it emits 0 nits (no light).
  constexpr bool on() const { return on_; }

  // The amount of light emitted by the device, relatively to its maximum power.
  //
  // Guaranteed to be in [0.0, 1.0]. Guaranteed to be 0.0 if `on` is false.
  //
  // 1.0 maps to the maximum brightness level supported by the hardware.
  //
  // 0.0 maps to the minimum configurable brightness level supported by the
  // hardware.
  // TODO(https://fxbug.dev/419035481): Specify the meaning of having `on` set
  // to true and `brightness_fraction()` / `brightness_nits()` set to 0.0.
  constexpr double brightness_fraction() const { return brightness_fraction_; }

  // The amount of light emitted by the device, in nits.
  //
  // Guaranteed to be non-negative. Guaranteed to be 0.0 if `on` is false.
  //
  // Set to nullopt if the driver does not provide guarantees around the amount
  // of light emitted.
  constexpr std::optional<double> brightness_nits() const { return brightness_nits_; }

 private:
  struct ConstructorArgs {
    bool on;
    double brightness_fraction;
    std::optional<double> brightness_nits;
  };

  // In debug mode, asserts that IsValid() would return true.
  //
  // IsValid() variant with developer-friendly debug assertions.
  static constexpr void DebugAssertIsValid(const BacklightState::ConstructorArgs& args);
  static constexpr void DebugAssertIsValid(
      const fuchsia_hardware_backlight::wire::State& fidl_state, bool use_absolute_brightness);

  bool on_;
  double brightness_fraction_;
  std::optional<double> brightness_nits_;
};

// static
constexpr bool BacklightState::IsValid(const fuchsia_hardware_backlight::wire::State& fidl_state,
                                       bool use_absolute_brightness) {
  if (fidl_state.brightness < 0.0) {
    return false;
  }
  if (!use_absolute_brightness && fidl_state.brightness > 1.0) {
    return false;
  }
  if (!fidl_state.backlight_on && fidl_state.brightness != 0.0) {
    return false;
  }

  return true;
}

constexpr BacklightState::BacklightState(const BacklightState::ConstructorArgs& args)
    : on_(args.on),
      brightness_fraction_(args.brightness_fraction),
      brightness_nits_(args.brightness_nits) {
  DebugAssertIsValid(args);
}

// static
constexpr BacklightState BacklightState::From(
    const fuchsia_hardware_backlight::wire::State& fidl_state, bool use_absolute_brightness) {
  DebugAssertIsValid(fidl_state, use_absolute_brightness);

  // The casts do not overflow (causing UB) because of the limits for valid
  // values.
  return BacklightState({
      .on = fidl_state.backlight_on,
      .brightness_fraction = use_absolute_brightness ? 0.0 : fidl_state.brightness,
      .brightness_nits =
          use_absolute_brightness ? std::make_optional(fidl_state.brightness) : std::nullopt,
  });
}

constexpr bool operator==(const BacklightState& lhs, const BacklightState& rhs) {
  return lhs.on_ == rhs.on_ && lhs.brightness_fraction_ == rhs.brightness_fraction_ &&
         lhs.brightness_nits_ == rhs.brightness_nits_;
}

constexpr bool operator!=(const BacklightState& lhs, const BacklightState& rhs) {
  return !(lhs == rhs);
}

constexpr fuchsia_hardware_backlight::wire::State BacklightState::ToFidl(
    bool use_absolute_brightness) const {
  ZX_DEBUG_ASSERT(!use_absolute_brightness || brightness_nits_.has_value());

  // The casts do not underflow (causing UB) because all numeric data members
  // are positive.
  return fuchsia_hardware_backlight::wire::State{
      .backlight_on = on_,
      .brightness = use_absolute_brightness ? brightness_nits_.value() : brightness_fraction_,
  };
}

// static
constexpr void BacklightState::DebugAssertIsValid(const BacklightState::ConstructorArgs& args) {
  ZX_DEBUG_ASSERT(args.brightness_fraction >= 0.0);
  ZX_DEBUG_ASSERT(args.brightness_fraction <= 1.0);
  ZX_DEBUG_ASSERT(!args.brightness_nits.has_value() || args.brightness_nits >= 0.0);
  ZX_DEBUG_ASSERT(args.on || args.brightness_fraction == 0.0);
  ZX_DEBUG_ASSERT(args.on || !args.brightness_nits.has_value() || args.brightness_nits == 0.0);
}

// static
constexpr void BacklightState::DebugAssertIsValid(
    const fuchsia_hardware_backlight::wire::State& fidl_state, bool use_absolute_brightness) {
  ZX_DEBUG_ASSERT(fidl_state.brightness >= 0.0);
  ZX_DEBUG_ASSERT(use_absolute_brightness || fidl_state.brightness <= 1.0);
  ZX_DEBUG_ASSERT(fidl_state.backlight_on || fidl_state.brightness == 0.0);
}

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_BACKLIGHT_STATE_H_
