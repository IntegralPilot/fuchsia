// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/typespace.h"

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/type_resolver.h"

namespace fidlc {

static std::optional<PrimitiveSubtype> BuiltinToPrimitiveSubtype(Builtin::Identity id) {
  switch (id) {
    case Builtin::Identity::kBool:
      return PrimitiveSubtype::kBool;
    case Builtin::Identity::kInt8:
      return PrimitiveSubtype::kInt8;
    case Builtin::Identity::kInt16:
      return PrimitiveSubtype::kInt16;
    case Builtin::Identity::kInt32:
      return PrimitiveSubtype::kInt32;
    case Builtin::Identity::kInt64:
      return PrimitiveSubtype::kInt64;
    case Builtin::Identity::kUint8:
      return PrimitiveSubtype::kUint8;
    case Builtin::Identity::kZxUchar:
      return PrimitiveSubtype::kZxUchar;
    case Builtin::Identity::kUint16:
      return PrimitiveSubtype::kUint16;
    case Builtin::Identity::kUint32:
      return PrimitiveSubtype::kUint32;
    case Builtin::Identity::kUint64:
      return PrimitiveSubtype::kUint64;
    case Builtin::Identity::kZxUsize64:
      return PrimitiveSubtype::kZxUsize64;
    case Builtin::Identity::kZxUintptr64:
      return PrimitiveSubtype::kZxUintptr64;
    case Builtin::Identity::kFloat32:
      return PrimitiveSubtype::kFloat32;
    case Builtin::Identity::kFloat64:
      return PrimitiveSubtype::kFloat64;
    default:
      return std::nullopt;
  }
}

static std::optional<InternalSubtype> BuiltinToInternalSubtype(Builtin::Identity id) {
  switch (id) {
    case Builtin::Identity::kFrameworkErr:
      return InternalSubtype::kFrameworkErr;
    default:
      return std::nullopt;
  }
}

Typespace::Typespace(const Library* root_library, Reporter* reporter) : reporter_(reporter) {
  for (auto& builtin : root_library->declarations.builtins) {
    if (auto subtype = BuiltinToPrimitiveSubtype(builtin->id)) {
      primitive_types_.emplace(subtype.value(),
                               std::make_unique<PrimitiveType>(builtin->name, subtype.value()));
    } else if (auto subtype = BuiltinToInternalSubtype(builtin->id)) {
      internal_types_.emplace(subtype.value(),
                              std::make_unique<InternalType>(builtin->name, subtype.value()));
    } else if (builtin->id == Builtin::Identity::kString) {
      unbounded_string_type_ =
          std::make_unique<StringType>(builtin->name, StringType::Constraints());
    } else if (builtin->id == Builtin::Identity::kVector) {
      vector_layout_name_ = builtin->name;
    } else if (builtin->id == Builtin::Identity::kZxExperimentalPointer) {
      pointer_type_name_ = builtin->name;
    }
  }
  untyped_numeric_type_ =
      std::make_unique<UntypedNumericType>(Name::CreateIntrinsic(nullptr, "untyped numeric"));
}

PrimitiveType* Typespace::GetPrimitiveType(PrimitiveSubtype subtype) {
  return primitive_types_.at(subtype).get();
}

InternalType* Typespace::GetInternalType(InternalSubtype subtype) {
  return internal_types_.at(subtype).get();
}

Type* Typespace::GetUnboundedStringType() { return unbounded_string_type_.get(); }

Type* Typespace::GetStringType(size_t max_size) {
  auto name = unbounded_string_type_->name;
  sizes_.push_back(std::make_unique<SizeValue>(max_size));
  auto size = sizes_.back().get();
  types_.push_back(
      std::make_unique<StringType>(name, StringType::Constraints(size, Nullability::kNonnullable)));
  return types_.back().get();
}

Type* Typespace::GetUntypedNumericType() { return untyped_numeric_type_.get(); }

Type* Typespace::Intern(std::unique_ptr<Type> type) {
  types_.push_back(std::move(type));
  return types_.back().get();
}

class Typespace::Creator {
 public:
  Creator(Typespace* typespace, TypeResolver* resolver, const Reference& layout,
          const LayoutParameterList& parameters, const TypeConstraints& constraints,
          bool compile_decls, LayoutInvocation* out_params)
      : typespace_(typespace),
        resolver_(resolver),
        layout_(layout),
        parameters_(parameters),
        constraints_(constraints),
        compile_decls_(compile_decls),
        out_params_(out_params) {}

