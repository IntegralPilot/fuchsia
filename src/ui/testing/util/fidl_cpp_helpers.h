// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_TESTING_UTIL_FIDL_CPP_HELPERS_H_
#define SRC_UI_TESTING_UTIL_FIDL_CPP_HELPERS_H_

#define ZX_ASSERT_OK(expression)                                                      \
  {                                                                                   \
    auto&& result = (expression);                                                     \
    if (result.is_error()) {                                                          \
      FX_LOGS(FATAL) << "Error in `" << #expression << "`: " << result.error_value(); \
    }                                                                                 \
  }

#endif  // SRC_UI_TESTING_UTIL_FIDL_CPP_HELPERS_H_
