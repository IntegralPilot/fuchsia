// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/json_generator.h"

#include <zircon/assert.h>

#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/name.h"
#include "tools/fidl/fidlc/src/names.h"
#include "tools/fidl/fidlc/src/properties.h"
#include "tools/fidl/fidlc/src/types.h"

namespace fidlc {

void JSONGenerator::Generate(Version version) { EmitString(version.ToString()); }

void JSONGenerator::Generate(SourceSpan value) { EmitString(value.data()); }

void JSONGenerator::Generate(NameSpan value) {
  GenerateObject([&]() {
    GenerateObjectMember("filename", value.filename, Position::kFirst);
    GenerateObjectMember("line", static_cast<uint32_t>(value.position.line));
    GenerateObjectMember("column", static_cast<uint32_t>(value.position.column));
    GenerateObjectMember("length", static_cast<uint32_t>(value.length));
  });
}

void JSONGenerator::Generate(const ConstantValue& value) {
  switch (value.kind) {
    case ConstantValue::Kind::kUint8:
    case ConstantValue::Kind::kZxUchar:
    case ConstantValue::Kind::kUint16:
    case ConstantValue::Kind::kUint32:
    case ConstantValue::Kind::kUint64:
    case ConstantValue::Kind::kZxUsize64:
    case ConstantValue::Kind::kZxUintptr64:
      EmitNumeric(value.AsUnsigned().value(), kAsString);
      break;
    case ConstantValue::Kind::kInt8:
    case ConstantValue::Kind::kInt16:
    case ConstantValue::Kind::kInt32:
    case ConstantValue::Kind::kInt64:
      EmitNumeric(value.AsSigned().value(), kAsString);
      break;
    case ConstantValue::Kind::kFloat32:
      EmitNumeric(value.AsNumeric<float>().value(), kAsString);
      break;
    case ConstantValue::Kind::kFloat64:
      EmitNumeric(value.AsNumeric<double>().value(), kAsString);
      break;
    case ConstantValue::Kind::kBool:
      EmitBoolean(static_cast<const BoolConstantValue&>(value).value, kAsString);
      break;
    case ConstantValue::Kind::kDocComment:
      EmitString(static_cast<const DocCommentConstantValue&>(value).value);
      break;
    case ConstantValue::Kind::kString:
      EmitLiteral(value.AsString().value());
      break;
  }
}

void JSONGenerator::Generate(HandleSubtype value) { EmitString(NameHandleSubtype(value)); }

void JSONGenerator::Generate(Nullability value) {
  switch (value) {
    case Nullability::kNullable:
      EmitBoolean(true);
      break;
    case Nullability::kNonnullable:
      EmitBoolean(false);
      break;
  }
}

void JSONGenerator::Generate(Strictness value) { EmitBoolean(value == Strictness::kStrict); }

void JSONGenerator::Generate(Openness value) {
  switch (value) {
    case Openness::kOpen:
      EmitString("open");
      break;
    case Openness::kAjar:
      EmitString("ajar");
      break;
    case Openness::kClosed:
      EmitString("closed");
      break;
  }
}

void JSONGenerator::Generate(TransportSide value) {
  switch (value) {
    case TransportSide::kClient:
      EmitString("client");
      break;
    case TransportSide::kServer:
      EmitString("server");
      break;
  }
}

void JSONGenerator::Generate(const RawIdentifier& value) { EmitString(value.span().data()); }

void JSONGenerator::Generate(const LiteralConstant& value) {
  GenerateObject([&]() {
    GenerateObjectMember("kind", NameRawLiteralKind(value.literal->kind), Position::kFirst);
    GenerateObjectMember("value", value.Value());
    GenerateObjectMember("expression", value.literal->span().data());
  });
}

void JSONGenerator::Generate(const Constant& value) {
  GenerateObject([&]() {
    GenerateObjectMember("kind", NameConstantKind(value.kind), Position::kFirst);
    GenerateObjectMember("value", value.Value());
    GenerateObjectMember("expression", value.span);
    switch (value.kind) {
      case Constant::Kind::kIdentifier: {
        auto identifier = static_cast<const IdentifierConstant*>(&value);
        GenerateObjectMember("identifier", identifier->reference.resolved().name());
        break;
      }
      case Constant::Kind::kLiteral: {
        auto literal = static_cast<const LiteralConstant*>(&value);
        GenerateObjectMember("literal", *literal);
        break;
      }
      case Constant::Kind::kBinaryOperator: {
        // Avoid emitting a structure for binary operators in favor of "expression".
        break;
      }
    }
  });
}

void JSONGenerator::Generate(const Type* value) {
  GenerateObject([&]() {
    GenerateObjectMember("kind_v2", NameTypeKind(value), Position::kFirst);

    switch (value->kind) {
      case Type::Kind::kBox: {
        const auto* type = static_cast<const BoxType*>(value)->boxed_type;
        ZX_ASSERT(type->kind == Type::Kind::kIdentifier);
        const auto* id_type = static_cast<const IdentifierType*>(type);
        GenerateObjectMember("identifier", id_type->name);
        GenerateObjectMember("nullable", id_type->nullability);
        break;
      }
      case Type::Kind::kVector: {
        // This code path should only be exercised if the type is "bytes." All
        // other handling of kVector is handled in GenerateParameterizedType.
        const auto* type = static_cast<const VectorType*>(value);
        GenerateObjectMember("element_type", type->element_type);
        if (type->ElementCount() < kMaxSize)
          GenerateObjectMember("maybe_element_count", type->ElementCount());
        GenerateObjectMember("nullable", type->nullability);
        break;
      }
      case Type::Kind::kString: {
        const auto* type = static_cast<const StringType*>(value);
        if (type->MaxSize() < kMaxSize)
          GenerateObjectMember("maybe_element_count", type->MaxSize());
        GenerateObjectMember("nullable", type->nullability);
        break;
      }
      case Type::Kind::kHandle: {
        const auto* type = static_cast<const HandleType*>(value);
        GenerateObjectMember("obj_type", static_cast<uint32_t>(type->subtype));
        GenerateObjectMember("subtype", type->subtype);
        GenerateObjectMember("rights", type->rights->AsNumeric<uint32_t>().value());
        GenerateObjectMember("nullable", type->nullability);
        GenerateObjectMember("resource_identifier", FullyQualifiedName(type->resource_decl->name));
        break;
      }
      case Type::Kind::kPrimitive: {
        const auto* type = static_cast<const PrimitiveType*>(value);
        GenerateObjectMember("subtype", type->name);
        break;
      }
      case Type::Kind::kInternal: {
        const auto* type = static_cast<const InternalType*>(value);
        switch (type->subtype) {
          case InternalSubtype::kFrameworkErr:
            GenerateObjectMember("subtype", std::string_view("framework_error"));
            break;
        }
        break;
      }
      case Type::Kind::kIdentifier: {
        const auto* type = static_cast<const IdentifierType*>(value);
        GenerateObjectMember("identifier", type->name);
        GenerateObjectMember("nullable", type->nullability);
        break;
      }
      case Type::Kind::kTransportSide: {
        const auto* type = static_cast<const TransportSideType*>(value);

        GenerateObjectMember("role", type->end);
        GenerateObjectMember("protocol", type->protocol_decl->name);
        GenerateObjectMember("nullable", type->nullability);
        GenerateObjectMember("protocol_transport", type->protocol_transport);

        break;
      }
      case Type::Kind::kZxExperimentalPointer: {
        const auto* type = static_cast<const ZxExperimentalPointerType*>(value);
        GenerateObjectMember("pointee_type", type->pointee_type);
        break;
      }
      case Type::Kind::kArray:
      case Type::Kind::kUntypedNumeric:
        ZX_PANIC("unexpected kind");
    }

    GenerateObjectMember("type_shape_v2", value->type_shape.value());
  });
}

void JSONGenerator::Generate(const AttributeArg& value) {
  GenerateObject([&]() {
    ZX_ASSERT_MSG(
        value.name.has_value(),
        "anonymous attribute argument names should always be inferred during compilation");
    GenerateObjectMember("name", value.name.value(), Position::kFirst);
    GenerateObjectMember("type", value.value->type->name);
    GenerateObjectMember("value", value.value);
    ZX_ASSERT(value.span.valid());
    GenerateObjectMember("location", NameSpan(value.span));
  });
}

void JSONGenerator::Generate(const Attribute& value) {
  GenerateObject([&]() {
    const auto& name = ToLowerSnakeCase(std::string(value.name.data()));
    GenerateObjectMember("name", name, Position::kFirst);
    GenerateObjectMember("arguments", value.args);
    ZX_ASSERT(value.span.valid());
    GenerateObjectMember("location", NameSpan(value.span));
  });
}

void JSONGenerator::Generate(const AttributeList& value) { Generate(value.attributes); }

void JSONGenerator::Generate(const RawOrdinal64& value) { EmitNumeric(value.value); }

void JSONGenerator::GenerateDeclName(const Name& name) {
  GenerateObjectMember("name", name, Position::kFirst);
  if (auto n = name.as_anonymous()) {
    GenerateObjectMember("naming_context", n->context->Context());
  } else {
    std::vector<std::string> ctx = {std::string(name.decl_name())};
    GenerateObjectMember("naming_context", ctx);
  }
}

void JSONGenerator::Generate(const Name& name) {
  // TODO(https://fxbug.dev/42174095): FullyQualifiedName omits the library name
  // for builtins, since we want errors to say "uint32" not "fidl/uint32".
  // However, builtins MAX and HEAD can end up in the JSON IR as identifier
  // constants, and to satisfy the schema we must produce a proper compound
  // identifier (with a library name). We should solve this in a cleaner way.
  if (name.is_intrinsic() && name.decl_name() == "MAX") {
    EmitString(std::string_view("fidl/MAX"));
  } else if (name.is_intrinsic() && name.decl_name() == "NEXT") {
    EmitString(std::string_view("fidl/NEXT"));
  } else if (name.is_intrinsic() && name.decl_name() == "HEAD") {
    EmitString(std::string_view("fidl/HEAD"));
  } else {
    EmitString(FullyQualifiedName(name));
  }
}

void JSONGenerator::Generate(const Bits& value) {
  GenerateObject([&]() {
    GenerateDeclName(value.name);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    GenerateTypeAndFromAlias(value.subtype_ctor.get());
    // TODO(https://fxbug.dev/42156522): When all numbers are wrapped as string, we can simply
    // call GenerateObjectMember directly.
    GenerateObjectPunctuation(Position::kSubsequent);
    EmitObjectKey("mask");
    EmitNumeric(value.mask, kAsString);
    GenerateObjectMember("members", value.members);
    GenerateObjectMember("strict", value.strictness.value());
  });
}

void JSONGenerator::Generate(const Bits::Member& value) {
  GenerateObject([&]() {
    GenerateObjectMember("name", value.name, Position::kFirst);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    GenerateObjectMember("value", value.value);
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
  });
}

void JSONGenerator::Generate(const Const& value) {
  GenerateObject([&]() {
    GenerateObjectMember("name", value.name, Position::kFirst);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    GenerateTypeAndFromAlias(value.type_ctor.get());
    GenerateObjectMember("value", value.value);
  });
}

void JSONGenerator::Generate(const Enum& value) {
  GenerateObject([&]() {
    GenerateDeclName(value.name);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    // TODO(https://fxbug.dev/42156522): Due to legacy reasons, the 'type' of enums is actually
    // the primitive subtype, and therefore cannot use
    // GenerateTypeAndFromAlias here.
    GenerateObjectMember("type", value.type->name);
    GenerateExperimentalMaybeFromAlias(value.subtype_ctor->resolved_params);
    GenerateObjectMember("members", value.members);
    GenerateObjectMember("strict", value.strictness.value());
    if (value.strictness.value() == Strictness::kFlexible) {
      if (value.unknown_value_signed) {
        GenerateObjectMember("maybe_unknown_value", value.unknown_value_signed.value());
      } else {
        GenerateObjectMember("maybe_unknown_value", value.unknown_value_unsigned.value());
      }
    }
  });
}

void JSONGenerator::Generate(const Enum::Member& value) {
  GenerateObject([&]() {
    GenerateObjectMember("name", value.name, Position::kFirst);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    GenerateObjectMember("value", value.value);
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
  });
}

void JSONGenerator::Generate(const Protocol& value) {
  GenerateObject([&]() {
    GenerateObjectMember("name", value.name, Position::kFirst);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    GenerateObjectMember("openness", value.openness.value());
    GenerateObjectMember("composed_protocols", value.composed_protocols);
    GenerateObjectMember("methods", value.all_methods);
    GenerateProtocolImplementationLocations(value);
  });
}

void JSONGenerator::Generate(const Protocol::ComposedProtocol& value) {
  GenerateObject([&]() {
    GenerateObjectMember("name", value.reference.resolved().name(), Position::kFirst);
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    GenerateObjectMember("location", NameSpan(value.reference.span()));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
  });
}

void JSONGenerator::Generate(const Protocol::MethodWithInfo& method_with_info) {
  ZX_ASSERT(method_with_info.method != nullptr);
  const auto& value = *method_with_info.method;
  GenerateObject([&]() {
    GenerateObjectMember("kind", NameMethodKind(value.kind), Position::kFirst);
    GenerateObjectMember("ordinal", value.ordinal);
    GenerateObjectMember("name", value.name);
    GenerateObjectMember("strict", value.strictness.value());
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    GenerateObjectMember("has_request", value.kind != Protocol::Method::Kind::kEvent);
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    if (value.maybe_request) {
      GenerateTypeAndFromAlias(TypeKind::kRequestPayload, value.maybe_request.get(),
                               Position::kSubsequent);
    }
    GenerateObjectMember("has_response", value.kind != Protocol::Method::Kind::kOneWay);
    if (value.maybe_response) {
      GenerateTypeAndFromAlias(TypeKind::kResponsePayload, value.maybe_response.get(),
                               Position::kSubsequent);
    }
    GenerateObjectMember("is_composed", method_with_info.composed != nullptr);
    GenerateObjectMember("has_error", value.has_error);
    if (auto type_ctor = value.result_success_type_ctor)
      GenerateObjectMember("maybe_response_success_type", type_ctor->type);
    if (auto type_ctor = value.result_domain_error_type_ctor)
      GenerateObjectMember("maybe_response_err_type", type_ctor->type);
  });
}
void JSONGenerator::GenerateProtocolImplementationLocations(const Protocol& value) {
  auto discoverable = value.attributes->Get("discoverable");
  if (discoverable && (discoverable->GetArg("client") || discoverable->GetArg("server"))) {
    GenerateObjectPunctuation(Position::kSubsequent);
    EmitObjectKey("implementation_locations");
    GenerateObject([&]() {
      GenerateEndpointImplementationLocations("client", discoverable, Position::kFirst);
      GenerateEndpointImplementationLocations("server", discoverable, Position::kSubsequent);
    });
  }
}

void JSONGenerator::GenerateEndpointImplementationLocations(const std::string_view& endpoint,
                                                            const Attribute* discoverable,
                                                            Position position) {
  auto arg = discoverable->GetArg(endpoint);
  if (arg) {
    auto locations = ParseImplementationLocations(arg->value->Value().AsString().value());
    ZX_ASSERT(locations.has_value());
    GenerateObjectMember(endpoint, locations.value(), position);
  } else {
    static const std::vector<std::string_view> all_locations{"platform", "external"};
    GenerateObjectMember(endpoint, all_locations, position);
  }
}

void JSONGenerator::GenerateTypeAndFromAlias(const TypeConstructor* value, Position position) {
  GenerateTypeAndFromAlias(TypeKind::kConcrete, value, position);
}

void JSONGenerator::GenerateTypeAndFromAlias(TypeKind parent_type_kind,
                                             const TypeConstructor* value, Position position) {
  const auto* type = value->type;
  const auto& invocation = value->resolved_params;
  if (type->kind == Type::Kind::kArray || type->kind == Type::Kind::kVector) {
    if (invocation.from_alias) {
      GenerateParameterizedType(parent_type_kind, type,
                                invocation.from_alias->partial_type_ctor.get(), position);
    } else {
      GenerateParameterizedType(parent_type_kind, type, value, position);
    }
    GenerateExperimentalMaybeFromAlias(invocation);
    return;
  }

  std::string key;
  switch (parent_type_kind) {
    case kConcrete: {
      key = "type";
      break;
    }
    case kParameterized: {
      key = "element_type";
      break;
    }
    case kRequestPayload: {
      key = "maybe_request_payload";
      break;
    }
    case kResponsePayload: {
      key = "maybe_response_payload";
      break;
    }
  }

  GenerateObjectMember(key, type, position);
  GenerateExperimentalMaybeFromAlias(invocation);
}

void JSONGenerator::GenerateExperimentalMaybeFromAlias(const LayoutInvocation& invocation) {
  if (invocation.from_alias)
    GenerateObjectMember("experimental_maybe_from_alias", invocation);
}

void JSONGenerator::GenerateParameterizedType(TypeKind parent_type_kind, const Type* type,
                                              const TypeConstructor* type_ctor, Position position) {
  const auto& invocation = type_ctor->resolved_params;
  std::string key = parent_type_kind == TypeKind::kConcrete ? "type" : "element_type";

  GenerateObjectPunctuation(position);
  EmitObjectKey(key);
  GenerateObject([&]() {
    GenerateObjectMember("kind_v2", NameTypeKind(type), Position::kFirst);

    switch (type->kind) {
      case Type::Kind::kArray: {
        const auto* array_type = static_cast<const ArrayType*>(type);
        if (!array_type->IsStringArray()) {
          GenerateTypeAndFromAlias(TypeKind::kParameterized, invocation.element_type_raw);
        }
        GenerateObjectMember("element_count", array_type->element_count->value);
        break;
      }
      case Type::Kind::kVector: {
        const auto* vector_type = static_cast<const VectorType*>(type);
        GenerateTypeAndFromAlias(TypeKind::kParameterized, invocation.element_type_raw);
        if (vector_type->ElementCount() < kMaxSize)
          GenerateObjectMember("maybe_element_count", vector_type->ElementCount());
        GenerateObjectMember("nullable", vector_type->nullability);
        break;
      }
      case Type::Kind::kZxExperimentalPointer: {
        GenerateTypeAndFromAlias(TypeKind::kParameterized, invocation.element_type_raw);
        break;
      }
      case Type::Kind::kIdentifier:
      case Type::Kind::kString:
      case Type::Kind::kPrimitive:
      case Type::Kind::kBox:
      case Type::Kind::kHandle:
      case Type::Kind::kTransportSide:
      case Type::Kind::kUntypedNumeric:
        ZX_PANIC("unexpected kind");
      case Type::Kind::kInternal: {
        switch (static_cast<const InternalType*>(type)->subtype) {
          case InternalSubtype::kFrameworkErr:
            ZX_PANIC("unexpected kind");
        }
      }
    }
    GenerateObjectMember("type_shape_v2", type->type_shape.value());
  });
}

void JSONGenerator::Generate(const Resource::Property& value) {
  GenerateObject([&]() {
    GenerateObjectMember("name", value.name, Position::kFirst);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    GenerateTypeAndFromAlias(value.type_ctor.get());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
  });
}

void JSONGenerator::Generate(const Resource& value) {
  GenerateObject([&]() {
    GenerateObjectMember("name", value.name, Position::kFirst);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    GenerateTypeAndFromAlias(value.subtype_ctor.get());
    GenerateObjectMember("properties", value.properties);
  });
}

void JSONGenerator::Generate(const Service& value) {
  GenerateObject([&]() {
    GenerateObjectMember("name", value.name, Position::kFirst);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    GenerateObjectMember("members", value.members);
  });
}

void JSONGenerator::Generate(const Service::Member& value) {
  GenerateObject([&]() {
    GenerateTypeAndFromAlias(value.type_ctor.get(), Position::kFirst);
    GenerateObjectMember("name", value.name);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
  });
}

void JSONGenerator::Generate(const Struct& value) {
  GenerateObject([&]() {
    GenerateDeclName(value.name);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    GenerateObjectMember("members", value.members);
    GenerateObjectMember("resource", value.resourceness.value() == Resourceness::kResource);
    auto anon = value.name.as_anonymous();
    bool is_empty_success_struct =
        anon && anon->provenance == Name::Provenance::kGeneratedEmptySuccessStruct;
    GenerateObjectMember("is_empty_success_struct", is_empty_success_struct);
    GenerateObjectMember("type_shape_v2", value.type_shape.value());
  });
}

void JSONGenerator::Generate(const Struct::Member& value) {
  GenerateObject([&]() {
    GenerateTypeAndFromAlias(value.type_ctor.get(), Position::kFirst);
    GenerateObjectMember("name", value.name);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    if (value.maybe_default_value)
      GenerateObjectMember("maybe_default_value", value.maybe_default_value);
    GenerateObjectMember("field_shape_v2", value.field_shape);
  });
}

void JSONGenerator::Generate(const Table& value) {
  GenerateObject([&]() {
    GenerateDeclName(value.name);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    GenerateObjectMember("members", value.members);
    GenerateObjectMember("strict", value.strictness.value());
    GenerateObjectMember("resource", value.resourceness.value() == Resourceness::kResource);
    GenerateObjectMember("type_shape_v2", value.type_shape.value());
  });
}

void JSONGenerator::Generate(const Table::Member& value) {
  GenerateObject([&]() {
    GenerateObjectMember("ordinal", *value.ordinal, Position::kFirst);
    GenerateTypeAndFromAlias(value.type_ctor.get());
    GenerateObjectMember("name", value.name);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty()) {
      GenerateObjectMember("maybe_attributes", value.attributes);
    }
  });
}

