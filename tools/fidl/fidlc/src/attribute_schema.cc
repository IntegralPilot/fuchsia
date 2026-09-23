// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/attribute_schema.h"

#include <zircon/assert.h>

#include "tools/fidl/fidlc/src/compile_step.h"
#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/transport.h"
#include "tools/fidl/fidlc/src/typespace.h"

namespace fidlc {

AttributeSchema& AttributeSchema::RestrictTo(std::set<Element::Kind> placements) {
  ZX_ASSERT_MSG(!placements.empty(), "must allow some placements");
  ZX_ASSERT_MSG(kind_ == AttributeSchema::Kind::kValidateOnly ||
                    kind_ == AttributeSchema::Kind::kUseEarly ||
                    kind_ == AttributeSchema::Kind::kCompileEarly,
                "wrong kind");
  ZX_ASSERT_MSG(placement_ == AttributeSchema::Placement::kAnywhere, "already set placements");
  ZX_ASSERT_MSG(specific_placements_.empty(), "already set placements");
  placement_ = AttributeSchema::Placement::kSpecific;
  specific_placements_ = std::move(placements);
  return *this;
}

AttributeSchema& AttributeSchema::RestrictToAnonymousLayouts() {
  ZX_ASSERT_MSG(kind_ == AttributeSchema::Kind::kValidateOnly ||
                    kind_ == AttributeSchema::Kind::kUseEarly ||
                    kind_ == AttributeSchema::Kind::kCompileEarly,
                "wrong kind");
  ZX_ASSERT_MSG(placement_ == AttributeSchema::Placement::kAnywhere, "already set placements");
  ZX_ASSERT_MSG(specific_placements_.empty(), "already set placements");
  placement_ = AttributeSchema::Placement::kAnonymousLayout;
  return *this;
}

AttributeSchema& AttributeSchema::DisallowOnAnonymousLayouts() {
  ZX_ASSERT_MSG(kind_ == AttributeSchema::Kind::kValidateOnly ||
                    kind_ == AttributeSchema::Kind::kUseEarly ||
                    kind_ == AttributeSchema::Kind::kCompileEarly,
                "wrong kind");
  ZX_ASSERT_MSG(placement_ == AttributeSchema::Placement::kAnywhere, "already set placements");
  ZX_ASSERT_MSG(specific_placements_.empty(), "already set placements");
  placement_ = AttributeSchema::Placement::kAnythingButAnonymousLayout;
  return *this;
}

AttributeSchema& AttributeSchema::AddArg(AttributeArgSchema arg_schema) {
  ZX_ASSERT_MSG(kind_ == AttributeSchema::Kind::kValidateOnly ||
                    kind_ == AttributeSchema::Kind::kUseEarly ||
                    kind_ == AttributeSchema::Kind::kCompileEarly,
                "wrong kind");
  ZX_ASSERT_MSG(arg_schemas_.empty(), "can only have one unnamed arg");
  arg_schemas_.emplace(AttributeArg::kDefaultAnonymousName, arg_schema);
  return *this;
}

AttributeSchema& AttributeSchema::AddArg(std::string name, AttributeArgSchema arg_schema) {
  ZX_ASSERT_MSG(kind_ == AttributeSchema::Kind::kValidateOnly ||
                    kind_ == AttributeSchema::Kind::kUseEarly ||
                    kind_ == AttributeSchema::Kind::kCompileEarly,
                "wrong kind");
  auto [_, inserted] = arg_schemas_.try_emplace(std::move(name), arg_schema);
  ZX_ASSERT_MSG(inserted, "duplicate argument name");
  return *this;
}

AttributeSchema& AttributeSchema::Constrain(AttributeSchema::Constraint constraint) {
  ZX_ASSERT_MSG(constraint != nullptr, "constraint must be non-null");
  ZX_ASSERT_MSG(constraint_ == nullptr, "already set constraint");
  ZX_ASSERT_MSG(kind_ == AttributeSchema::Kind::kValidateOnly,
                "constraints only allowed on kValidateOnly attributes");
  constraint_ = std::move(constraint);
  return *this;
}

AttributeSchema& AttributeSchema::UseEarly() {
  ZX_ASSERT_MSG(kind_ == AttributeSchema::Kind::kValidateOnly, "already changed kind");
  ZX_ASSERT_MSG(constraint_ == nullptr, "use-early attribute should not specify constraint");
  kind_ = AttributeSchema::Kind::kUseEarly;
  return *this;
}

AttributeSchema& AttributeSchema::CompileEarly() {
  ZX_ASSERT_MSG(kind_ == AttributeSchema::Kind::kValidateOnly, "already changed kind");
  ZX_ASSERT_MSG(constraint_ == nullptr, "compile-early attribute should not specify constraint");
  kind_ = AttributeSchema::Kind::kCompileEarly;
  return *this;
}

AttributeSchema& AttributeSchema::Deprecate() {
  ZX_ASSERT_MSG(kind_ == AttributeSchema::Kind::kValidateOnly, "wrong kind");
  ZX_ASSERT_MSG(placement_ == AttributeSchema::Placement::kAnywhere,
                "deprecated attribute should not specify placement");
  ZX_ASSERT_MSG(arg_schemas_.empty(), "deprecated attribute should not specify arguments");
  ZX_ASSERT_MSG(constraint_ == nullptr, "deprecated attribute should not specify constraint");
  kind_ = AttributeSchema::Kind::kDeprecated;
  return *this;
}

// static
const AttributeSchema AttributeSchema::kUserDefined(Kind::kUserDefined);

void AttributeSchema::Validate(Reporter* reporter, ExperimentalFlagSet flags,
                               const Attribute* attribute, const Element* element) const {
  switch (kind_) {
    case Kind::kValidateOnly:
      break;
    case Kind::kUseEarly:
    case Kind::kCompileEarly:
      ZX_ASSERT_MSG(constraint_ == nullptr,
                    "use-early and compile-early schemas should not have a constraint");
      break;
    case Kind::kDeprecated:
      reporter->Fail(ErrDeprecatedAttribute, attribute->span, attribute);
      return;
    case Kind::kUserDefined:
      return;
  }

  bool valid_placement;
  switch (placement_) {
    case Placement::kAnywhere:
      valid_placement = true;
      break;
    case Placement::kSpecific:
      valid_placement = specific_placements_.count(element->kind) > 0;
      break;
    case Placement::kAnonymousLayout:
      valid_placement = element->IsAnonymousLayout();
      break;
    case Placement::kAnythingButAnonymousLayout:
      valid_placement = !element->IsAnonymousLayout();
      break;
  }
  if (!valid_placement) {
    reporter->Fail(ErrInvalidAttributePlacement, attribute->span, attribute);
    return;
  }

  if (constraint_ == nullptr) {
    return;
  }
  auto check = reporter->Checkpoint();
  auto passed = constraint_(reporter, flags, attribute, element);
  if (passed) {
    ZX_ASSERT_MSG(check.NoNewErrors(), "cannot add errors and pass");
    return;
  }
  ZX_ASSERT_MSG(!check.NoNewErrors(), "cannot fail a constraint without reporting errors");
}

void AttributeSchema::ResolveArgs(CompileStep* step, Attribute* attribute) const {
  Reporter* reporter = step->reporter();

  switch (kind_) {
    case Kind::kValidateOnly:
    case Kind::kUseEarly:
    case Kind::kCompileEarly:
      break;
    case Kind::kDeprecated:
      // Don't attempt to resolve arguments, as we don't store argument schemas
      // for deprecated attributes. Instead, rely on AttributeSchema::Validate
      // to report the error.
      return;
    case Kind::kUserDefined:
      ResolveArgsWithoutSchema(step, attribute);
      return;
  }

  // Name the anonymous argument (if present).
  if (auto anon_arg = attribute->GetStandaloneAnonymousArg()) {
    if (arg_schemas_.empty()) {
      reporter->Fail(ErrAttributeDisallowsArgs, attribute->span, attribute);
      return;
    }
    if (arg_schemas_.size() > 1) {
      reporter->Fail(ErrAttributeArgNotNamed, attribute->span, anon_arg->value->span.data());
      return;
    }
    anon_arg->name = step->generated_source_file()->AddLine(arg_schemas_.begin()->first);
  } else if (arg_schemas_.size() == 1 && attribute->args.size() == 1) {
    reporter->Fail(ErrAttributeArgMustNotBeNamed, attribute->span);
  }

  // Resolve each argument by name.
  for (auto& arg : attribute->args) {
    const auto it = arg_schemas_.find(arg->name.value().data());
    if (it == arg_schemas_.end()) {
      reporter->Fail(ErrUnknownAttributeArg, attribute->span, attribute, arg->name.value().data());
      continue;
    }
    const auto& [name, schema] = *it;
    const bool literal_only = kind_ == Kind::kCompileEarly;
    schema.ResolveArg(step, attribute, arg.get(), literal_only);
  }

  // Check for missing arguments.
  for (const auto& [name, schema] : arg_schemas_) {
    if (schema.IsOptional() || attribute->GetArg(name) != nullptr) {
      continue;
    }
    if (arg_schemas_.size() == 1) {
      reporter->Fail(ErrMissingRequiredAnonymousAttributeArg, attribute->span, attribute);
    } else {
      reporter->Fail(ErrMissingRequiredAttributeArg, attribute->span, attribute, name);
    }
  }
}

static bool ResolveAsSpecialVersion(CompileStep* step, IdentifierConstant* constant) {
  auto& components = constant->reference.raw_sourced().components;
  if (components.size() != 1)
    return false;
  auto name = components[0];
  auto& decls = step->all_libraries()->root_library()->declarations;
  Builtin* builtin;
  std::optional<Version> version;
  if (name == Version::kNext.name()) {
    builtin = decls.LookupBuiltin(Builtin::Identity::kNext);
    version = Version::kNext;
  } else if (name == Version::kHead.name()) {
    builtin = decls.LookupBuiltin(Builtin::Identity::kHead);
    version = Version::kHead;
  } else {
    return false;
  }
  constant->reference.ResolveTo(Reference::Target(builtin));
  constant->ResolveTo(std::make_unique<NumericConstantValue<uint32_t>>(version->number()),
                      step->typespace()->GetPrimitiveType(PrimitiveSubtype::kUint32));
  return true;
}

void AttributeArgSchema::ResolveArg(CompileStep* step, Attribute* attribute, AttributeArg* arg,
                                    bool literal_only) const {
  Reporter* reporter = step->reporter();
  Constant* constant = arg->value.get();
  ZX_ASSERT_MSG(!constant->IsResolved(), "argument should not be resolved yet");

  ConstantValue::Kind kind;
  if (auto special_case = std::get_if<SpecialCase>(&type_)) {
    switch (*special_case) {
      case SpecialCase::kVersion:
        if (constant->kind == Constant::Kind::kIdentifier) {
          if (!ResolveAsSpecialVersion(step, static_cast<IdentifierConstant*>(constant)))
            reporter->Fail(ErrInvalidVersion, arg->span, arg->value->span.data());
          return;
        }
        kind = ConstantValue::Kind::kUint32;
        break;
    }
  } else {
    kind = std::get<ConstantValue::Kind>(type_);
  }

  if (literal_only && constant->kind != Constant::Kind::kLiteral) {
    reporter->Fail(ErrAttributeArgRequiresLiteral, constant->span, arg->name.value().data(),
                   attribute);
    return;
  }

  const Type* target_type;
  switch (kind) {
    case ConstantValue::Kind::kDocComment:
      ZX_PANIC("we know the target type of doc comments, and should not end up here");
    case ConstantValue::Kind::kString:
      target_type = step->typespace()->GetUnboundedStringType();
      break;
    case ConstantValue::Kind::kBool:
      target_type = step->typespace()->GetPrimitiveType(PrimitiveSubtype::kBool);
      break;
    case ConstantValue::Kind::kInt8:
      target_type = step->typespace()->GetPrimitiveType(PrimitiveSubtype::kInt8);
      break;
    case ConstantValue::Kind::kInt16:
      target_type = step->typespace()->GetPrimitiveType(PrimitiveSubtype::kInt16);
      break;
    case ConstantValue::Kind::kInt32:
      target_type = step->typespace()->GetPrimitiveType(PrimitiveSubtype::kInt32);
      break;
    case ConstantValue::Kind::kInt64:
      target_type = step->typespace()->GetPrimitiveType(PrimitiveSubtype::kInt64);
      break;
    case ConstantValue::Kind::kUint8:
      target_type = step->typespace()->GetPrimitiveType(PrimitiveSubtype::kUint8);
      break;
    case ConstantValue::Kind::kZxUchar:
      target_type = step->typespace()->GetPrimitiveType(PrimitiveSubtype::kZxUchar);
      break;
    case ConstantValue::Kind::kUint16:
      target_type = step->typespace()->GetPrimitiveType(PrimitiveSubtype::kUint16);
      break;
    case ConstantValue::Kind::kUint32:
      target_type = step->typespace()->GetPrimitiveType(PrimitiveSubtype::kUint32);
      break;
    case ConstantValue::Kind::kUint64:
      target_type = step->typespace()->GetPrimitiveType(PrimitiveSubtype::kUint64);
      break;
    case ConstantValue::Kind::kZxUsize64:
      target_type = step->typespace()->GetPrimitiveType(PrimitiveSubtype::kZxUsize64);
      break;
    case ConstantValue::Kind::kZxUintptr64:
      target_type = step->typespace()->GetPrimitiveType(PrimitiveSubtype::kZxUintptr64);
      break;
    case ConstantValue::Kind::kFloat32:
      target_type = step->typespace()->GetPrimitiveType(PrimitiveSubtype::kFloat32);
      break;
    case ConstantValue::Kind::kFloat64:
      target_type = step->typespace()->GetPrimitiveType(PrimitiveSubtype::kFloat64);
      break;
  }
  if (!step->ResolveConstant(constant, target_type)) {
    reporter->Fail(ErrCouldNotResolveAttributeArg, arg->span);
  }
}

// static
void AttributeSchema::ResolveArgsWithoutSchema(CompileStep* step, Attribute* attribute) {
  Reporter* reporter = step->reporter();

  // For attributes with a single, anonymous argument like `@foo("bar")`, assign
  // a default name so that arguments are always named after compilation.
  if (auto anon_arg = attribute->GetStandaloneAnonymousArg()) {
    anon_arg->name = step->generated_source_file()->AddLine(AttributeArg::kDefaultAnonymousName);
  }

  // Try resolving each argument as string or bool. We don't allow numerics
  // because it's not clear what type (int8, uint32, etc.) we should infer.
  for (const auto& arg : attribute->args) {
    ZX_ASSERT_MSG(arg->value->kind != Constant::Kind::kBinaryOperator,
                  "attribute arg with a binary operator is a parse error");

    auto inferred_type = step->InferType(arg->value.get());
    if (!inferred_type) {
      reporter->Fail(ErrCouldNotResolveAttributeArg, attribute->span);
      continue;
    }
    // Only string or bool supported.
    switch (inferred_type->kind) {
      case Type::Kind::kString:
        break;
      case Type::Kind::kPrimitive:
        if (static_cast<const PrimitiveType*>(inferred_type)->subtype == PrimitiveSubtype::kBool) {
          break;
        }
        [[fallthrough]];
      case Type::Kind::kInternal:
      case Type::Kind::kIdentifier:
      case Type::Kind::kArray:
      case Type::Kind::kBox:
      case Type::Kind::kVector:
      case Type::Kind::kZxExperimentalPointer:
      case Type::Kind::kHandle:
      case Type::Kind::kTransportSide:
      case Type::Kind::kUntypedNumeric:
        reporter->Fail(ErrCanOnlyUseStringOrBool, attribute->span, arg.get(), attribute);
        continue;
    }
    ZX_ASSERT_MSG(step->ResolveConstant(arg->value.get(), inferred_type),
                  "resolving cannot fail when we've inferred the type");
  }
}

static bool DiscoverableConstraint(Reporter* reporter, ExperimentalFlagSet flags,
                                   const Attribute* attr, const Element* element) {
  if (auto arg = attr->GetArg("name")) {
    auto name = arg->value->Value().AsString().value();
    if (!IsValidDiscoverableName(name)) {
      return reporter->Fail(ErrInvalidDiscoverableName, arg->span, name);
    }
  }
  if (auto arg = attr->GetArg("client")) {
    auto locations = arg->value->Value().AsString().value();
    if (!IsValidImplementationLocations(locations)) {
      return reporter->Fail(ErrInvalidDiscoverableLocation, arg->span, locations);
    }
  }
  if (auto arg = attr->GetArg("server")) {
    auto locations = arg->value->Value().AsString().value();
    if (!IsValidImplementationLocations(locations)) {
      return reporter->Fail(ErrInvalidDiscoverableLocation, arg->span, locations);
    }
  }
  return true;
}

static bool TransportConstraint(Reporter* reporter, ExperimentalFlagSet flags,
                                const Attribute* attribute, const Element* element) {
  ZX_ASSERT(element);
  ZX_ASSERT(element->kind == Element::Kind::kProtocol);
  auto value =
      attribute->GetArg(AttributeArg::kDefaultAnonymousName)->value->Value().AsString().value();
  if (!Transport::FromTransportName(value)) {
    return reporter->Fail(ErrInvalidTransportType, attribute->span, value,
                          Transport::AllTransportNames());
  }
  return true;
}

static bool NoResourceConstraint(Reporter* reporter, ExperimentalFlagSet flags,
                                 const Attribute* attribute, const Element* element) {
  ZX_ASSERT(element);
  ZX_ASSERT(element->kind == Element::Kind::kProtocol);
  if (!flags.IsEnabled(ExperimentalFlag::kNoResourceAttribute)) {
    return reporter->Fail(ErrExperimentalNoResource, attribute->span);
  }
  return true;
}

// static
AttributeSchemaMap AttributeSchema::OfficialAttributes() {
  AttributeSchemaMap map;
  // This attribute exists only to demonstrate and test our ability to deprecate
  // attributes. It will never be removed.
  map["example_deprecated_attribute"].Deprecate();
  map["discoverable"]
      .RestrictTo({
          Element::Kind::kProtocol,
      })
      .AddArg("name", AttributeArgSchema(ConstantValue::Kind::kString,
                                         AttributeArgSchema::Optionality::kOptional))
      .AddArg("client", AttributeArgSchema(ConstantValue::Kind::kString,
                                           AttributeArgSchema::Optionality::kOptional))
      .AddArg("server", AttributeArgSchema(ConstantValue::Kind::kString,
                                           AttributeArgSchema::Optionality::kOptional))
      .Constrain(DiscoverableConstraint);
  map["serializable"]
      .RestrictTo({Element::Kind::kStruct, Element::Kind::kTable, Element::Kind::kUnion})
      .AddArg("read", AttributeArgSchema(ConstantValue::Kind::kString,
                                         AttributeArgSchema::Optionality::kOptional))
      .AddArg("write", AttributeArgSchema(ConstantValue::Kind::kString,
                                          AttributeArgSchema::Optionality::kOptional));
  map[std::string(Attribute::kDocCommentName)].AddArg(
      AttributeArgSchema(ConstantValue::Kind::kString));
  map["generated_name"]
      .RestrictToAnonymousLayouts()
      .AddArg(AttributeArgSchema(ConstantValue::Kind::kString))
      .CompileEarly();
  map["selector"]
      .RestrictTo({
          Element::Kind::kProtocolMethod,
      })
      .AddArg(AttributeArgSchema(ConstantValue::Kind::kString))
      .UseEarly();
  map["transitional"].Deprecate();
  map["transport"]
      .RestrictTo({
          Element::Kind::kProtocol,
      })
      .AddArg(AttributeArgSchema(ConstantValue::Kind::kString))
      .Constrain(TransportConstraint);
  map["unknown"].RestrictTo({Element::Kind::kEnumMember});
  map["available"]
      .DisallowOnAnonymousLayouts()
      .AddArg("platform", AttributeArgSchema(ConstantValue::Kind::kString,
                                             AttributeArgSchema::Optionality::kOptional))
      .AddArg("added", AttributeArgSchema(AttributeArgSchema::SpecialCase::kVersion,
                                          AttributeArgSchema::Optionality::kOptional))
      .AddArg("deprecated", AttributeArgSchema(AttributeArgSchema::SpecialCase::kVersion,
                                               AttributeArgSchema::Optionality::kOptional))
      .AddArg("removed", AttributeArgSchema(AttributeArgSchema::SpecialCase::kVersion,
                                            AttributeArgSchema::Optionality::kOptional))
      .AddArg("replaced", AttributeArgSchema(AttributeArgSchema::SpecialCase::kVersion,
                                             AttributeArgSchema::Optionality::kOptional))
      .AddArg("renamed", AttributeArgSchema(ConstantValue::Kind::kString,
                                            AttributeArgSchema::Optionality::kOptional))
      .AddArg("note", AttributeArgSchema(ConstantValue::Kind::kString,
                                         AttributeArgSchema::Optionality::kOptional))
      .CompileEarly();
  map["no_resource"].RestrictTo({Element::Kind::kProtocol}).Constrain(NoResourceConstraint);
  map["sensitive"].RestrictTo({
      Element::Kind::kStruct,
      Element::Kind::kTable,
      Element::Kind::kUnion,
      Element::Kind::kStructMember,
      Element::Kind::kTableMember,
      Element::Kind::kUnionMember,
      Element::Kind::kOverlay,
      Element::Kind::kOverlayMember,
      Element::Kind::kNewType,
  });
  return map;
}

}  // namespace fidlc
