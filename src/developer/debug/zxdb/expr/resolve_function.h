// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_EXPR_RESOLVE_FUNCTION_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_EXPR_RESOLVE_FUNCTION_H_

#include "src/developer/debug/zxdb/expr/eval_callback.h"
#include "src/developer/debug/zxdb/expr/expr_value.h"
#include "src/developer/debug/zxdb/expr/parsed_identifier.h"
#include "src/lib/fxl/memory/ref_ptr.h"

namespace zxdb {

struct FunctionCallInfo;

// Tries to resolve and extract the given function name to a Function symbol.
//
// There are several cases that currently return an error, but will be
// implemented eventually:
//  * |fn_name| resolves to multiple locations
//  * |params| is not empty
// These features are being tracked in https://fxbug.dev/42132103.
//
// An error is also returned in the case where |fn_name| resolves to no
// locations or the function has been inlined.
void ResolveFunction(const fxl::RefPtr<EvalContext>& eval_context, const ParsedIdentifier& fn_name,
                     const std::vector<ExprValue>& params,
                     fit::callback<void(ErrOr<FunctionCallInfo>)> cb);
}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_EXPR_RESOLVE_FUNCTION_H_