void JSONGenerator::Generate(const TypeShape& type_shape) {
  GenerateObject([&]() {
    GenerateObjectMember("inline_size", type_shape.inline_size, Position::kFirst);
    GenerateObjectMember("alignment", type_shape.alignment);
    GenerateObjectMember("depth", type_shape.depth);
    GenerateObjectMember("max_handles", type_shape.max_handles);
    GenerateObjectMember("max_out_of_line", type_shape.max_out_of_line);
    GenerateObjectMember("has_padding", type_shape.has_padding);
    GenerateObjectMember("has_flexible_envelope", type_shape.has_flexible_envelope);
  });
}

void JSONGenerator::Generate(const FieldShape& field_shape) {
  GenerateObject([&]() {
    GenerateObjectMember("offset", field_shape.offset, Position::kFirst);
    GenerateObjectMember("padding", field_shape.padding);
  });
}

void JSONGenerator::Generate(const Union& value) {
  GenerateObject([&]() {
    GenerateDeclName(value.name);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    GenerateObjectMember("members", value.members);
    GenerateObjectMember("strict", value.strictness.value());
    GenerateObjectMember("resource", value.resourceness.value() == Resourceness::kResource);
    auto anon = value.name.as_anonymous();
    bool is_result = anon && anon->provenance == Name::Provenance::kGeneratedResultUnion;
    GenerateObjectMember("is_result", is_result);
    GenerateObjectMember("type_shape_v2", value.type_shape.value());
  });
}