  Type* Create();

 private:
  Reporter* reporter() { return typespace_->reporter(); }

  bool EnsureNumberOfLayoutParams(size_t expected_params);

  Type* CreatePrimitiveType(PrimitiveSubtype subtype);
  Type* CreateInternalType(InternalSubtype subtype);
  Type* CreateStringType();
  Type* CreateArrayType();
  Type* CreateStringArrayType();
  Type* CreateVectorType();
  Type* CreateBytesType();
  Type* CreateFrameworkErrType();
  Type* CreateBoxType();
  Type* CreateHandleType(Resource* resource);
  Type* CreateTransportSideType(TransportSide end);
  Type* CreateIdentifierType(TypeDecl* type_decl);
  Type* CreateAliasType(Alias* alias);
  Type* CreateZxExperimentalPointerType();

  Typespace* typespace_;
  TypeResolver* resolver_;
  const Reference& layout_;
  const LayoutParameterList& parameters_;
  const TypeConstraints& constraints_;
  bool compile_decls_;
  LayoutInvocation* out_params_;
};

Type* Typespace::Create(TypeResolver* resolver, const Reference& layout,
                        const LayoutParameterList& parameters, const TypeConstraints& constraints,
                        bool compile_decls, LayoutInvocation* out_params) {
  return Creator(this, resolver, layout, parameters, constraints, compile_decls, out_params)
      .Create();
}

Type* Typespace::Creator::Create() {
  Decl* target = layout_.resolved().element()->AsDecl();

  switch (target->kind) {
    case Decl::Kind::kBits:
    case Decl::Kind::kEnum:
    case Decl::Kind::kNewType:
    case Decl::Kind::kStruct:
    case Decl::Kind::kTable:
    case Decl::Kind::kUnion:
    case Decl::Kind::kOverlay:
      return CreateIdentifierType(static_cast<TypeDecl*>(target));
    case Decl::Kind::kResource:
      return CreateHandleType(static_cast<Resource*>(target));
    case Decl::Kind::kAlias:
      return CreateAliasType(static_cast<Alias*>(target));
    case Decl::Kind::kBuiltin:
      // Handled below.
      break;
    case Decl::Kind::kConst:
    case Decl::Kind::kProtocol:
    case Decl::Kind::kService:
      reporter()->Fail(ErrExpectedType, layout_.span());
      return nullptr;
  }

  auto builtin = static_cast<const Builtin*>(target);
  switch (builtin->id) {
    case Builtin::Identity::kBool:
    case Builtin::Identity::kInt8:
    case Builtin::Identity::kInt16:
    case Builtin::Identity::kInt32:
    case Builtin::Identity::kInt64:
    case Builtin::Identity::kUint8:
    case Builtin::Identity::kZxUchar:
    case Builtin::Identity::kUint16:
    case Builtin::Identity::kUint32:
    case Builtin::Identity::kUint64:
    case Builtin::Identity::kZxUsize64:
    case Builtin::Identity::kZxUintptr64:
    case Builtin::Identity::kFloat32:
    case Builtin::Identity::kFloat64:
      return CreatePrimitiveType(BuiltinToPrimitiveSubtype(builtin->id).value());
    case Builtin::Identity::kString:
      return CreateStringType();
    case Builtin::Identity::kBox:
      return CreateBoxType();
    case Builtin::Identity::kArray:
      return CreateArrayType();
    case Builtin::Identity::kStringArray:
      return CreateStringArrayType();
    case Builtin::Identity::kVector:
      return CreateVectorType();
    case Builtin::Identity::kZxExperimentalPointer:
      return CreateZxExperimentalPointerType();
    case Builtin::Identity::kClientEnd:
      return CreateTransportSideType(TransportSide::kClient);
    case Builtin::Identity::kServerEnd:
      return CreateTransportSideType(TransportSide::kServer);
    case Builtin::Identity::kByte:
      return CreatePrimitiveType(PrimitiveSubtype::kUint8);
    case Builtin::Identity::kFrameworkErr:
      return CreateInternalType(BuiltinToInternalSubtype(builtin->id).value());
    case Builtin::Identity::kOptional:
    case Builtin::Identity::kMax:
    case Builtin::Identity::kNext:
    case Builtin::Identity::kHead:
      reporter()->Fail(ErrExpectedType, layout_.span());
      return nullptr;
  }
}

bool Typespace::Creator::EnsureNumberOfLayoutParams(size_t expected_params) {
  auto num_params = parameters_.items.size();
  if (num_params == expected_params) {
    return true;
  }
  auto span = num_params == 0 ? layout_.span() : parameters_.span.value();
  return reporter()->Fail(ErrWrongNumberOfLayoutParameters, span, layout_.resolved().name(),
                          expected_params, num_params);
}

Type* Typespace::Creator::CreatePrimitiveType(PrimitiveSubtype subtype) {
  if (!EnsureNumberOfLayoutParams(0)) {
    return nullptr;
  }
  std::unique_ptr<Type> constrained_type;
  typespace_->GetPrimitiveType(subtype)->ApplyConstraints(resolver_, reporter(), constraints_,
                                                          layout_, &constrained_type, out_params_);
  return typespace_->Intern(std::move(constrained_type));
}

Type* Typespace::Creator::CreateInternalType(InternalSubtype subtype) {
  if (!EnsureNumberOfLayoutParams(0)) {
    return nullptr;
  }
  std::unique_ptr<Type> constrained_type;
  typespace_->GetInternalType(subtype)->ApplyConstraints(resolver_, reporter(), constraints_,
                                                         layout_, &constrained_type, out_params_);
  return typespace_->Intern(std::move(constrained_type));
}

Type* Typespace::Creator::CreateArrayType() {
  if (!EnsureNumberOfLayoutParams(2))
    return nullptr;

  Type* element_type = nullptr;
  if (!resolver_->ResolveParamAsType(layout_, parameters_.items[0], compile_decls_, &element_type))
    return nullptr;
  out_params_->element_type_resolved = element_type;
  out_params_->element_type_raw = parameters_.items[0]->AsTypeCtor();

  const SizeValue* size = nullptr;
  if (!resolver_->ResolveParamAsSize(layout_, parameters_.items[1], &size))
    return nullptr;
  out_params_->size_resolved = size;
  out_params_->size_raw = parameters_.items[1]->AsConstant();

  ArrayType type(layout_.resolved().name(), element_type, size);
  std::unique_ptr<Type> constrained_type;
  type.ApplyConstraints(resolver_, reporter(), constraints_, layout_, &constrained_type,
                        out_params_);
  return typespace_->Intern(std::move(constrained_type));
}

Type* Typespace::Creator::CreateStringArrayType() {
  if (!EnsureNumberOfLayoutParams(1)) {
    return nullptr;
  }

  Type* uint8_type = typespace_->GetPrimitiveType(PrimitiveSubtype::kUint8);

  const SizeValue* size = nullptr;
  if (!resolver_->ResolveParamAsSize(layout_, parameters_.items[0], &size))
    return nullptr;
  out_params_->size_resolved = size;
  out_params_->size_raw = parameters_.items[0]->AsConstant();

  ArrayType type(layout_.resolved().name(), uint8_type, size);
  type.utf8 = true;
  std::unique_ptr<Type> constrained_type;
  type.ApplyConstraints(resolver_, reporter(), constraints_, layout_, &constrained_type,
                        out_params_);
  return typespace_->Intern(std::move(constrained_type));
}

Type* Typespace::Creator::CreateVectorType() {
  if (!EnsureNumberOfLayoutParams(1))
    return nullptr;

  // Check for `optional`, since vector<T>:optional types are cycle-breaking.
  // TODO(https://fxbug.dev/42153849): As part of refactoring Typespace we could
  // resolve all parameters and constraints before creating the Type.
  for (auto& constraint : constraints_.items) {
    if (resolver_->ResolveAsOptional(constraint.get())) {
      compile_decls_ = false;
      break;
    }
  }
  Type* element_type = nullptr;
  if (!resolver_->ResolveParamAsType(layout_, parameters_.items[0], compile_decls_, &element_type))
    return nullptr;
  out_params_->element_type_resolved = element_type;
  out_params_->element_type_raw = parameters_.items[0]->AsTypeCtor();

  VectorType type(layout_.resolved().name(), element_type);
  std::unique_ptr<Type> constrained_type;
  type.ApplyConstraints(resolver_, reporter(), constraints_, layout_, &constrained_type,
                        out_params_);
  return typespace_->Intern(std::move(constrained_type));
}

Type* Typespace::Creator::CreateZxExperimentalPointerType() {
  if (!EnsureNumberOfLayoutParams(1))
    return nullptr;

  Type* element_type = nullptr;
  if (!resolver_->ResolveParamAsType(layout_, parameters_.items[0], compile_decls_, &element_type))
    return nullptr;
  out_params_->element_type_resolved = element_type;
  out_params_->element_type_raw = parameters_.items[0]->AsTypeCtor();

  ZxExperimentalPointerType type(layout_.resolved().name(), element_type);
  std::unique_ptr<Type> constrained_type;
  type.ApplyConstraints(resolver_, reporter(), constraints_, layout_, &constrained_type,
                        out_params_);
  return typespace_->Intern(std::move(constrained_type));
}

Type* Typespace::Creator::CreateStringType() {
  if (!EnsureNumberOfLayoutParams(0))
    return nullptr;

  StringType type(layout_.resolved().name());
  std::unique_ptr<Type> constrained_type;
  type.ApplyConstraints(resolver_, reporter(), constraints_, layout_, &constrained_type,
                        out_params_);
  return typespace_->Intern(std::move(constrained_type));
}

Type* Typespace::Creator::CreateHandleType(Resource* resource) {
  if (!EnsureNumberOfLayoutParams(0))
    return nullptr;
  resolver_->CompileDecl(resource);

  HandleType type(layout_.resolved().name(), resource);
  std::unique_ptr<Type> constrained_type;
  type.ApplyConstraints(resolver_, reporter(), constraints_, layout_, &constrained_type,
                        out_params_);
  return typespace_->Intern(std::move(constrained_type));
}

// TODO(https://fxbug.dev/42134495): Support more transports.
static constexpr std::string_view kChannelTransport = "Channel";

Type* Typespace::Creator::CreateTransportSideType(TransportSide end) {
  if (!EnsureNumberOfLayoutParams(0))
    return nullptr;

  TransportSideType type(layout_.resolved().name(), end, kChannelTransport);
  std::unique_ptr<Type> constrained_type;
  type.ApplyConstraints(resolver_, reporter(), constraints_, layout_, &constrained_type,
                        out_params_);
  return typespace_->Intern(std::move(constrained_type));
}

Type* Typespace::Creator::CreateIdentifierType(TypeDecl* type_decl) {
  if (!EnsureNumberOfLayoutParams(0))
    return nullptr;
  IdentifierType type(type_decl);
  std::unique_ptr<Type> constrained_type;
  type.ApplyConstraints(resolver_, reporter(), constraints_, layout_, &constrained_type,
                        out_params_);
  if (compile_decls_ && constrained_type && !constrained_type->IsNullable())
    resolver_->CompileDecl(type_decl);
  return typespace_->Intern(std::move(constrained_type));
}

Type* Typespace::Creator::CreateAliasType(Alias* alias) {
  if (!EnsureNumberOfLayoutParams(0))
    return nullptr;
  resolver_->CompileDecl(alias);
  if (!alias->partial_type_ctor->type)
    return nullptr;
  const auto& aliased_type = alias->partial_type_ctor->type;
  out_params_->from_alias = alias;
  std::unique_ptr<Type> constrained_type;
  aliased_type->ApplyConstraints(resolver_, reporter(), constraints_, layout_, &constrained_type,
                                 out_params_);
  return typespace_->Intern(std::move(constrained_type));
}

static bool IsStruct(const Type* type) {
  if (type->kind != Type::Kind::kIdentifier) {
    return false;
  }
  return static_cast<const IdentifierType*>(type)->type_decl->kind == Decl::Kind::kStruct;
}

static bool CannotBeBoxedNorOptional(const Type* type) {
  if (type->kind == Type::Kind::kArray || type->kind == Type::Kind::kBox ||
      type->kind == Type::Kind::kPrimitive || type->kind == Type::Kind::kUntypedNumeric) {
    return true;
  }
  if (type->kind == Type::Kind::kIdentifier) {
    Decl::Kind decl_kind = static_cast<const IdentifierType*>(type)->type_decl->kind;
    if (decl_kind == Decl::Kind::kEnum || decl_kind == Decl::Kind::kBits ||
        decl_kind == Decl::Kind::kTable || decl_kind == Decl::Kind::kBuiltin) {
      return true;
    }
  }
  return false;
}

Type* Typespace::Creator::CreateBoxType() {
  if (!EnsureNumberOfLayoutParams(1))
    return nullptr;

  // box<T> types are cycle-breaking.
  compile_decls_ = false;
  Type* boxed_type = nullptr;
  if (!resolver_->ResolveParamAsType(layout_, parameters_.items[0], compile_decls_, &boxed_type))
    return nullptr;
  if (!IsStruct(boxed_type)) {
    if (boxed_type) {
      if (CannotBeBoxedNorOptional(boxed_type)) {
        reporter()->Fail(ErrCannotBeBoxedNorOptional, parameters_.items[0]->span, boxed_type->name);
      } else {
        reporter()->Fail(ErrCannotBeBoxedShouldBeOptional, parameters_.items[0]->span,
                         boxed_type->name);
      }
    }
    return nullptr;
  }
  const auto* inner = static_cast<const IdentifierType*>(boxed_type);
  ZX_ASSERT_MSG(inner->nullability == Nullability::kNonnullable,
                "the inner type must be non-nullable because it is a struct");
  // We disallow specifying the boxed type as nullable in FIDL source but
  // then mark the boxed type as nullable, so that internally it shares the
  // same code path as its old syntax equivalent (a nullable struct). This
  // allows us to call `f(type->boxed_type)` wherever we used to call `f(type)`
  // in the old code.
  // As a temporary workaround for piping unconst-ness everywhere or having
  // box types own their own boxed types, we cast away the const to be able
  // to change the boxed type to be mutable.
  auto* mutable_inner = const_cast<IdentifierType*>(inner);
  mutable_inner->nullability = Nullability::kNullable;

  out_params_->boxed_type_resolved = boxed_type;
  out_params_->boxed_type_raw = parameters_.items[0]->AsTypeCtor();

  BoxType type(layout_.resolved().name(), boxed_type);
  std::unique_ptr<Type> constrained_type;
  type.ApplyConstraints(resolver_, reporter(), constraints_, layout_, &constrained_type,
                        out_params_);
  return typespace_->Intern(std::move(constrained_type));
}

}  // namespace fidlc
