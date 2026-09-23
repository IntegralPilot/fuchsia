// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_ID_TYPE_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_ID_TYPE_H_

#include <compare>
#include <concepts>
#include <format>
#include <functional>
#include <type_traits>

namespace display::internal {

// `IdType` traits implementation that works for most display types.
//
// `ValueT` is the underlying type for the integer ID. Only C++ integer types are
// supported.
//
// `FidlT` is the FIDL type used to represent IDs. It must be a wire type for a
// FIDL struct whose `value` member is the same integer type as `ValueT`.
template <typename ValueT, typename FidlT>
struct DefaultIdTypeTraits {
  // The ID's underlying type.
  using ValueType = ValueT;

  // The corresponding FIDL wire type.
  using FidlType = FidlT;

  // Converts from the FIDL wire type to the underlying type.
  static constexpr ValueType FromFidl(const FidlType& fidl_value) noexcept;

  // Converts from the underlying type to the FIDL wire type.
  static constexpr FidlType ToFidl(const ValueType& value) noexcept;
};

// Newtype pattern implementation for integer identifiers.
//
// Instances are value types, and support being stored in containers. Copying is
// supported. The destructor is trivial.
//
// Instances support incrementing for sequential ID generation.
//
// Instances support being used as container keys, by implementing comparison
// with strict ordering guarantees and a std::hash specialization.
//
// Instances support being used with std::format(), by delegating to the
// std::formatter specialization for the underlying value type.
//
// This type is a zero-cost abstraction. Compiler optimizations will remove any
// overhead associated with it.
//
// `IdTraits` must be a traits structure with the same type aliases and static
// methods as `DefaultIdTraits`.
template <typename IdTraits>
class IdType {
 public:
  using ValueType = typename IdTraits::ValueType;
  using FidlType = typename IdTraits::FidlType;

  constexpr IdType() noexcept = default;

  constexpr explicit IdType(const ValueType& int_value) noexcept : value_(int_value) {}

  constexpr explicit IdType(const FidlType& fidl_value) noexcept
    requires(!std::same_as<FidlType, ValueType>)
      : value_(IdTraits::FromFidl(fidl_value)) {}

  constexpr IdType(const IdType&) noexcept = default;
  constexpr IdType(IdType&&) noexcept = default;
  constexpr IdType& operator=(const IdType&) noexcept = default;
  constexpr IdType& operator=(IdType&&) noexcept = default;

  ~IdType() = default;

  constexpr explicit operator ValueType() const { return value_; }
  constexpr const ValueType& value() const { return value_; }

  constexpr FidlType ToFidl() const { return IdTraits::ToFidl(value_); }

  constexpr bool operator==(const IdType&) const noexcept = default;
  constexpr std::strong_ordering operator<=>(const IdType&) const noexcept = default;

  constexpr IdType& operator++();
  constexpr IdType operator++(int);

 private:
  ValueType value_;
};

template <typename IdTraits>
constexpr IdType<IdTraits>& IdType<IdTraits>::operator++() {
  ++value_;
  return *this;
}

template <typename IdTraits>
constexpr IdType<IdTraits> IdType<IdTraits>::operator++(int) {
  const IdType return_value = *this;
  ++value_;
  return return_value;
}

template <typename ValueT, typename FidlT>
constexpr ValueT DefaultIdTypeTraits<ValueT, FidlT>::FromFidl(const FidlT& fidl_value) noexcept {
  static_assert(std::is_same_v<decltype(FidlT::value), ValueT>,
                "If the FIDL struct's value type does not match the underlying integer type, use "
                "custom traits, override FromFidl(), and document the safety of a static_cast");

  return fidl_value.value;
}

template <typename ValueT, typename FidlT>
constexpr FidlT DefaultIdTypeTraits<ValueT, FidlT>::ToFidl(const ValueT& value) noexcept {
  static_assert(std::is_same_v<decltype(FidlT::value), ValueT>,
                "If the FIDL struct's value type does not match the underlying integer type, use "
                "custom traits, override ToFidl(), and document the safety of a static_cast");

  return FidlT{.value = value};
}

}  // namespace display::internal

template <typename IdTraits>
struct std::hash<display::internal::IdType<IdTraits>> {
  size_t operator()(const display::internal::IdType<IdTraits>& id) const noexcept {
    return std::hash<typename IdTraits::ValueType>()(id.value());
  }
};

template <typename IdTraits>
struct std::formatter<display::internal::IdType<IdTraits>>
    : std::formatter<typename display::internal::IdType<IdTraits>::ValueType> {
  auto format(const display::internal::IdType<IdTraits>& id, std::format_context& ctx) const {
    return std::formatter<typename display::internal::IdType<IdTraits>::ValueType>::format(
        id.value(), ctx);
  }
};

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_ID_TYPE_H_
