// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_MODULES_TEST_UBSAN_H_
#define LIB_LD_TEST_MODULES_TEST_UBSAN_H_

#include <string_view>

namespace ubsan {

int LogError(std::string_view str);

}  // namespace ubsan

#endif  // LIB_LD_TEST_MODULES_TEST_UBSAN_H_
