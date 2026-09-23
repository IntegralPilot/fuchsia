// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_FORMAT_ASYNC_TASK_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_FORMAT_ASYNC_TASK_H_

#include "src/developer/debug/zxdb/format/async_output_buffer.h"
#include "src/developer/debug/zxdb/format/format.h"

namespace zxdb {

class AsyncTask;
class AsyncTaskTree;
class TargetSymbols;
class EvalContext;

struct FormatTaskOptions {
  bool verbose = false;
  // The amount of indentation given to child nodes.
  int indent_amount = 3;
  // The amount of indentation given to variables within a node.
  int variable_indent_amount = 2;
  FormatBufferOptions variable;
};

fxl::RefPtr<AsyncOutputBuffer> FormatAsyncTask(const AsyncTask& task, const TargetSymbols* symbols,
                                               const FormatTaskOptions& options,
                                               const fxl::RefPtr<EvalContext>& eval_context,
                                               int indent);

// Formats the given |AsyncTaskTree| in a tree-like view with appropriate indentation.
fxl::RefPtr<AsyncOutputBuffer> FormatAsyncTaskTree(const AsyncTaskTree& tree,
                                                   const TargetSymbols* symbols,
                                                   const FormatTaskOptions& options,
                                                   const fxl::RefPtr<EvalContext>& eval_context);

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_FORMAT_ASYNC_TASK_H_
