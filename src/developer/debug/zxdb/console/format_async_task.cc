// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/format_async_task.h"

#include "src/developer/debug/zxdb/client/async_task.h"
#include "src/developer/debug/zxdb/client/async_task_tree.h"
#include "src/developer/debug/zxdb/console/format_location.h"
#include "src/developer/debug/zxdb/format/format_name.h"
#include "src/developer/debug/zxdb/format/string_util.h"

namespace zxdb {

namespace {
constexpr std::string kAwaiteeMarker = "└─ ";
}  // namespace

fxl::RefPtr<AsyncOutputBuffer> FormatAsyncTask(const AsyncTask& task, const TargetSymbols* symbols,
                                               const FormatTaskOptions& options,
                                               const fxl::RefPtr<EvalContext>& eval_context,
                                               int indent) {
  auto out = fxl::MakeRefCounted<AsyncOutputBuffer>();

  if (indent >= options.indent_amount) {
    out->Append(std::string(indent - options.indent_amount, ' '));
    out->Append(kAwaiteeMarker);
  } else {
    out->Append(std::string(indent, ' '));
  }

  switch (task.GetType()) {
    case AsyncTask::Type::kFunction: {
      out->Append(FormatIdentifier(task.GetIdentifier(), {}));
      if (!task.GetState().empty()) {
        out->Append(" (");
        out->Append(Syntax::kComment, task.GetState());
        out->Append(")");
      }

      if (task.GetLocation().is_valid()) {
        out->Append(" " + GetBullet() + " ");
        out->Append(FormatFileLine(task.GetLocation().file_line(), symbols));
      }
      break;
    }
    case AsyncTask::Type::kScope:
      // The Scope's "identifier" is the literal word "Scope" these are bolded.
      out->Append(Syntax::kStringBold, task.GetIdentifier().GetFullNameNoQual());

      // Unnamed scopes are allowed.
      if (!task.GetState().empty()) {
        out->Append("(\"");
        out->Append(Syntax::kComment, task.GetState());
        out->Append("\")");
      }
      break;
    case AsyncTask::Type::kTask:
      [[fallthrough]];
    case AsyncTask::Type::kFuture:
      out->Append(FormatIdentifier(task.GetIdentifier(), {}));
      // These usually have some form of unique identifier associated with them.
      out->Append("(");
      out->Append(Syntax::kStringDim, task.GetState());
      out->Append(")");
      break;
    case AsyncTask::Type::kOther:
      // For types of tasks that we don't have other handlers for, we treat the "identifier" as a
      // literal string which doesn't go through the FormatIdentifier context..
      out->Append(task.GetIdentifier().GetFullNameNoQual());

      if (!task.GetState().empty()) {
        out->Append("(");
        out->Append(Syntax::kStringDim, task.GetState());
        out->Append(")");
      }
      break;
  }

  out->Append("\n");

  if (options.verbose) {
    for (const auto& value : task.GetValues()) {
      out->Append(std::string(indent + options.variable_indent_amount, ' '));
      out->Append(
          FormatValue(value.value, options.variable, eval_context, value.name ? *value.name : ""));
      out->Append("\n");
    }
  }

  out->Complete();
  return out;
}

fxl::RefPtr<AsyncOutputBuffer> FormatAsyncTaskTree(const AsyncTaskTree& tree,
                                                   const TargetSymbols* symbols,
                                                   const FormatTaskOptions& options,
                                                   const fxl::RefPtr<EvalContext>& eval_context) {
  auto out = fxl::MakeRefCounted<AsyncOutputBuffer>();
  if (!tree.has_tasks()) {
    out->Append(Syntax::kComment, "No async tasks found.\n");
    out->Complete();
    return out;
  }

  tree.ForEach([&, out](const AsyncTaskTree::TaskEntry& entry) {
    out->Append(FormatAsyncTask(entry.task, symbols, options, eval_context,
                                entry.depth * options.indent_amount));
  });

  out->Complete();
  return out;
}

}  // namespace zxdb
