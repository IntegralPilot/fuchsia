// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_ENGINE_INFO_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_ENGINE_INFO_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>
#include <zircon/assert.h>

#include <cstdint>

namespace display {

// Equivalent to the FIDL type [`fuchsia.hardware.display.engine/EngineInfo`].
//
// See `::fuchsia_hardware_display_engine::wire::EngineInfo` for references.
//
// Instances are guaranteed to represent display valid engine information.
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
class EngineInfo {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // Number of display configuration layers supported by the display stack.
  static constexpr int kMaxAllowedMaxLayerCount = 8;

  // Number of connected displays supported by the display stack.
  static constexpr int kMaxAllowedMaxConnectedDisplayCount = 1;

  // True iff `fidl_engine_info` is convertible to a valid EngineInfo.
  [[nodiscard]] static constexpr bool IsValid(
      const fuchsia_hardware_display_engine::wire::EngineInfo& fidl_engine_info);

  // `fidl_engine_info` must be convertible to a valid EngineInfo.
  //
  // This is not a constructor to allow designated initializer syntax. Making
  // this a constructor would introduce ambiguity when designated initializer
  // syntax is used, because `fuchsia.hardware.display.engine/EngineInfo` has the
  // same field names as our supported designated initializer syntax.
  [[nodiscard]] static constexpr EngineInfo From(
      const fuchsia_hardware_display_engine::wire::EngineInfo& fidl_engine_info);

  // Constructor that enables the designated initializer syntax with containers.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr EngineInfo(const EngineInfo::ConstructorArgs& args);

  constexpr EngineInfo(const EngineInfo&) noexcept = default;
  constexpr EngineInfo(EngineInfo&&) noexcept = default;
  constexpr EngineInfo& operator=(const EngineInfo&) noexcept = default;
  constexpr EngineInfo& operator=(EngineInfo&&) noexcept = default;
  ~EngineInfo() = default;

  friend constexpr bool operator==(const EngineInfo& lhs, const EngineInfo& rhs);
  friend constexpr bool operator!=(const EngineInfo& lhs, const EngineInfo& rhs);

  constexpr fuchsia_hardware_display_engine::wire::EngineInfo ToFidl() const;

  // Guaranteed to meet the requirements in the FIDL documentation.
  constexpr int max_layer_count() const { return max_layer_count_; }

  // Guaranteed to meet the requirements in the FIDL documentation.
  constexpr int max_connected_display_count() const { return max_connected_display_count_; }

  // Guaranteed to meet the requirements in the FIDL documentation.
  constexpr bool is_capture_supported() const { return is_capture_supported_; }

 private:
  struct ConstructorArgs {
    int max_layer_count = 1;
    int max_connected_display_count = 1;
    bool is_capture_supported = false;
  };

  // In debug mode, asserts that IsValid() would return true.
  //
  // IsValid() variant with developer-friendly debug assertions.
  static constexpr void DebugAssertIsValid(const EngineInfo::ConstructorArgs& args);
  static constexpr void DebugAssertIsValid(
      const fuchsia_hardware_display_engine::wire::EngineInfo& fidl_engine_info);

  int max_layer_count_;
  int max_connected_display_count_;
  bool is_capture_supported_;
};

// static
constexpr bool EngineInfo::IsValid(
    const fuchsia_hardware_display_engine::wire::EngineInfo& fidl_engine_info) {
  if (fidl_engine_info.max_layer_count <= 0) {
    return false;
  }
  if (fidl_engine_info.max_layer_count > kMaxAllowedMaxLayerCount) {
    return false;
  }

  if (fidl_engine_info.max_connected_display_count <= 0) {
    return false;
  }
  if (fidl_engine_info.max_connected_display_count > kMaxAllowedMaxConnectedDisplayCount) {
    return false;
  }

  return true;
}

constexpr EngineInfo::EngineInfo(const EngineInfo::ConstructorArgs& args)
    : max_layer_count_(args.max_layer_count),
      max_connected_display_count_(args.max_connected_display_count),
      is_capture_supported_(args.is_capture_supported) {
  DebugAssertIsValid(args);
}

// static
constexpr EngineInfo EngineInfo::From(
    const fuchsia_hardware_display_engine::wire::EngineInfo& fidl_engine_info) {
  DebugAssertIsValid(fidl_engine_info);

  // The casts do not overflow (causing UB) because of the limits for valid
  // values.
  return EngineInfo({
      .max_layer_count = static_cast<int16_t>(fidl_engine_info.max_layer_count),
      .max_connected_display_count =
          static_cast<int16_t>(fidl_engine_info.max_connected_display_count),
      .is_capture_supported = fidl_engine_info.is_capture_supported,
  });
}

constexpr bool operator==(const EngineInfo& lhs, const EngineInfo& rhs) {
  return lhs.max_layer_count_ == rhs.max_layer_count_ &&
         lhs.max_connected_display_count_ == rhs.max_connected_display_count_ &&
         lhs.is_capture_supported_ == rhs.is_capture_supported_;
}

constexpr bool operator!=(const EngineInfo& lhs, const EngineInfo& rhs) { return !(lhs == rhs); }

constexpr fuchsia_hardware_display_engine::wire::EngineInfo EngineInfo::ToFidl() const {
  // The casts do not underflow (causing UB) because all numeric data members
  // are positive.
  return fuchsia_hardware_display_engine::wire::EngineInfo{
      .max_layer_count = static_cast<uint16_t>(max_layer_count_),
      .max_connected_display_count = static_cast<uint16_t>(max_connected_display_count_),
      .is_capture_supported = is_capture_supported_,
  };
}

// static
constexpr void EngineInfo::DebugAssertIsValid(const EngineInfo::ConstructorArgs& args) {
  ZX_DEBUG_ASSERT(args.max_layer_count > 0);
  ZX_DEBUG_ASSERT(args.max_layer_count <= kMaxAllowedMaxLayerCount);

  ZX_DEBUG_ASSERT(args.max_connected_display_count > 0);
  ZX_DEBUG_ASSERT(args.max_connected_display_count <= kMaxAllowedMaxConnectedDisplayCount);
}

// static
constexpr void EngineInfo::DebugAssertIsValid(
    const fuchsia_hardware_display_engine::wire::EngineInfo& fidl_engine_info) {
  ZX_DEBUG_ASSERT(fidl_engine_info.max_layer_count > 0);
  ZX_DEBUG_ASSERT(fidl_engine_info.max_layer_count <= kMaxAllowedMaxLayerCount);

  ZX_DEBUG_ASSERT(fidl_engine_info.max_connected_display_count > 0);
  ZX_DEBUG_ASSERT(fidl_engine_info.max_connected_display_count <=
                  kMaxAllowedMaxConnectedDisplayCount);
}

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_ENGINE_INFO_H_