void JSONGenerator::Generate(const Union::Member& value) {
  GenerateObject([&]() {
    GenerateObjectMember("ordinal", value.ordinal, Position::kFirst);
    GenerateObjectMember("name", value.name);
    GenerateTypeAndFromAlias(value.type_ctor.get());
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty()) {
      GenerateObjectMember("maybe_attributes", value.attributes);
    }
  });
}

void JSONGenerator::Generate(const Overlay& value) {
  GenerateObject([&]() {
    GenerateDeclName(value.name);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    GenerateObjectMember("members", value.members);
    ZX_ASSERT(value.strictness.value() == Strictness::kStrict);
    GenerateObjectMember("strict", value.strictness.value());
    ZX_ASSERT(value.resourceness.value() == Resourceness::kValue);
    GenerateObjectMember("resource", value.resourceness.value() == Resourceness::kResource);
    GenerateObjectMember("type_shape_v2", value.type_shape.value());
  });
}

void JSONGenerator::Generate(const Overlay::Member& value) {
  GenerateObject([&]() {
    GenerateObjectMember("ordinal", value.ordinal, Position::kFirst);
    GenerateObjectMember("name", value.name);
    GenerateTypeAndFromAlias(value.type_ctor.get());
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty()) {
      GenerateObjectMember("maybe_attributes", value.attributes);
    }
  });
}

