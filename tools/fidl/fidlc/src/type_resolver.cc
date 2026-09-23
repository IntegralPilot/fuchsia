// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/type_resolver.h"

#include <zircon/assert.h>

#include "tools/fidl/fidlc/src/compile_step.h"
#include "tools/fidl/fidlc/src/diagnostics.h"

namespace fidlc {

bool TypeResolver::ResolveParamAsType(const Reference& layout,
                                      const std::unique_ptr<LayoutParameter>& param,
                                      bool compile_decls, Type** out_type) {
  auto type_ctor = param->AsTypeCtor();
  auto check = reporter()->Checkpoint();
  if (!type_ctor || !ResolveType(type_ctor, compile_decls)) {
    // if there were no errors reported but we couldn't resolve to a type, it must
    // mean that the parameter referred to a non-type, so report a new error here.
    if (check.NoNewErrors()) {
      return reporter()->Fail(ErrExpectedType, param->span);
    }
    // otherwise, there was an error during the type resolution process, so we
    // should just report that rather than add an extra error here
    return false;
  }
  *out_type = type_ctor->type;
  return true;
}

bool TypeResolver::ResolveParamAsSize(const Reference& layout,
                                      const std::unique_ptr<LayoutParameter>& param,
                                      const SizeValue** out_size) {
  // We could use param->AsConstant() here, leading to code similar to ResolveParamAsType.
  // However, unlike ErrExpectedType, ErrExpectedValueButGotType requires a name to be
  // reported, which would require doing a switch on the parameter kind anyway to find
  // its Name. So we just handle all the cases ourselves from the start.
  switch (param->kind) {
    case LayoutParameter::Kind::kLiteral: {
      auto literal_param = static_cast<LiteralLayoutParameter*>(param.get());
      if (!ResolveSizeBound(literal_param->literal.get(), out_size))
        return reporter()->Fail(ErrCouldNotResolveSizeBound, literal_param->span);
      break;
    }
    case LayoutParameter::kType: {
      auto type_param = static_cast<TypeLayoutParameter*>(param.get());
      return reporter()->Fail(ErrExpectedValueButGotType, type_param->span,
                              type_param->type_ctor->layout.resolved().name());
    }
    case LayoutParameter::Kind::kIdentifier: {
      auto ambig_param = static_cast<IdentifierLayoutParameter*>(param.get());
      auto as_constant = ambig_param->AsConstant();
      if (!as_constant) {
        return reporter()->Fail(ErrExpectedValueButGotType, ambig_param->span,
                                ambig_param->reference.resolved().name());
      }
      if (!ResolveSizeBound(as_constant, out_size)) {
        return reporter()->Fail(ErrCannotResolveConstantValue, ambig_param->span);
      }
      break;
    }
  }
  ZX_ASSERT(*out_size);
  if ((*out_size)->value == 0)
    return reporter()->Fail(ErrMustHaveNonZeroSize, param->span, layout.resolved().name());
  return true;
}

bool TypeResolver::ResolveType(TypeConstructor* type, bool compile_decls) {
  compile_step_->CompileTypeConstructor(type, compile_decls);
  return type->type != nullptr;
}

bool TypeResolver::ResolveSizeBound(Constant* size_constant, const SizeValue** out_size) {
  return compile_step_->ResolveSizeBound(size_constant, out_size);
}

bool TypeResolver::ResolveAsOptional(Constant* constant) {
  return compile_step_->ResolveAsOptional(constant);
}

bool TypeResolver::ResolveAsHandleSubtype(Resource* resource, Constant* constant,
                                          HandleSubtype* out_obj_type) {
  return compile_step_->ResolveHandleSubtypeIdentifier(resource, constant, out_obj_type);
}

bool TypeResolver::ResolveAsHandleRights(Resource* resource, Constant* constant,
                                         const HandleRightsValue** out_rights) {
  return compile_step_->ResolveHandleRightsConstant(resource, constant, out_rights);
}

bool TypeResolver::ResolveAsProtocol(const Constant* constant, const Protocol** out_decl) {
  if (constant->kind != Constant::Kind::kIdentifier)
    return false;

  const auto* as_identifier = static_cast<const IdentifierConstant*>(constant);
  const auto* target = as_identifier->reference.resolved().element();
  if (target->kind != Element::Kind::kProtocol) {
    return false;
  }
  if (out_decl) {
    *out_decl = static_cast<const Protocol*>(target);
  }
  return true;
}

void TypeResolver::CompileDecl(Decl* decl) { compile_step_->CompileDecl(decl); }

}  // namespace fidlc
