// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_SHARED_RESULT_H_
#define SRC_DEVELOPER_DEBUG_SHARED_RESULT_H_

#include <lib/syslog/cpp/macros.h>

#include <variant>

namespace debug {

template <typename T, typename E>
class Result {
  using Variant = std::variant<T, E>;

 public:
  Result(E e) : variant_(std::move(e)) {}

  Result(T v) : variant_(std::move(v)) {}

  bool ok() const { return !std::holds_alternative<E>(variant_); }
  bool has_error() const { return std::holds_alternative<E>(variant_); }

  // Requires that has_error be true or this function will crash. See also err_or_empty().
  const E& err() const {
    FX_DCHECK(has_error());
    return std::get<E>(variant_);
  }

  // Requires that has_error be false or this function will crash. See also [take_]value_or_empty().
  const T& value() const {
    FX_DCHECK(!has_error());
    return std::get<T>(variant_);
  }
  T& value() {
    FX_DCHECK(!has_error());
    return std::get<T>(variant_);
  }
  T take_value() {  // Destructively moves the value out.
    FX_DCHECK(!has_error());
    return std::move(std::get<T>(variant_));
  }

  // Implicit conversion to bool indicating "OK".
  explicit operator bool() const { return ok(); }

  // These functions are designed for integrating with code that uses an (E, T) pair. If this
  // class isn't in the corresponding state, default constructs the missing object.
  //
  // The value version can optionally destructively move the value out (with the "take" variant)
  // since some values are inefficient to copy and often this is used in a context where the value
  // is no longer needed.
  //
  // The E version does not allow destructive moving because it would leave this object in an
  // inconsistent state where the error object is stored in the variant, but err().has_error() is
  // not set. We assume that errors are unusual so are not worth optimizing for saving one string
  // copy to avoid this.
  E err_or_empty() const {  // Makes a copy of the error.
    if (has_error())
      return std::get<E>(variant_);
    return E();
  }
  T value_or_empty() const {  // Makes a copy of the value.
    if (!has_error())
      return std::get<T>(variant_);
    return T();
  }
  T take_value_or_empty() {  // Destructively moves the value out.
    if (!has_error())
      return std::move(std::get<T>(variant_));
    return T();
  }

  // Comparators for unit tests.
  bool operator==(const Result<T, E>& other) const {
    if (ok() != other.ok())
      return false;
    if (ok())
      return value() == other.value();
    return err() == other.err();
  }
  bool operator!=(const Result<T, E>& other) const { return !operator==(other); }

 private:
  Variant variant_;
};

}  // namespace debug

#endif  // SRC_DEVELOPER_DEBUG_SHARED_RESULT_H_
