// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/types.h"

#include <zircon/assert.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/type_resolver.h"

namespace fidlc {

// ZX_HANDLE_SAME_RIGHTS
const HandleRightsValue HandleType::kSameRights = HandleRightsValue(0x80000000);

bool RejectOptionalConstraints::OnUnexpectedConstraint(
    TypeResolver* resolver, Reporter* reporter, std::optional<SourceSpan> params_span,
    const Name& layout_name, Resource* resource, size_t num_constraints,
    const std::vector<std::unique_ptr<Constant>>& params, size_t param_index) const {
  if (params.size() == 1 && resolver->ResolveAsOptional(params[0].get())) {
    return reporter->Fail(ErrCannotBeOptional, params[0]->span, layout_name);
  }
  return ConstraintsBase::OnUnexpectedConstraint(resolver, reporter, params_span, layout_name,
                                                 resource, num_constraints, params, param_index);
}

bool ArrayConstraints::OnUnexpectedConstraint(TypeResolver* resolver, Reporter* reporter,
                                              std::optional<SourceSpan> params_span,
                                              const Name& layout_name, Resource* resource,
                                              size_t num_constraints,
                                              const std::vector<std::unique_ptr<Constant>>& params,
                                              size_t param_index) const {
  if (params.size() == 1 && resolver->ResolveAsOptional(params[0].get())) {
    return reporter->Fail(ErrCannotBeOptional, params[0]->span, layout_name);
  }
  return ConstraintsBase::OnUnexpectedConstraint(resolver, reporter, params_span, layout_name,
                                                 resource, num_constraints, params, param_index);
}

bool ArrayType::ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                                 const TypeConstraints& constraints, const Reference& layout,
                                 std::unique_ptr<Type>* out_type,
                                 LayoutInvocation* out_params) const {
  Constraints c;
  if (!ResolveAndMergeConstraints(resolver, reporter, constraints.span, layout.resolved().name(),
                                  nullptr, constraints.items, &c, out_params)) {
    return false;
  }

  if (c.utf8 && !resolver->experimental_flags().IsEnabled(ExperimentalFlag::kZxCTypes)) {
    return reporter->Fail(ErrExperimentalZxCTypesDisallowed, layout.span(),
                          layout.resolved().name());
  }

  *out_type = std::make_unique<ArrayType>(name, element_type, element_count, std::move(c));
  return true;
}

bool VectorConstraints::OnUnexpectedConstraint(TypeResolver* resolver, Reporter* reporter,
                                               std::optional<SourceSpan> params_span,
                                               const Name& layout_name, Resource* resource,
                                               size_t num_constraints,
                                               const std::vector<std::unique_ptr<Constant>>& params,
                                               size_t param_index) const {
  if (!params.empty() && param_index == 0) {
    return reporter->Fail(ErrCouldNotResolveSizeBound, params[0]->span);
  }
  return ConstraintsBase::OnUnexpectedConstraint(resolver, reporter, params_span, layout_name,
                                                 resource, num_constraints, params, param_index);
}

bool VectorType::ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                                  const TypeConstraints& constraints, const Reference& layout,
                                  std::unique_ptr<Type>* out_type,
                                  LayoutInvocation* out_params) const {
  Constraints c;
  if (!ResolveAndMergeConstraints(resolver, reporter, constraints.span, layout.resolved().name(),
                                  nullptr, constraints.items, &c, out_params)) {
    return false;
  }

  *out_type = std::make_unique<VectorType>(name, element_type, std::move(c));
  return true;
}

bool StringType::ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                                  const TypeConstraints& constraints, const Reference& layout,
                                  std::unique_ptr<Type>* out_type,
                                  LayoutInvocation* out_params) const {
  Constraints c;
  if (!ResolveAndMergeConstraints(resolver, reporter, constraints.span, layout.resolved().name(),
                                  nullptr, constraints.items, &c, out_params)) {
    return false;
  }

  *out_type = std::make_unique<StringType>(name, c);

  return true;
}