void JSONGenerator::Generate(const LayoutInvocation& value) {
  GenerateObject([&]() {
    GenerateObjectMember("name", value.from_alias->name, Position::kFirst);
    GenerateObjectPunctuation(Position::kSubsequent);
    EmitObjectKey("args");

    // In preparation of template support, it is better to expose a
    // heterogeneous argument list to backends, rather than the currently
    // limited internal view.
    EmitArrayBegin();
    if (value.element_type_resolved) {
      Indent();
      EmitNewlineWithIndent();
      Generate(value.element_type_raw->layout.resolved().name());
      Outdent();
      EmitNewlineWithIndent();
    }
    EmitArrayEnd();

    GenerateObjectMember("nullable", value.nullability);

    if (value.size_raw)
      GenerateObjectMember("maybe_size", *value.size_raw);
  });
}

void JSONGenerator::Generate(const TypeConstructor& value) {
  GenerateObject([&]() {
    const auto* type = value.type;
    bool is_box = false;
    // TODO(https://fxbug.dev/42149402): We need to coerce client/server
    // ends into the same representation as P, request<P>; and box<S> into S?
    // For box, we just need to access the inner IdentifierType and the rest
    // mostly works (except for the correct value for nullability)
    if (type && type->kind == Type::Kind::kBox) {
      type = static_cast<const BoxType*>(type)->boxed_type;
      is_box = true;
    }
    const TransportSideType* server_end = nullptr;
    if (type && type->kind == Type::Kind::kTransportSide) {
      const auto* end_type = static_cast<const TransportSideType*>(type);
      if (end_type->end == TransportSide::kClient) {
        // for client ends, the partial_type_ctor name should be the protocol name
        // (since client_end:P is P in the old syntax)
        GenerateObjectMember("name", end_type->protocol_decl->name, Position::kFirst);
      } else {
        // for server ends, the partial_type_ctor name is just "request" (since
        // server_end:P is request<P> in the old syntax), and we also need to
        // emit the protocol "arg" below
        GenerateObjectMember("name", Name::CreateIntrinsic(nullptr, "request"), Position::kFirst);
        server_end = end_type;
      }
    } else {
      GenerateObjectMember("name", value.type ? value.type->name : value.layout.resolved().name(),
                           Position::kFirst);
    }
    GenerateObjectPunctuation(Position::kSubsequent);
    EmitObjectKey("args");
    const auto& invocation = value.resolved_params;

    // In preparation of template support, it is better to expose a
    // heterogeneous argument list to backends, rather than the currently
    // limited internal view.
    EmitArrayBegin();
    if (server_end || is_box || invocation.element_type_resolved) {
      Indent();
      EmitNewlineWithIndent();
      if (server_end) {
        // TODO(https://fxbug.dev/42149402): Because the JSON IR still uses request<P>
        // instead of server_end:P, we have to hardcode the P argument here.
        GenerateObject([&]() {
          GenerateObjectMember("name", server_end->protocol_decl->name, Position::kFirst);
          GenerateObjectPunctuation(Position::kSubsequent);
          EmitObjectKey("args");
          EmitArrayBegin();
          EmitArrayEnd();
          GenerateObjectMember("nullable", Nullability::kNonnullable);
        });
      } else if (is_box) {
        Generate(*invocation.boxed_type_raw);
      } else {
        Generate(*invocation.element_type_raw);
      }
      Outdent();
      EmitNewlineWithIndent();
    }
    EmitArrayEnd();

    if (value.type && value.type->kind == Type::Kind::kBox) {
      // invocation.nullability will always be non nullable, because users can't
      // specify optional on box. however, we need to output nullable in this case
      // in order to match the behavior for Struct?
      GenerateObjectMember("nullable", Nullability::kNullable);
    } else {
      GenerateObjectMember("nullable", invocation.nullability);
    }

    if (invocation.size_raw)
      GenerateObjectMember("maybe_size", *invocation.size_raw);
    if (invocation.rights_raw)
      GenerateObjectMember("handle_rights", *invocation.rights_raw);
  });
}

