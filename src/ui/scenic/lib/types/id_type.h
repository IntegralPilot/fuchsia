// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_TYPES_ID_TYPE_H_
#define SRC_UI_SCENIC_LIB_TYPES_ID_TYPE_H_

#include <format>
#include <functional>
#include <ostream>
#include <type_traits>

namespace types {

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
// methods as `DefaultIdTraitsForWireFidl` and `DefaultIdTraitsForWireFidl`, see below.
template <typename IdTraits>
class IdType {
 public:
  using ValueType = typename IdTraits::ValueType;
  using FidlType = typename IdTraits::FidlType;

  // These static_asserts guarantee that it is safe to say e.g.
  //    std::array<ValueType, 10> values;
  //    auto view = fidl::VectorView<FidlType>::FromExternal(
  //        reinterpret_cast<FidlType*>(values.data()), values.size());
  static_assert(sizeof(ValueType) == sizeof(FidlType));
  static_assert(alignof(ValueType) == alignof(FidlType));
  static_assert(std::is_standard_layout_v<ValueType>, "Value type must be standard-layout");
  static_assert(std::is_standard_layout_v<FidlType>, "FIDL type must be standard-layout");

  constexpr IdType() noexcept = default;

  constexpr explicit IdType(const ValueType& int_value) noexcept : value_(int_value) {}

  template <typename _ = std::enable_if<!std::is_same_v<FidlType, ValueType>>>
  constexpr explicit IdType(const FidlType& fidl_value) noexcept
      : value_(IdTraits::FromFidl(fidl_value)) {}

  constexpr IdType(const IdType&) noexcept = default;
  constexpr IdType(IdType&&) noexcept = default;
  constexpr IdType& operator=(const IdType&) noexcept = default;
  constexpr IdType& operator=(IdType&&) noexcept = default;

  ~IdType() = default;

  constexpr explicit operator ValueType() const { return value_; }
  constexpr const ValueType& value() const { return value_; }

  constexpr FidlType ToFidl() const { return IdTraits::ToFidl(value_); }

  constexpr IdType& operator++();
  constexpr IdType operator++(int);

 private:
  ValueType value_;
};

// `IdType` traits implementation that works for most display types.
//
// `ValueT` is the underlying type for the integer ID. Only C++ integer types are
// supported.
//
// `FidlT` is the FIDL type used to represent IDs. It must be a wire type for a
// FIDL struct whose `value` member is the same integer type as `ValueT`.
template <typename ValueT, typename FidlT>
struct DefaultIdTypeTraitsForWireFidl {
  // The ID's underlying type.
  using ValueType = ValueT;

  // The corresponding FIDL wire type.
  using FidlType = FidlT;

  // Converts from the FIDL wire type to the underlying type.
  static constexpr ValueType FromFidl(const FidlType& fidl_value) noexcept {
    static_assert(std::is_same_v<decltype(FidlT::value), ValueT>,
                  "If the FIDL struct's value type does not match the underlying integer type, use "
                  "custom traits, override FromFidl(), and document the safety of a static_cast");

    return fidl_value.value;
  }

  // Converts from the underlying type to the FIDL wire type.
  static constexpr FidlType ToFidl(const ValueType& value) noexcept {
    static_assert(std::is_same_v<decltype(FidlT::value), ValueT>,
                  "If the FIDL struct's value type does not match the underlying integer type, use "
                  "custom traits, override ToFidl(), and document the safety of a static_cast");

    return FidlT{.value = value};
  }
};

template <typename ValueT, typename FidlT>
struct DefaultIdTypeTraitsForNaturalFidl {
  using ValueType = ValueT;
  using FidlType = FidlT;

  static constexpr ValueType FromFidl(const FidlType& fidl_value) noexcept {
    static_assert(std::is_same_v<std::decay_t<decltype(fidl_value.value())>, ValueT>,
                  "If the FIDL struct's value type does not match the underlying integer type, use "
                  "custom traits, override FromFidl(), and document the safety of a static_cast");
    return fidl_value.value();
  }

  static constexpr FidlType ToFidl(const ValueType& value) noexcept {
    static_assert(std::is_same_v<std::decay_t<decltype(std::declval<FidlType>().value())>, ValueT>,
                  "If the FIDL struct's value type does not match the underlying integer type, use "
                  "custom traits, override ToFidl(), and document the safety of a static_cast");
    return FidlType(value);
  }
};

// Equality comparison can be defaulted when C++20 is required.
template <typename IdTraits>
constexpr bool operator==(const IdType<IdTraits>& lhs, const IdType<IdTraits>& rhs) {
  return lhs.value() == rhs.value();
}
template <typename IdTraits>
constexpr bool operator!=(const IdType<IdTraits>& lhs, const IdType<IdTraits>& rhs) {
  return !(lhs == rhs);
}

// Ordering comparison can be defaulted when C++20 is required.
template <typename IdTraits>
constexpr bool operator<(const IdType<IdTraits>& lhs, const IdType<IdTraits>& rhs) {
  return lhs.value() < rhs.value();
}
template <typename IdTraits>
constexpr bool operator>=(const IdType<IdTraits>& lhs, const IdType<IdTraits>& rhs) {
  return !(lhs < rhs);
}
template <typename IdTraits>
constexpr bool operator>(const IdType<IdTraits>& lhs, const IdType<IdTraits>& rhs) {
  return lhs.value() > rhs.value();
}
template <typename IdTraits>
constexpr bool operator<=(const IdType<IdTraits>& lhs, const IdType<IdTraits>& rhs) {
  return !(lhs > rhs);
}

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

template <typename IdTraits>
std::ostream& operator<<(std::ostream& os, const IdType<IdTraits>& id) {
  return os << id.value();
}

}  // namespace types

template <typename IdTraits>
struct std::hash<types::IdType<IdTraits>> {
  size_t operator()(const types::IdType<IdTraits>& id) const noexcept {
    return std::hash<typename IdTraits::ValueType>()(id.value());
  }
};

template <typename IdTraits>
struct std::formatter<types::IdType<IdTraits>>
    : std::formatter<typename types::IdType<IdTraits>::ValueType> {
  auto format(const types::IdType<IdTraits>& id, std::format_context& ctx) const {
    return std::formatter<typename types::IdType<IdTraits>::ValueType>::format(id.value(), ctx);
  }
};

#endif  // SRC_UI_SCENIC_LIB_TYPES_ID_TYPE_H_
