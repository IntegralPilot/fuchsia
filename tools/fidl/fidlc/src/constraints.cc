// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/constraints.h"

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/reporter.h"
#include "tools/fidl/fidlc/src/type_resolver.h"

namespace fidlc {

////////// ConstraintBase
bool ConstraintStorageBase::ReportMergeFailure(Reporter* reporter, const Name& layout_name,
                                               const Constant* param) const {
  return reporter->Fail(ErrCannotConstrainTwice, param->span, layout_name);
}

////////// Constraint
template <ConstraintKind K>
void Constraint<K>::PopulateLayoutInvocation(LayoutInvocation* layout_invocation) {
  using S = ConstraintStorage<K>;
  if (S::kLayoutInvocationValue != nullptr) {
    layout_invocation->*S::kLayoutInvocationValue = *ValuePtr();
  }
  if (S::kLayoutInvocationRaw != nullptr) {
    layout_invocation->*S::kLayoutInvocationRaw = raw_constraint;
  }
}

////////// Handle Subtype
template class Constraint<ConstraintKind::kHandleSubtype>;
ConstraintStorage<ConstraintKind::kHandleSubtype>::ValueType LayoutInvocation::*
    ConstraintStorage<ConstraintKind::kHandleSubtype>::kLayoutInvocationValue =
        &LayoutInvocation::subtype_resolved;
const Constant* LayoutInvocation::*
    ConstraintStorage<ConstraintKind::kHandleSubtype>::kLayoutInvocationRaw =
        &LayoutInvocation::subtype_raw;

bool ConstraintStorage<ConstraintKind::kHandleSubtype>::ResolveConstraint(TypeResolver* resolver,
                                                                          Constant* param,
                                                                          Resource* resource) {
  return resolver->ResolveAsHandleSubtype(resource, param, &subtype);
}

////////// Handle Rights
template class Constraint<ConstraintKind::kHandleRights>;
const ConstraintStorage<ConstraintKind::kHandleRights>::ValueType
    ConstraintStorage<ConstraintKind::kHandleRights>::kDefault = &HandleType::kSameRights;
ConstraintStorage<ConstraintKind::kHandleRights>::ValueType LayoutInvocation::*
    ConstraintStorage<ConstraintKind::kHandleRights>::kLayoutInvocationValue =
        &LayoutInvocation::rights_resolved;
const Constant* LayoutInvocation::*
    ConstraintStorage<ConstraintKind::kHandleRights>::kLayoutInvocationRaw =
        &LayoutInvocation::rights_raw;

bool ConstraintStorage<ConstraintKind::kHandleRights>::ResolveConstraint(TypeResolver* resolver,
                                                                         Constant* param,
                                                                         Resource* resource) {
  return resolver->ResolveAsHandleRights(resource, param, &rights);
}

////////// Size
template class Constraint<ConstraintKind::kSize>;
const ConstraintStorage<ConstraintKind::kSize>::ValueType
    ConstraintStorage<ConstraintKind::kSize>::kDefault = nullptr;
ConstraintStorage<ConstraintKind::kSize>::ValueType LayoutInvocation::*
    ConstraintStorage<ConstraintKind::kSize>::kLayoutInvocationValue =
        &LayoutInvocation::size_resolved;
const Constant* LayoutInvocation::*ConstraintStorage<ConstraintKind::kSize>::kLayoutInvocationRaw =
    &LayoutInvocation::size_raw;

bool ConstraintStorage<ConstraintKind::kSize>::ResolveConstraint(TypeResolver* resolver,
                                                                 Constant* param,
                                                                 Resource* resource) {
  return resolver->ResolveSizeBound(param, &size);
}

bool ConstraintStorage<ConstraintKind::kSize>::ReportMergeFailure(Reporter* reporter,
                                                                  const Name& layout_name,
                                                                  const Constant* param) const {
  return reporter->Fail(ErrCannotBoundTwice, param->span, layout_name);
}

////////// Nullability
template class Constraint<ConstraintKind::kNullability>;
ConstraintStorage<ConstraintKind::kNullability>::ValueType LayoutInvocation::*
    ConstraintStorage<ConstraintKind::kNullability>::kLayoutInvocationValue =
        &LayoutInvocation::nullability;
const Constant* LayoutInvocation::*
    ConstraintStorage<ConstraintKind::kNullability>::kLayoutInvocationRaw = nullptr;

bool ConstraintStorage<ConstraintKind::kNullability>::ResolveConstraint(TypeResolver* resolver,
                                                                        Constant* param,
                                                                        Resource* resource) {
  if (resolver->ResolveAsOptional(param)) {
    nullability = ValueType::kNullable;
    return true;
  }
  return false;
}

bool ConstraintStorage<ConstraintKind::kNullability>::ReportMergeFailure(
    Reporter* reporter, const Name& layout_name, const Constant* param) const {
  return reporter->Fail(ErrCannotIndicateOptionalTwice, param->span, layout_name);
}

////////// Protocol
template class Constraint<ConstraintKind::kProtocol>;
ConstraintStorage<ConstraintKind::kProtocol>::ValueType LayoutInvocation::*
    ConstraintStorage<ConstraintKind::kProtocol>::kLayoutInvocationValue =
        &LayoutInvocation::protocol_decl;
const Constant* LayoutInvocation::*
    ConstraintStorage<ConstraintKind::kProtocol>::kLayoutInvocationRaw =
        &LayoutInvocation::protocol_decl_raw;

bool ConstraintStorage<ConstraintKind::kProtocol>::ResolveConstraint(TypeResolver* resolver,
                                                                     Constant* param,
                                                                     Resource* resource) {
  return resolver->ResolveAsProtocol(param, &protocol_decl);
}

////////// UTF8
template class Constraint<ConstraintKind::kUtf8>;
ConstraintStorage<ConstraintKind::kUtf8>::ValueType LayoutInvocation::*
    ConstraintStorage<ConstraintKind::kUtf8>::kLayoutInvocationValue = &LayoutInvocation::utf8;
const Constant* LayoutInvocation::*ConstraintStorage<ConstraintKind::kUtf8>::kLayoutInvocationRaw =
    nullptr;

bool ConstraintStorage<ConstraintKind::kUtf8>::ResolveConstraint(TypeResolver* resolver,
                                                                 Constant* param,
                                                                 Resource* resource) {
  // Never actually resolve it from the layout.
  return false;
}

bool ConstraintStorage<ConstraintKind::kUtf8>::ReportMergeFailure(Reporter* reporter,
                                                                  const Name& layout_name,
                                                                  const Constant* param) const {
  return reporter->Fail(ErrCannotBoundTwice, param->span, layout_name);
}

////////// ConstraintsBase
bool ConstraintsBase::OnUnexpectedConstraint(TypeResolver* resolver, Reporter* reporter,
                                             std::optional<SourceSpan> params_span,
                                             const Name& layout_name, Resource* resource,
                                             size_t num_constraints,
                                             const std::vector<std::unique_ptr<Constant>>& params,
                                             size_t param_index) const {
  ZX_ASSERT(params_span.has_value());
  if (params.size() > num_constraints) {
    return reporter->Fail(ErrTooManyConstraints, params_span.value(), layout_name, num_constraints,
                          params.size());
  }
  return reporter->Fail(ErrUnexpectedConstraint, params[param_index]->span, layout_name);
}

}  // namespace fidlc
