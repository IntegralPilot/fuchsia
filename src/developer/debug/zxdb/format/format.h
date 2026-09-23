// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_FORMAT_FORMAT_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_FORMAT_FORMAT_H_

#include "lib/fit/defer.h"
#include "src/developer/debug/zxdb/expr/eval_context.h"
#include "src/developer/debug/zxdb/expr/format_options.h"
#include "src/developer/debug/zxdb/format/async_output_buffer.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"

namespace zxdb {

class ExprValue;
class FormatNode;
class Variable;

// Output-specific options for value formatting to buffer.
struct FormatBufferOptions : public FormatOptions {
  // This has numeric values so one can compare verbosity levels.
  enum class Verbosity : int {
    // Show as little as possible without being misleading. Some long types will be elided with
    // "..." and other things may be minimized.
    kMinimal = 0,

    // Show the full names of base classes.
    kMedium = 1,

    // All full type information and pointer values are shown for everything.
    kAllTypes = 2
  };
  Verbosity verbosity = Verbosity::kMedium;

  enum class Wrapping {
    kNone,      // No linebreaks or whitespace will be inserted.
    kExpanded,  // Every member will be on a separate line and indented.
    kSmart      // Use single-line if it fits in smart_indent_cols, multiline otherwise.
  };
  Wrapping wrapping = Wrapping::kNone;
  int indent_amount = 2;       // Number of spaces to indent when using expanded formatting.
  int smart_indent_cols = 80;  // Wrapping threshold for "kSmart" wrapping mode.

  // The number of pointers to resolve to values recursively.
  //
  // When we encounter pointers, we can't blindly follow and expand them because there can be
  // cycles and this will put us into an infinite loop.
  //
  // This number tracks the number of nested pointers that the code will resolve to the pointed-to
  // values. A value of 0 does not expand pointers and will only print their address. A value of two
  // would print up to two nested levels of pointer. These need not be consecutive in the hierarchy:
  // there could be a pointer, then a bunch of levels of concrete struct nesting, then another
  // pointer and this would count toward the two.
  int pointer_expand_depth = 1;

  // An upper bound on the level of nesting that we'll do. This prevents the presentation from
  // getting too crazy and also protects against infinite recursion in some error cases.
  int max_depth = 16;
};

// Recursively describes the given format node according to the given settings. Executes the given
// callback on completion or if the node is destroyed before formatting is done.
void DescribeFormatTreeNode(FormatNode* node, const FormatBufferOptions& options,
                            fxl::RefPtr<EvalContext> context, fit::deferred_callback cb);

// Formats the given FormatNode. The string will not be followed by a newline.
//
// This assumes the node has been evaluated and described as desired by the caller so the result
// can be synchronously formatted and returned.
OutputBuffer FormatTreeNode(const FormatNode& node, const FormatBufferOptions& options);

// Describes and formats the given ExprValue and returns it as an async output buffer. The result
// will not be followed by a newline.
//
// If the value_name is given, it will be printed with that name, otherwise it will have no name.
fxl::RefPtr<AsyncOutputBuffer> FormatValue(ExprValue value, const FormatBufferOptions& options,
                                           fxl::RefPtr<EvalContext> eval_context,
                                           const std::string& value_name = std::string());

// Like FormatValue but evaluates the given variable in the given context to get the
// result. The name of the variable will be included.
fxl::RefPtr<AsyncOutputBuffer> FormatVariable(const Variable* var,
                                              const FormatBufferOptions& options,
                                              fxl::RefPtr<EvalContext> eval_context);

// Outputs all of the given expressions. The expressions themselves will be displayed as the
// "variable name" of each resulting value.
fxl::RefPtr<AsyncOutputBuffer> FormatExpressions(const std::vector<std::string>& expressions,
                                                 const FormatBufferOptions& options,
                                                 fxl::RefPtr<EvalContext> eval_context);

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_FORMAT_FORMAT_H_