bool HandleType::ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                                  const TypeConstraints& constraints, const Reference& layout,
                                  std::unique_ptr<Type>* out_type,
                                  LayoutInvocation* out_params) const {
  ZX_ASSERT(resource_decl);

  Constraints c;
  if (!ResolveAndMergeConstraints(resolver, reporter, constraints.span, layout.resolved().name(),
                                  resource_decl, constraints.items, &c, out_params)) {
    return false;
  }

  *out_type = std::make_unique<HandleType>(name, resource_decl, std::move(c));
  return true;
}

bool TransportSideConstraints::OnUnexpectedConstraint(
    TypeResolver* resolver, Reporter* reporter, std::optional<SourceSpan> params_span,
    const Name& layout_name, Resource* resource, size_t num_constraints,
    const std::vector<std::unique_ptr<Constant>>& params, size_t param_index) const {
  if (!params.empty() && param_index == 0) {
    return reporter->Fail(ErrMustBeAProtocol, params[0]->span, layout_name);
  }
  return ConstraintsBase::OnUnexpectedConstraint(resolver, reporter, params_span, layout_name,
                                                 resource, num_constraints, params, param_index);
}

bool TransportSideType::ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                                         const TypeConstraints& constraints,
                                         const Reference& layout, std::unique_ptr<Type>* out_type,
                                         LayoutInvocation* out_params) const {
  Constraints c;
  if (!ResolveAndMergeConstraints(resolver, reporter, constraints.span, layout.resolved().name(),
                                  nullptr, constraints.items, &c, out_params)) {
    return false;
  }

  if (!c.HasConstraint<ConstraintKind::kProtocol>()) {
    return reporter->Fail(ErrProtocolConstraintRequired, layout.span(), layout.resolved().name());
  }

  const Attribute* transport_attribute = c.protocol_decl->attributes->Get("transport");
  std::string_view transport("Channel");
  if (transport_attribute) {
    auto arg = (transport_attribute->compiled)
                   ? transport_attribute->GetArg(AttributeArg::kDefaultAnonymousName)
                   : transport_attribute->GetStandaloneAnonymousArg();
    std::string_view quoted_transport =
        static_cast<const LiteralConstant*>(arg->value.get())->literal->span().data();
    // Remove quotes around the transport.
    transport = quoted_transport.substr(1, quoted_transport.size() - 2);
  }

  *out_type = std::make_unique<TransportSideType>(name, std::move(c), end, transport);
  return true;
}

IdentifierType::IdentifierType(TypeDecl* type_decl, Constraints constraints)
    : Type(type_decl->name, Kind::kIdentifier),
      Constraints(std::move(constraints)),
      type_decl(type_decl) {}