void JSONGenerator::Generate(const Alias& value) {
  GenerateObject([&]() {
    GenerateObjectMember("name", value.name, Position::kFirst);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    // TODO(https://fxbug.dev/42158155): Remove "partial_type_ctor".
    GenerateObjectMember("partial_type_ctor", value.partial_type_ctor);
    GenerateTypeAndFromAlias(value.partial_type_ctor.get());
  });
}

void JSONGenerator::Generate(const NewType& value) {
  GenerateObject([&]() {
    GenerateObjectMember("name", value.name, Position::kFirst);
    GenerateObjectMember("location", NameSpan(value.name));
    GenerateObjectMember("deprecated", value.availability.is_deprecated());
    if (!value.attributes->Empty())
      GenerateObjectMember("maybe_attributes", value.attributes);
    GenerateTypeAndFromAlias(value.type_ctor.get());
  });
}

void JSONGenerator::Generate(const Compilation::Dependency& dependency) {
  GenerateObject([&]() {
    GenerateObjectMember("name", dependency.library->name, Position::kFirst);
    GenerateExternalDeclarationsMember(dependency.declarations);
  });
}

void JSONGenerator::GenerateDeclarationsEntry(int count, const Name& name,
                                              std::string_view decl_kind) {
  if (count == 0) {
    Indent();
    EmitNewlineWithIndent();
  } else {
    EmitObjectSeparator();
  }
  EmitObjectKey(FullyQualifiedName(name));
  EmitString(decl_kind);
}

