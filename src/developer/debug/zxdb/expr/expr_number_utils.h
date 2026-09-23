// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_EXPR_EXPR_NUMBER_UTILS_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_EXPR_EXPR_NUMBER_UTILS_H_

#include <cstdint>
#include <string>

#include "src/developer/debug/zxdb/common/err.h"

namespace zxdb {

[[nodiscard]] Err StringToInt(const std::string& s, int* out);
[[nodiscard]] Err StringToInt64(const std::string& s, int64_t* out);
[[nodiscard]] Err StringToUint32(const std::string& s, uint32_t* out);
[[nodiscard]] Err StringToUint64(const std::string& s, uint64_t* out);

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_EXPR_EXPR_NUMBER_UTILS_H_