bool IdentifierType::ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                                      const TypeConstraints& constraints, const Reference& layout,
                                      std::unique_ptr<Type>* out_type,
                                      LayoutInvocation* out_params) const {
  const auto& layout_name = layout.resolved().name();

  if (type_decl->kind == Decl::Kind::kNewType && !constraints.items.empty()) {
    // Currently, we are disallowing optional on new-types. And since new-types are semi-opaque
    // wrappers around types, we need not bother about other constraints. So no constraints for
    // you, new-types!
    return reporter->Fail(ErrNewTypeCannotHaveConstraint, constraints.span.value(), layout_name);
  }

  Constraints c;
  if (!ResolveAndMergeConstraints(resolver, reporter, constraints.span, layout.resolved().name(),
                                  nullptr, constraints.items, &c, out_params)) {
    return false;
  }

  switch (type_decl->kind) {
    // These types have no allowed constraints
    case Decl::Kind::kBits:
    case Decl::Kind::kEnum:
    case Decl::Kind::kTable:
    case Decl::Kind::kOverlay:
      if (c.HasConstraint<ConstraintKind::kNullability>()) {
        return reporter->Fail(ErrCannotBeOptional, constraints.span.value(), layout_name);
      }
      break;

    case Decl::Kind::kStruct:
      if (c.HasConstraint<ConstraintKind::kNullability>()) {
        return reporter->Fail(ErrStructCannotBeOptional, constraints.span.value(), layout_name);
      }
      break;

    // These types have one allowed constraint (`optional`). For aliases,
    // we need to allow the possibility that the concrete type does allow `optional`,
    // if it doesn't the Type itself will catch the error.
    case Decl::Kind::kAlias:
    case Decl::Kind::kUnion:
      break;

    case Decl::Kind::kNewType:
      // Constraints on new-types should be handled above.
      ZX_ASSERT(constraints.items.empty());
      break;

    case Decl::Kind::kBuiltin:
    case Decl::Kind::kConst:
    case Decl::Kind::kResource:
      // Cannot have const: entries for constants do not exist in the typespace, so
      // they're caught earlier.
      // Cannot have resource: resource types should have resolved to the HandleTypeTemplate
      ZX_PANIC("unexpected identifier type decl kind");
    // These can't be used as types. This will be caught later, in VerifyTypeCategory.
    case Decl::Kind::kService:
    case Decl::Kind::kProtocol:
      break;
  }

  *out_type = std::make_unique<IdentifierType>(type_decl, std::move(c));
  return true;
}

bool BoxConstraints::OnUnexpectedConstraint(TypeResolver* resolver, Reporter* reporter,
                                            std::optional<SourceSpan> params_span,
                                            const Name& layout_name, Resource* resource,
                                            size_t num_constraints,
                                            const std::vector<std::unique_ptr<Constant>>& params,
                                            size_t param_index) const {
  if (params.size() == 1 && resolver->ResolveAsOptional(params[0].get())) {
    return reporter->Fail(ErrBoxCannotBeOptional, params[0]->span);
  }
  return ConstraintsBase::OnUnexpectedConstraint(resolver, reporter, params_span, layout_name,
                                                 resource, num_constraints, params, param_index);
}
bool BoxType::ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                               const TypeConstraints& constraints, const Reference& layout,
                               std::unique_ptr<Type>* out_type,
                               LayoutInvocation* out_params) const {
  if (!ResolveAndMergeConstraints(resolver, reporter, constraints.span, layout.resolved().name(),
                                  nullptr, constraints.items, nullptr)) {
    return false;
  }

  *out_type = std::make_unique<BoxType>(name, boxed_type);
  return true;
}

bool UntypedNumericType::ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                                          const TypeConstraints& constraints,
                                          const Reference& layout, std::unique_ptr<Type>* out_type,
                                          LayoutInvocation* out_params) const {
  ZX_PANIC("should not have untyped numeric here");
}

uint32_t PrimitiveType::SubtypeSize(PrimitiveSubtype subtype) {
  switch (subtype) {
    case PrimitiveSubtype::kBool:
    case PrimitiveSubtype::kInt8:
    case PrimitiveSubtype::kUint8:
    case PrimitiveSubtype::kZxUchar:
      return 1u;

    case PrimitiveSubtype::kInt16:
    case PrimitiveSubtype::kUint16:
      return 2u;

    case PrimitiveSubtype::kFloat32:
    case PrimitiveSubtype::kInt32:
    case PrimitiveSubtype::kUint32:
      return 4u;

    case PrimitiveSubtype::kFloat64:
    case PrimitiveSubtype::kInt64:
    case PrimitiveSubtype::kUint64:
    case PrimitiveSubtype::kZxUsize64:
    case PrimitiveSubtype::kZxUintptr64:
      return 8u;
  }
}

