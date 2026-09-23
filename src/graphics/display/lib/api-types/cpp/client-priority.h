// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_CLIENT_PRIORITY_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_CLIENT_PRIORITY_H_

#include <fidl/fuchsia.hardware.display/cpp/wire.h>

#include <compare>
#include <format>

namespace display {

// Equivalent to the FIDL type [`fuchsia.hardware.display/ClientPriority`].
//
// This is a value type with a strong ordering relationship. Instances can be
// stored in containers and sorted. Copying, moving and destruction are trivial.
class ClientPriority {
 public:
  explicit constexpr ClientPriority(fuchsia_hardware_display::wire::ClientPriorityValue fidl_value)
      : value_(fidl_value) {}

  constexpr ClientPriority(const ClientPriority&) noexcept = default;
  constexpr ClientPriority(ClientPriority&&) noexcept = default;
  constexpr ClientPriority& operator=(const ClientPriority&) = default;
  constexpr ClientPriority& operator=(ClientPriority&&) noexcept = default;
  ~ClientPriority() = default;

  constexpr fuchsia_hardware_display::wire::ClientPriority ToFidl() const {
    return {.value = value_};
  }
  constexpr fuchsia_hardware_display::wire::ClientPriorityValue ToFidlValue() const {
    return value_;
  }

  constexpr bool operator==(const ClientPriority&) const noexcept = default;
  constexpr std::strong_ordering operator<=>(const ClientPriority&) const noexcept = default;

  // Raw numerical value of the equivalent FIDL value.
  //
  // This is intended to be used for developer-facing output, such as logging
  // and Inspect. The values have the same stability guarantees as the
  // equivalent FIDL type.
  constexpr uint32_t ValueForLogging() const { return value_; }

  // See [`fuchsia.hardware.display/INVALID_CLIENT_PRIORITY_VALUE`].
  static const ClientPriority kInvalid;

  // See [`fuchsia.hardware.display/VIRTCON_CLIENT_PRIORITY_VALUE`].
  static const ClientPriority kVirtcon;

  // See [`fuchsia.hardware.display/COMPOSITOR_CLIENT_PRIORITY_VALUE`].
  static const ClientPriority kCompositor;

  // See [`fuchsia.hardware.display/TEST_UTILITY_CLIENT_PRIORITY_VALUE`].
  static const ClientPriority kTestUtility;

 private:
  fuchsia_hardware_display::wire::ClientPriorityValue value_;
};

constexpr inline ClientPriority ClientPriority::kInvalid(
    fuchsia_hardware_display::wire::kInvalidClientPriorityValue);
constexpr inline ClientPriority ClientPriority::kVirtcon(
    fuchsia_hardware_display::wire::kVirtconClientPriorityValue);
constexpr inline ClientPriority ClientPriority::kCompositor(
    fuchsia_hardware_display::wire::kCompositorClientPriorityValue);
constexpr inline ClientPriority ClientPriority::kTestUtility(
    fuchsia_hardware_display::wire::kTestUtilityClientPriorityValue);

}  // namespace display

template <>
struct std::formatter<display::ClientPriority> : std::formatter<uint32_t> {
  auto format(const display::ClientPriority& client_priority, std::format_context& ctx) const {
    return std::formatter<uint32_t>::format(client_priority.ValueForLogging(), ctx);
  }
};

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_CLIENT_PRIORITY_H_
