// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_FORMATTER_H_
#define TOOLS_FIDL_FIDLC_SRC_FORMATTER_H_

#include "tools/fidl/fidlc/src/experimental_flags.h"
#include "tools/fidl/fidlc/src/raw_ast.h"
#include "tools/fidl/fidlc/src/reporter.h"

namespace fidlc {

class Formatter final {
 public:
  Formatter(size_t cols, Reporter* reporter) : cols_(cols), reporter_(reporter) {}

  std::optional<std::string> Format(const SourceFile& source_file,
                                    ExperimentalFlagSet experimental_flags) const;

 private:
  std::string Print(std::unique_ptr<File> ast, size_t original_file_size) const;

  const size_t cols_;
  Reporter* reporter_;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_FORMATTER_H_
