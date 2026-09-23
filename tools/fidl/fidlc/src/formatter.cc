// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/formatter.h"

#include "tools/fidl/fidlc/src/lexer.h"
#include "tools/fidl/fidlc/src/parser.h"
#include "tools/fidl/fidlc/src/span_sequence.h"
#include "tools/fidl/fidlc/src/span_sequence_tree_visitor.h"

namespace fidlc {

std::optional<std::string> Formatter::Format(const SourceFile& source_file,
                                             ExperimentalFlagSet experimental_flags) const {
  Lexer lexer(source_file, reporter_);
  Parser parser(&lexer, reporter_, experimental_flags);
  std::unique_ptr<File> ast = parser.Parse();
  if (parser.Success()) {
    return Print(std::move(ast), source_file.data().size());
  }
  return std::nullopt;
}

std::string Formatter::Print(std::unique_ptr<File> ast, size_t original_file_size) const {
  std::string out;
  auto visitor = SpanSequenceTreeVisitor(ast->tokens);
  visitor.OnFile(ast);
  MultilineSpanSequence result = visitor.Result();

  // A good guess for the size of the formatted file is just the size of the original, possibly
  // unformatted file.  Reserve that space now to minimize allocations as the printed string grows
  // down the road.
  out.reserve(original_file_size);
  result.Print(cols_, std::nullopt, 0, false, AdjacentIndents(false, false), &out);

  // Formatted files always have a trailing newline.
  if (!out.empty() && out.at(out.size() - 1) != '\n')
    return out + '\n';
  return out;
}
}  // namespace fidlc