void JSONGenerator::GenerateDeclarationsMember(const Compilation::Declarations& declarations,
                                               Position position) {
  GenerateObjectPunctuation(position);
  EmitObjectKey("declarations");
  GenerateObject([&]() {
    int count = 0;
    for (const auto& decl : declarations.bits)
      GenerateDeclarationsEntry(count++, decl->name, "bits");

    for (const auto& decl : declarations.consts)
      GenerateDeclarationsEntry(count++, decl->name, "const");

    for (const auto& decl : declarations.enums)
      GenerateDeclarationsEntry(count++, decl->name, "enum");

    for (const auto& decl : declarations.resources)
      GenerateDeclarationsEntry(count++, decl->name, "experimental_resource");

    for (const auto& decl : declarations.protocols)
      GenerateDeclarationsEntry(count++, decl->name, "protocol");

    for (const auto& decl : declarations.services)
      GenerateDeclarationsEntry(count++, decl->name, "service");

    for (const auto& decl : declarations.structs)
      GenerateDeclarationsEntry(count++, decl->name, "struct");

    for (const auto& decl : declarations.tables)
      GenerateDeclarationsEntry(count++, decl->name, "table");

    for (const auto& decl : declarations.unions)
      GenerateDeclarationsEntry(count++, decl->name, "union");

    for (const auto& decl : declarations.overlays)
      GenerateDeclarationsEntry(count++, decl->name, "overlay");

    for (const auto& decl : declarations.aliases)
      GenerateDeclarationsEntry(count++, decl->name, "alias");

    for (const auto& decl : declarations.new_types)
      GenerateDeclarationsEntry(count++, decl->name, "new_type");
  });
}

