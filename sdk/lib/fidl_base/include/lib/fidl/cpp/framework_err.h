// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_FRAMEWORK_ERR_H_
#define LIB_FIDL_CPP_FRAMEWORK_ERR_H_

#include <zircon/fidl.h>

namespace fidl::internal {

enum class FrameworkErr : fidl_framework_err_t {
  kUnknownMethod = FIDL_FRAMEWORK_ERR_UNKNOWN_METHOD,
};

}  // namespace fidl::internal

#endif  // LIB_FIDL_CPP_FRAMEWORK_ERR_H_
