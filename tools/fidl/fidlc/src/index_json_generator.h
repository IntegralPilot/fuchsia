// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_INDEX_JSON_GENERATOR_H_
#define TOOLS_FIDL_FIDLC_SRC_INDEX_JSON_GENERATOR_H_

#include <zircon/assert.h>

#include <sstream>
#include <string>
#include <vector>

#include "tools/fidl/fidlc/src/compiler.h"
#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/json_writer.h"
#include "tools/fidl/fidlc/src/names.h"

namespace fidlc {

class IndexJSONGenerator : public JsonWriter<IndexJSONGenerator> {
 public:
  // "using" is required for overridden methods, so the implementations in
  // both the base class and in this derived class are visible when matching
  // parameter types
  using JsonWriter<IndexJSONGenerator>::Generate;
  using JsonWriter<IndexJSONGenerator>::GenerateArray;

  explicit IndexJSONGenerator(const Compilation* compilation)
      : JsonWriter(json_file_), compilation_(compilation) {}

  ~IndexJSONGenerator() = default;

  // struct representing an identifier from dependency library referenced in target library
  struct ReferencedIdentifier {
    explicit ReferencedIdentifier(const Name& name) : identifier(FullyQualifiedName(name)) {
      ZX_ASSERT_MSG(name.span().has_value(), "anonymous name used as an identifier");
      span = name.span().value();
    }
    ReferencedIdentifier(std::string identifier, SourceSpan span)
        : span(span), identifier(std::move(identifier)) {}
    SourceSpan span;
    std::string identifier;
  };

  void Generate(SourceSpan value);
  void Generate(ReferencedIdentifier value);
  void Generate(const Compilation::Dependency& dependency);
  void Generate(std::pair<Library*, SourceSpan> reference);
  void Generate(const Const& value);
  void Generate(const Constant& value);
  void Generate(const Enum& value);
  void Generate(const Enum::Member& value);
  void Generate(const Name& name);
  void Generate(const Struct& value);
  void Generate(const Struct::Member& value);
  void Generate(const TypeConstructor* value);
  void Generate(const Protocol& value);
  void Generate(const Protocol::ComposedProtocol& value);
  void Generate(const Protocol::MethodWithInfo& method_with_info);
  void Generate(const Union& value);
  void Generate(const Union::Member& value);
  void Generate(const Table& value);
  void Generate(const Table::Member& value);

  std::ostringstream Produce();

 private:
  std::vector<ReferencedIdentifier> GetDependencyIdentifiers();
  const Compilation* compilation_;
  std::ostringstream json_file_;
};
}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_INDEX_JSON_GENERATOR_H_
