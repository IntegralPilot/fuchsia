// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DL_STATEFUL_ERROR_H_
#define LIB_DL_STATEFUL_ERROR_H_

#include <lib/fit/result.h>

#include "error.h"

namespace dl {

// dl::StatefulError holds a dl::Error object for use in the dlerror() model.
// It's used as `return gPerThreadErrorState(..., error_value);` where ...
// returns some `fit::result<dl::Error, T>` type and error_value is a T.  That
// sets the error state that GetAndClearLastError() returns like dlerror().
class StatefulError {
 public:
  // Convert the fit::result<dl::Error, T> return value into just a T.
  // If result.is_error() then
  template <typename T, typename U>
  T operator()(fit::result<Error, T> result, U error_value) {
    if (result.is_error()) {
      error_.ClearAndAssign(std::move(result).error_value());
      return error_value;
    }
    return std::move(result).value();
  }

  // This works like dlerror(): it returns nullptr if there hasn't been a
  // failing call since the last GetAndClearLastError call.  Regardless,
  // it ends the lifetime of the last string returned by a previous call.
  [[nodiscard]] const char* GetAndClearLastError() { return error_.take_c_str_or_clear(); }

  // Destruction is always harmless, unlike dl::Error.
  ~StatefulError() { error_.clear(); }

 private:
  Error error_;
};

// This is the main instance that's used by the <dlfcn.h> C API functions
// to provide the dlerror() API model.
extern thread_local StatefulError gPerThreadErrorState;

}  // namespace dl

#endif  // LIB_DL_STATEFUL_ERROR_H_
