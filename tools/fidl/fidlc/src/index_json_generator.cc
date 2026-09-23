// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/index_json_generator.h"

#include <zircon/assert.h>

#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/name.h"
#include "tools/fidl/fidlc/src/names.h"
#include "tools/fidl/fidlc/src/types.h"

namespace fidlc {

std::ostringstream IndexJSONGenerator::Produce() {
  ResetIndentLevel();
  GenerateObject([&]() {
    GenerateObjectMember("name", compilation_->library_name, Position::kFirst);
    GenerateObjectMember("lib_declarations", compilation_->library_declarations);
    GenerateObjectMember("using_declarations", compilation_->using_references);
    GenerateObjectMember("dependencies", compilation_->direct_and_composed_dependencies);
    GenerateObjectMember("dependency_identifiers", IndexJSONGenerator::GetDependencyIdentifiers());
    GenerateObjectMember("consts", compilation_->declarations.consts);
    GenerateObjectMember("enums", compilation_->declarations.enums);
    GenerateObjectMember("unions", compilation_->declarations.unions);
    GenerateObjectMember("tables", compilation_->declarations.tables);
    GenerateObjectMember("structs", compilation_->declarations.structs);
    GenerateObjectMember("protocols", compilation_->declarations.protocols);
  });
  GenerateEOF();

  return std::move(json_file_);
}

std::vector<IndexJSONGenerator::ReferencedIdentifier>
IndexJSONGenerator::GetDependencyIdentifiers() {
  std::vector<IndexJSONGenerator::ReferencedIdentifier> identifiers;
  for (auto& dependency : compilation_->direct_and_composed_dependencies) {
    for (auto& constdecl : dependency.declarations.consts) {
      auto identifier = IndexJSONGenerator::ReferencedIdentifier(constdecl->name);
      identifiers.emplace_back(identifier);
    }
    for (auto& enumdecl : dependency.declarations.enums) {
      auto identifier = IndexJSONGenerator::ReferencedIdentifier(enumdecl->name);
      identifiers.emplace_back(identifier);
      for (auto& member : enumdecl->members) {
        Name full_name = enumdecl->name.WithMemberName(std::string(member.name.data()));
        auto member_identifier =
            IndexJSONGenerator::ReferencedIdentifier(FullyQualifiedName(full_name), member.name);
        identifiers.emplace_back(member_identifier);
      }
    }
    for (auto& structdecl : dependency.declarations.structs) {
      auto identifier = IndexJSONGenerator::ReferencedIdentifier(structdecl->name);
      identifiers.emplace_back(identifier);
    }
    for (auto& protocoldecl : dependency.declarations.protocols) {
      auto identifier = IndexJSONGenerator::ReferencedIdentifier(protocoldecl->name);
      identifiers.emplace_back(identifier);
    }
    for (auto& uniondecl : dependency.declarations.unions) {
      auto identifier = IndexJSONGenerator::ReferencedIdentifier(uniondecl->name);
      identifiers.emplace_back(identifier);
    }
    for (auto& tabledecl : dependency.declarations.tables) {
      auto identifier = IndexJSONGenerator::ReferencedIdentifier(tabledecl->name);
      identifiers.emplace_back(identifier);
    }
  }
  return identifiers;
}

void IndexJSONGenerator::Generate(SourceSpan value) {
  GenerateObject([&]() {
    GenerateObjectMember("is_virtual", value.source_file().IsVirtual(), Position::kFirst);
    // In the case file is generated, we ignore the location for now
    if (!value.source_file().IsVirtual()) {
      GenerateObjectMember("file", value.source_file().filename());
      GenerateObjectMember("data", value.data());
      uint64_t start_offset = value.data().data() - value.source_file().data().data();
      GenerateObjectMember("start_offset", start_offset);
      GenerateObjectMember("end_offset", start_offset + value.data().size());
    }
  });
}

void IndexJSONGenerator::Generate(IndexJSONGenerator::ReferencedIdentifier value) {
  GenerateObject([&]() {
    GenerateObjectMember("identifier", value.identifier, Position::kFirst);
    GenerateObjectMember("location", value.span);
  });
}

void IndexJSONGenerator::Generate(std::pair<Library*, SourceSpan> reference) {
  GenerateObject([&]() {
    // for debugging purpose, include the span data
    GenerateObjectMember("library_name", reference.first->name, Position::kFirst);
    GenerateObjectMember("referenced_at", reference.second);
  });
}

void IndexJSONGenerator::Generate(const Compilation::Dependency& dependency) {
  GenerateObject([&]() {
    GenerateObjectMember("library_name", dependency.library->name, Position::kFirst);
    GenerateObjectMember("library_location", dependency.library->name_spans[0]);
  });
}

void IndexJSONGenerator::Generate(const Constant& value) {
  GenerateObject([&]() {
    GenerateObjectMember("type", NameConstantKind(value.kind), Position::kFirst);
    switch (value.kind) {
      case Constant::Kind::kIdentifier: {
        auto identifier = static_cast<const IdentifierConstant*>(&value);
        GenerateObjectMember("identifier",
                             FullyQualifiedName((identifier->reference.resolved().name())));
        GenerateObjectMember("referenced_at", identifier->reference.span());
        break;
      }
      case Constant::Kind::kLiteral: {
        // No need to record literal values
        break;
      }
      case Constant::Kind::kBinaryOperator: {
        auto binary_operator_constant = static_cast<const BinaryOperatorConstant*>(&value);
        GenerateObjectMember("lhs", binary_operator_constant->left_operand);
        GenerateObjectMember("rhs", binary_operator_constant->right_operand);
        break;
      }
    }
  });
}

void IndexJSONGenerator::Generate(const Const& value) {
  GenerateObject([&]() {
    GenerateObjectMember("identifier", FullyQualifiedName(value.name), Position::kFirst);
    GenerateObjectMember("location", value.name.span().value());
    GenerateObjectMember("value", value.value);
  });
}

void IndexJSONGenerator::Generate(const Name& name) { Generate(FullyQualifiedName(name)); }

void IndexJSONGenerator::Generate(const Enum& value) {
  GenerateObject([&]() {
    GenerateObjectMember("is_anonymous", value.IsAnonymousLayout(), Position::kFirst);
    GenerateObjectMember("identifier", value.name);
    // Only generate identifier location if the enum is not anonymous
    if (!value.IsAnonymousLayout()) {
      GenerateObjectMember("location", value.name.span().value());
    };
    GenerateObjectMember("members", value.members);
  });
}

void IndexJSONGenerator::Generate(const Enum::Member& value) {
  GenerateObject([&]() {
    GenerateObjectMember("name", value.name.data(), Position::kFirst);
    GenerateObjectMember("location", value.name);
    GenerateObjectMember("value", value.value);
  });
}

void IndexJSONGenerator::Generate(const Union& value) {
  GenerateObject([&]() {
    GenerateObjectMember("is_anonymous", value.IsAnonymousLayout(), Position::kFirst);
    if (!value.IsAnonymousLayout()) {
      GenerateObjectMember("identifier", value.name);
      GenerateObjectMember("location", value.name.span().value());
    }
    GenerateObjectMember("members", value.members);
  });
}

void IndexJSONGenerator::Generate(const Union::Member& value) {
  GenerateObject([&]() {
    GenerateObjectMember("is_reserved", false, Position::kFirst);
    GenerateObjectMember("name", value.name.data());
    GenerateObjectMember("location", value.name);
    GenerateObjectMember("type", value.type_ctor.get());
  });
}

void IndexJSONGenerator::Generate(const Table& value) {
  GenerateObject([&]() {
    GenerateObjectMember("is_anonymous", value.IsAnonymousLayout(), Position::kFirst);
    if (!value.IsAnonymousLayout()) {
      GenerateObjectMember("identifier", value.name);
      GenerateObjectMember("location", value.name.span().value());
    }
    GenerateObjectMember("members", value.members);
  });
}

void IndexJSONGenerator::Generate(const Table::Member& value) {
  GenerateObject([&]() {
    GenerateObjectMember("is_reserved", false, Position::kFirst);
    GenerateObjectMember("name", value.name.data());
    GenerateObjectMember("location", value.name);
    GenerateObjectMember("type", value.type_ctor.get());
  });
}

void IndexJSONGenerator::Generate(const Struct& value) {
  GenerateObject([&]() {
    GenerateObjectMember("is_anonymous", value.IsAnonymousLayout(), Position::kFirst);
    GenerateObjectMember("identifier", value.name);
    if (!value.IsAnonymousLayout()) {
      GenerateObjectMember("location", value.name.span().value());
    }
    GenerateObjectMember("members", value.members);
  });
}

void IndexJSONGenerator::Generate(const Struct::Member& value) {
  GenerateObject([&]() {
    GenerateObjectMember("name", value.name.data(), Position::kFirst);
    GenerateObjectMember("location", value.name);
    GenerateObjectMember("type", value.type_ctor.get());
  });
}

void IndexJSONGenerator::Generate(const TypeConstructor* value) {
  auto type = value->type;
  GenerateObject([&]() {
    GenerateObjectMember("kind", NameTypeKind(type), Position::kFirst);
    // handle the non anonymous type identifier case only for now
    // parameterized types (arrays, vectors) are not handled yet
    if (type->kind == Type::Kind::kIdentifier) {
      GenerateObjectMember("is_anonymous", type->name.as_anonymous() != nullptr);
      const auto* identifier = static_cast<const IdentifierType*>(type);
      if (!type->name.as_anonymous()) {
        GenerateObjectMember("type_identifier", identifier->name);
        GenerateObjectMember("type_referenced_at", value->layout.span());
      }
    }
  });
}

void IndexJSONGenerator::Generate(const Protocol& value) {
  GenerateObject([&]() {
    GenerateObjectMember("identifier", value.name, Position::kFirst);
    GenerateObjectMember("location", value.name.span().value());
    GenerateObjectMember("methods", value.all_methods);
    GenerateObjectMember("composed_protocols", value.composed_protocols);
  });
}

void IndexJSONGenerator::Generate(const Protocol::ComposedProtocol& value) {
  GenerateObject([&]() {
    GenerateObjectMember("identifier", value.reference.resolved().name(), Position::kFirst);
    GenerateObjectMember("referenced_at", value.reference.span());
  });
}

void IndexJSONGenerator::Generate(const Protocol::MethodWithInfo& method_with_info) {
  const auto& value = *method_with_info.method;
  GenerateObject([&]() {
    GenerateObjectMember("identifier", value.name.data(), Position::kFirst);
    GenerateObjectMember("location", value.name);
    if (value.maybe_request) {
      GenerateObjectMember("request_type", value.maybe_request.get());
    }
    if (value.maybe_response) {
      GenerateObjectMember("response_type", value.maybe_response.get());
    }
  });
}

}  // namespace fidlc
