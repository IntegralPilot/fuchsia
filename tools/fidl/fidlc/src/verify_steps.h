// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_VERIFY_STEPS_H_
#define TOOLS_FIDL_FIDLC_SRC_VERIFY_STEPS_H_

#include "tools/fidl/fidlc/src/compiler.h"

namespace fidlc {

// Checks that value types do not contain resource types.
class VerifyResourcenessStep : public Compiler::Step {
 public:
  using Step::Step;

 private:
  void RunImpl() override;

  template <typename DeclType>
  void ValidateResourceness(const DeclType* decl);
};

// Ensures that protocols don't use endpoints of incompatible transports.
class VerifyHandleTransportStep : public Compiler::Step {
 public:
  using Step::Step;

 private:
  void RunImpl() override;
};

// Runs validation functions from attribute schemas.
class VerifyAttributesStep : public Compiler::Step {
 public:
  using Step::Step;

 private:
  void RunImpl() override;
};

// Ensures that all dependencies were used.
class VerifyDependenciesStep : public Compiler::Step {
 public:
  using Step::Step;

 private:
  void RunImpl() override;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_VERIFY_STEPS_H_