bool PrimitiveType::ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                                     const TypeConstraints& constraints, const Reference& layout,
                                     std::unique_ptr<Type>* out_type,
                                     LayoutInvocation* out_params) const {
  if (!ResolveAndMergeConstraints(resolver, reporter, constraints.span, layout.resolved().name(),
                                  nullptr, constraints.items, nullptr)) {
    return false;
  }

  if ((subtype == PrimitiveSubtype::kZxUsize64 || subtype == PrimitiveSubtype::kZxUintptr64 ||
       subtype == PrimitiveSubtype::kZxUchar) &&
      !resolver->experimental_flags().IsEnabled(ExperimentalFlag::kZxCTypes)) {
    return reporter->Fail(ErrExperimentalZxCTypesDisallowed, layout.span(),
                          layout.resolved().name());
  }
  *out_type = std::make_unique<PrimitiveType>(name, subtype);
  return true;
}

bool InternalType::ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                                    const TypeConstraints& constraints, const Reference& layout,
                                    std::unique_ptr<Type>* out_type,
                                    LayoutInvocation* out_params) const {
  if (!ResolveAndMergeConstraints(resolver, reporter, constraints.span, layout.resolved().name(),
                                  nullptr, constraints.items, nullptr)) {
    return false;
  }
  *out_type = std::make_unique<InternalType>(name, subtype);
  return true;
}

bool ZxExperimentalPointerType::ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                                                 const TypeConstraints& constraints,
                                                 const Reference& layout,
                                                 std::unique_ptr<Type>* out_type,
                                                 LayoutInvocation* out_params) const {
  if (!ResolveAndMergeConstraints(resolver, reporter, constraints.span, layout.resolved().name(),
                                  nullptr, constraints.items, nullptr)) {
    return false;
  }
  if (!resolver->experimental_flags().IsEnabled(ExperimentalFlag::kZxCTypes)) {
    return reporter->Fail(ErrExperimentalZxCTypesDisallowed, layout.span(),
                          layout.resolved().name());
  }
  *out_type = std::make_unique<ZxExperimentalPointerType>(name, pointee_type);
  return true;
}

Resourceness Type::Resourceness() const {
  switch (this->kind) {
    case Type::Kind::kPrimitive:
    case Type::Kind::kInternal:
    case Type::Kind::kString:
      return Resourceness::kValue;
    case Type::Kind::kHandle:
    case Type::Kind::kTransportSide:
      return Resourceness::kResource;
    case Type::Kind::kArray:
      return static_cast<const ArrayType*>(this)->element_type->Resourceness();
    case Type::Kind::kVector:
      return static_cast<const VectorType*>(this)->element_type->Resourceness();
    case Type::Kind::kZxExperimentalPointer:
      return static_cast<const ZxExperimentalPointerType*>(this)->pointee_type->Resourceness();
    case Type::Kind::kIdentifier:
      break;
    case Type::Kind::kBox:
      return static_cast<const BoxType*>(this)->boxed_type->Resourceness();
    case Type::Kind::kUntypedNumeric:
      ZX_PANIC("should not have untyped numeric here");
  }

  const auto* decl = static_cast<const IdentifierType*>(this)->type_decl;
  switch (decl->kind) {
    case Decl::Kind::kBits:
    case Decl::Kind::kEnum:
      return Resourceness::kValue;
    case Decl::Kind::kProtocol:
      return Resourceness::kResource;
    case Decl::Kind::kStruct:
      return static_cast<const Struct*>(decl)->resourceness.value();
    case Decl::Kind::kTable:
      return static_cast<const Table*>(decl)->resourceness.value();
    case Decl::Kind::kUnion:
      return static_cast<const Union*>(decl)->resourceness.value();
    case Decl::Kind::kOverlay:
      return static_cast<const Overlay*>(decl)->resourceness.value();
    case Decl::Kind::kNewType:
      return static_cast<const NewType*>(decl)->type_ctor->type->Resourceness();
    case Decl::Kind::kBuiltin:
    case Decl::Kind::kConst:
    case Decl::Kind::kResource:
    case Decl::Kind::kService:
    case Decl::Kind::kAlias:
      ZX_PANIC("unexpected kind");
  }
}

}  // namespace fidlc
