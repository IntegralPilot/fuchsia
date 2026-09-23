// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_TYPE_RESOLVER_H_
#define TOOLS_FIDL_FIDLC_SRC_TYPE_RESOLVER_H_

#include "tools/fidl/fidlc/src/compile_step.h"
#include "tools/fidl/fidlc/src/flat_ast.h"

namespace fidlc {

class CompileStep;

// TypeResolver exposes Resolve* methods from CompileStep to Typespace and Type.
class TypeResolver {
 public:
  explicit TypeResolver(CompileStep* compile_step) : compile_step_(compile_step) {}

  Reporter* reporter() { return compile_step_->reporter(); }
  ExperimentalFlagSet experimental_flags() const { return compile_step_->experimental_flags(); }

  // Top level methods for resolving layout parameters. These are used by
  // TypeTemplates.
  bool ResolveParamAsType(const Reference& layout, const std::unique_ptr<LayoutParameter>& param,
                          bool compile_decls, Type** out_type);
  bool ResolveParamAsSize(const Reference& layout, const std::unique_ptr<LayoutParameter>& param,
                          const SizeValue** out_size);

  // These methods forward their implementation to the library_. They are used
  // by the top level methods above
  bool ResolveType(TypeConstructor* type, bool compile_decls);
  bool ResolveSizeBound(Constant* size_constant, const SizeValue** out_size);
  bool ResolveAsOptional(Constant* constant);
  bool ResolveAsHandleSubtype(Resource* resource, Constant* constant, HandleSubtype* out_obj_type);
  bool ResolveAsHandleRights(Resource* resource, Constant* constant,
                             const HandleRightsValue** out_rights);
  bool ResolveAsProtocol(const Constant* size_constant, const Protocol** out_decl);

  // Used in Typespace::Creator::CreateAliasType to recursively compile the
  // right-hand side.
  void CompileDecl(Decl* decl);

 private:
  CompileStep* compile_step_;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_TYPE_RESOLVER_H_