void JSONGenerator::GenerateExternalDeclarationsEntry(
    int count, const Name& name, std::string_view decl_kind,
    std::optional<Resourceness> maybe_resourceness,
    const std::optional<TypeShape>& maybe_type_shape) {
  if (count == 0) {
    Indent();
    EmitNewlineWithIndent();
  } else {
    EmitObjectSeparator();
  }
  EmitObjectKey(FullyQualifiedName(name));
  GenerateObject([&]() {
    GenerateObjectMember("kind", decl_kind, Position::kFirst);
    if (maybe_resourceness) {
      GenerateObjectMember("resource", *maybe_resourceness == Resourceness::kResource);
    }
    if (maybe_type_shape) {
      GenerateObjectMember("type_shape_v2", *maybe_type_shape);
    }
  });
}

void JSONGenerator::GenerateExternalDeclarationsMember(
    const Compilation::Declarations& declarations, Position position) {
  GenerateObjectPunctuation(position);
  EmitObjectKey("declarations");
  GenerateObject([&]() {
    int count = 0;
    for (const auto& decl : declarations.bits) {
      GenerateExternalDeclarationsEntry(count++, decl->name, "bits", std::nullopt,
                                        decl->type_shape);
    }

    for (const auto& decl : declarations.consts) {
      GenerateExternalDeclarationsEntry(count++, decl->name, "const");
    }

    for (const auto& decl : declarations.enums) {
      GenerateExternalDeclarationsEntry(count++, decl->name, "enum", std::nullopt,
                                        decl->type_shape);
    }

    for (const auto& decl : declarations.resources) {
      GenerateExternalDeclarationsEntry(count++, decl->name, "experimental_resource");
    }

    for (const auto& decl : declarations.protocols) {
      GenerateExternalDeclarationsEntry(count++, decl->name, "protocol");
    }

    for (const auto& decl : declarations.services) {
      GenerateExternalDeclarationsEntry(count++, decl->name, "service");
    }

    for (const auto& decl : declarations.structs) {
      GenerateExternalDeclarationsEntry(count++, decl->name, "struct", decl->resourceness,
                                        decl->type_shape);
    }

    for (const auto& decl : declarations.tables) {
      GenerateExternalDeclarationsEntry(count++, decl->name, "table", decl->resourceness,
                                        decl->type_shape);
    }

    for (const auto& decl : declarations.unions) {
      GenerateExternalDeclarationsEntry(count++, decl->name, "union", decl->resourceness,
                                        decl->type_shape);
    }

    for (const auto& decl : declarations.overlays) {
      GenerateExternalDeclarationsEntry(count++, decl->name, "overlays", decl->resourceness,
                                        decl->type_shape);
    }

    for (const auto& decl : declarations.aliases) {
      GenerateExternalDeclarationsEntry(count++, decl->name, "alias");
    }

    for (const auto& decl : declarations.new_types) {
      GenerateExternalDeclarationsEntry(count++, decl->name, "new_type", std::nullopt,
                                        decl->type_shape);
    }
  });
}

