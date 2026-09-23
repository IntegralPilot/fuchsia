// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/values.h"

#include <zircon/assert.h>

namespace fidlc {

std::optional<uint64_t> ConstantValue::AsUnsigned() const {
  switch (kind) {
    case Kind::kUint8:
    case Kind::kZxUchar:
      return AsNumeric<uint8_t>();
    case Kind::kUint16:
      return AsNumeric<uint16_t>();
    case Kind::kUint32:
      return AsNumeric<uint32_t>();
    case Kind::kUint64:
    case Kind::kZxUsize64:
    case Kind::kZxUintptr64:
      return AsNumeric<uint64_t>();
    default:
      return std::nullopt;
  }
}

std::optional<int64_t> ConstantValue::AsSigned() const {
  switch (kind) {
    case Kind::kInt8:
      return AsNumeric<int8_t>();
    case Kind::kInt16:
      return AsNumeric<int16_t>();
    case Kind::kInt32:
      return AsNumeric<int32_t>();
    case Kind::kInt64:
      return AsNumeric<int64_t>();
    default:
      return std::nullopt;
  }
}

std::optional<std::string_view> ConstantValue::AsString() const {
  if (kind == Kind::kString)
    return static_cast<const StringConstantValue*>(this)->value;
  return std::nullopt;
}

// TODO(https://fxbug.dev/42064981): Use std::cmp_* functions when we're on C++20.
// https://en.cppreference.com/w/cpp/utility/intcmp#Possible_implementation
template <class T, class U>
constexpr bool cmp_less(T t, U u) noexcept {
  if constexpr (std::is_signed_v<T> == std::is_signed_v<U>)
    return t < u;
  if constexpr (std::is_signed_v<T>)
    return t < 0 || std::make_unsigned_t<T>(t) < u;
  return u >= 0 && t < std::make_unsigned_t<U>(u);
}

// TODO(https://fxbug.dev/42064981): Use std::in_range when we're on C++20.
template <class R, class T>
constexpr bool in_range(T t) noexcept {
  return !cmp_less(t, std::numeric_limits<R>::min()) && !cmp_less(std::numeric_limits<R>::max(), t);
}

// Explicit instantiations.
template struct NumericConstantValue<int8_t>;
template struct NumericConstantValue<int16_t>;
template struct NumericConstantValue<int32_t>;
template struct NumericConstantValue<int64_t>;
template struct NumericConstantValue<uint8_t>;
template struct NumericConstantValue<uint16_t>;
template struct NumericConstantValue<uint32_t>;
template struct NumericConstantValue<uint64_t>;
template struct NumericConstantValue<float>;
template struct NumericConstantValue<double>;

template <typename ValueType>
bool NumericConstantValue<ValueType>::Convert(Kind kind,
                                              std::unique_ptr<ConstantValue>* out_value) const {
  ZX_ASSERT(out_value != nullptr);
  switch (kind) {
    case Kind::kInt8:
      return ConvertTo<int8_t>(out_value);
    case Kind::kInt16:
      return ConvertTo<int16_t>(out_value);
    case Kind::kInt32:
      return ConvertTo<int32_t>(out_value);
    case Kind::kInt64:
      return ConvertTo<int64_t>(out_value);
    case Kind::kUint8:
    case Kind::kZxUchar:
      return ConvertTo<uint8_t>(out_value);
    case Kind::kUint16:
      return ConvertTo<uint16_t>(out_value);
    case Kind::kUint32:
      return ConvertTo<uint32_t>(out_value);
    case Kind::kUint64:
    case Kind::kZxUsize64:
    case Kind::kZxUintptr64:
      return ConvertTo<uint64_t>(out_value);
    case Kind::kFloat32:
      return ConvertTo<float>(out_value);
    case Kind::kFloat64:
      return ConvertTo<double>(out_value);
    case Kind::kDocComment:
    case Kind::kString:
    case Kind::kBool:
      return false;
  }
}

template <typename ValueType>
template <typename TargetType>
bool NumericConstantValue<ValueType>::ConvertTo(std::unique_ptr<ConstantValue>* out_value) const {
  if constexpr (std::is_integral_v<ValueType>) {
    if constexpr (!std::is_integral_v<TargetType>) {
      return false;
    } else if (!in_range<TargetType>(value)) {
      return false;
    }
  } else if constexpr (std::is_floating_point_v<ValueType>) {
    if constexpr (!std::is_floating_point_v<TargetType>) {
      return false;
    } else if (value < std::numeric_limits<TargetType>::lowest() ||
               value > std::numeric_limits<TargetType>::max()) {
      return false;
    }
  } else {
    static_assert(false, "NumericConstantValue must hold integer or float");
  }
  *out_value = std::make_unique<NumericConstantValue<TargetType>>(value);
  return true;
}

bool BoolConstantValue::Convert(Kind kind, std::unique_ptr<ConstantValue>* out_value) const {
  ZX_ASSERT(out_value != nullptr);
  switch (kind) {
    case Kind::kBool:
      *out_value = std::make_unique<BoolConstantValue>(value);
      return true;
    default:
      return false;
  }
}

bool DocCommentConstantValue::Convert(Kind kind, std::unique_ptr<ConstantValue>* out_value) const {
  ZX_ASSERT(out_value != nullptr);
  switch (kind) {
    case Kind::kDocComment:
      *out_value = std::make_unique<DocCommentConstantValue>(std::string_view(value));
      return true;
    default:
      return false;
  }
}

bool StringConstantValue::Convert(Kind kind, std::unique_ptr<ConstantValue>* out_value) const {
  ZX_ASSERT(out_value != nullptr);
  switch (kind) {
    case Kind::kString:
      *out_value = std::make_unique<StringConstantValue>(std::string_view(value));
      return true;
    default:
      return false;
  }
}

void Constant::ResolveTo(std::unique_ptr<ConstantValue> value, const Type* type) {
  ZX_ASSERT(value != nullptr);
  ZX_ASSERT_MSG(!IsResolved(), "constants should only be resolved once");
  value_ = std::move(value);
  this->type = type;
}

const ConstantValue& Constant::Value() const {
  ZX_ASSERT_MSG(IsResolved(), "accessing the value of an unresolved Constant: %s",
                std::string(span.data()).c_str());
  return *value_;
}

std::unique_ptr<Constant> Constant::Clone() const {
  auto cloned = CloneImpl();
  cloned->compiled = compiled;
  cloned->type = type;
  cloned->value_ = value_ ? value_->Clone() : nullptr;
  return cloned;
}

}  // namespace fidlc
