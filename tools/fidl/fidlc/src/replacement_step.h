// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_REPLACEMENT_STEP_H_
#define TOOLS_FIDL_FIDLC_SRC_REPLACEMENT_STEP_H_

#include "tools/fidl/fidlc/src/compiler.h"

namespace fidlc {

// Verifies that @available arguments `removed` and `replaced` are used correctly.
class ReplacementStep : public Compiler::Step {
 public:
  using Step::Step;

 private:
  void RunImpl() override;
  void CheckDecls();
  void CheckMembers();
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_REPLACEMENT_STEP_H_
