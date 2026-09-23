// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_FUNCTION_CALL_INFO_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_FUNCTION_CALL_INFO_H_

#include "src/lib/fxl/memory/ref_ptr.h"

namespace zxdb {

class ExprValue;
class Function;

struct FunctionCallInfo {
  fxl::RefPtr<Function> fn;
  std::vector<ExprValue> parameters;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_FUNCTION_CALL_INFO_H_
