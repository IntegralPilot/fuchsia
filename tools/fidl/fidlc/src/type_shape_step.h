// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_TYPE_SHAPE_STEP_H_
#define TOOLS_FIDL_FIDLC_SRC_TYPE_SHAPE_STEP_H_

#include "tools/fidl/fidlc/src/compiler.h"

namespace fidlc {

// Sets type_shape on all TypeDecls and Types, and field_shape on all struct members.
class TypeShapeStep : public Compiler::Step {
 public:
  using Step::Step;

 private:
  void RunImpl() override;

  // Compile and Calculate are mutually recursive. Compile calculates and (usually) stores
  // the type shape, while Calculate just returns it (but calls Compile recursively).
  TypeShape Compile(TypeDecl* decl, bool is_recursive_call = true);
  TypeShape Compile(Type* type, bool is_recursive_call = true);
  TypeShape Calculate(TypeDecl* decl);
  TypeShape Calculate(Type* type);

  template <typename TypeDeclOrType>
  TypeShape CompileImpl(TypeDeclOrType* target, bool is_recursive_call);
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_TYPE_SHAPE_STEP_H_
