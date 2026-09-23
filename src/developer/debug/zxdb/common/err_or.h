// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_COMMON_ERR_OR_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_COMMON_ERR_OR_H_

#include "lib/fit/function.h"
#include "src/developer/debug/shared/result.h"
#include "src/developer/debug/zxdb/common/err.h"

namespace zxdb {

// Specialization of the generic Result class that uses an Err.
template <typename T>
class ErrOr {
 public:
  // The Err must be set when constructing this object in an error state.
  ErrOr(Err e) : inner_(debug::Result<T, Err>(std::move(e))) {
    FX_DCHECK(inner_.err().has_error());
  }

  ErrOr(T t) : inner_(debug::Result<T, Err>(std::move(t))) {}

  bool ok() const { return inner_.ok(); }
  bool has_error() const { return inner_.has_error(); }

  const Err& err() const {
    // |inner_| will assert if there is no error. This assert is to make sure that the Err is not
    // ErrType::kNone.
    FX_DCHECK(inner_.err().has_error());
    return inner_.err();
  }

  const T& value() const { return inner_.value(); }
  T& value() { return inner_.value(); }
  T take_value() { return std::move(inner_.take_value()); }

  // Implicit conversion to bool indicating "OK".
  explicit operator bool() const { return inner_.ok(); }

  Err err_or_empty() const {  // Makes a copy of the error.
    if (inner_.has_error())
      return inner_.err();
    return Err();
  }
  T value_or_empty() const {  // Makes a copy of the value.
    return inner_.value_or_empty();
  }
  T take_value_or_empty() {  // Destructively moves the value out.
    return std::move(inner_.take_value_or_empty());
  }

  // Comparators for unit tests.
  bool operator==(const ErrOr<T>& other) const { return inner_ == other.inner_; }
  bool operator!=(const ErrOr<T>& other) const { return !operator==(other); }

  // Adapts an old-style callback that takes two parameters and returns a newer one that takes an
  // ErrOr.
  static fit::callback<void(ErrOr<T>)> FromPairCallback(fit::callback<void(const Err&, T)> cb) {
    return [cb = std::move(cb)](ErrOr<T> value) mutable {
      cb(value.err_or_empty(), value.take_value_or_empty());
    };
  }

 private:
  debug::Result<T, Err> inner_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_COMMON_ERR_OR_H_