std::ostringstream JSONGenerator::Produce() {
  ResetIndentLevel();
  GenerateObject([&]() {
    GenerateObjectMember("name", compilation_->library_name, Position::kFirst);
    GenerateObjectMember("platform", compilation_->platform->name());

    GenerateObjectPunctuation(Position::kSubsequent);
    EmitObjectKey("available");
    GenerateObject([&]() {
      auto position = Position::kFirst;
      for (auto& [platform, versions] : *compilation_->version_selection_) {
        GenerateObjectMember(platform.name(), versions, position);
        position = Position::kSubsequent;
      };
    });

    if (!compilation_->library_attributes->Empty()) {
      GenerateObjectMember("maybe_attributes", compilation_->library_attributes);
    }

    std::vector<std::string_view> active_experiments;
    for (auto& [name, flag] : kAllExperimentalFlags) {
      if (experimental_flags_.IsEnabled(flag))
        active_experiments.push_back(name);
    }
    GenerateObjectMember("experiments", active_experiments);

    GenerateObjectMember("library_dependencies", compilation_->direct_and_composed_dependencies);

    GenerateObjectMember("bits_declarations", compilation_->declarations.bits);
    GenerateObjectMember("const_declarations", compilation_->declarations.consts);
    GenerateObjectMember("enum_declarations", compilation_->declarations.enums);
    GenerateObjectMember("experimental_resource_declarations",
                         compilation_->declarations.resources);
    GenerateObjectMember("protocol_declarations", compilation_->declarations.protocols);
    GenerateObjectMember("service_declarations", compilation_->declarations.services);
    GenerateObjectMember("struct_declarations", compilation_->declarations.structs);
    GenerateObjectMember("external_struct_declarations", compilation_->external_structs);
    GenerateObjectMember("table_declarations", compilation_->declarations.tables);
    GenerateObjectMember("union_declarations", compilation_->declarations.unions);
    if (experimental_flags_.IsEnabled(ExperimentalFlag::kZxCTypes)) {
      GenerateObjectMember("overlay_declarations", compilation_->declarations.overlays);
    }
    GenerateObjectMember("alias_declarations", compilation_->declarations.aliases);
    GenerateObjectMember("new_type_declarations", compilation_->declarations.new_types);

    std::vector<std::string> declaration_order;
    declaration_order.reserve(compilation_->declaration_order.size());
    for (const auto decl : compilation_->declaration_order) {
      declaration_order.push_back(FullyQualifiedName(decl->name));
    }
    GenerateObjectMember("declaration_order", declaration_order);

    GenerateDeclarationsMember(compilation_->declarations);
  });
  GenerateEOF();

  return std::move(json_file_);
}

}  // namespace fidlc
