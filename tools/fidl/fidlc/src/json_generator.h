// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_JSON_GENERATOR_H_
#define TOOLS_FIDL_FIDLC_SRC_JSON_GENERATOR_H_

#include <zircon/assert.h>

#include <sstream>
#include <string>
#include <string_view>

#include "tools/fidl/fidlc/src/compiler.h"
#include "tools/fidl/fidlc/src/experimental_flags.h"
#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/json_writer.h"
#include "tools/fidl/fidlc/src/type_shape.h"

namespace fidlc {

struct NameSpan {
  explicit NameSpan(const SourceSpan& span)
      : filename(span.source_file().filename()), length(span.data().length()) {
    span.SourceLine(&position);
  }

  explicit NameSpan(const Name& name) : NameSpan(name.span().value()) {
    ZX_ASSERT_MSG(!name.is_intrinsic(), "must not use NameSpan on intrinsic names");
  }

  const std::string filename;
  SourceFile::Position position;
  const size_t length;
};

// Methods or functions named "Emit..." are the actual interface to
// the JSON output.

// Methods named "Generate..." directly generate JSON output via the
// "Emit" routines.

// Methods named "Produce..." indirectly generate JSON output by calling
// the Generate methods, and should not call the "Emit" functions
// directly.

// |JsonWriter| requires the derived type as a template parameter so it can
// match methods declared with parameter overrides in the derived class.
class JSONGenerator : public JsonWriter<JSONGenerator> {
 public:
  // "using" is required for overridden methods, so the implementations in
  // both the base class and in this derived class are visible when matching
  // parameter types
  using JsonWriter<JSONGenerator>::Generate;
  using JsonWriter<JSONGenerator>::GenerateArray;

  JSONGenerator(const Compilation* compilation, ExperimentalFlagSet experimental_flags)
      : JsonWriter(json_file_),
        compilation_(compilation),
        experimental_flags_(experimental_flags) {}

  ~JSONGenerator() = default;

  std::ostringstream Produce();

  void Generate(Version version);

  void Generate(SourceSpan value);
  void Generate(NameSpan value);

  void Generate(HandleSubtype value);
  void Generate(Nullability value);
  void Generate(Strictness value);
  void Generate(Openness value);
  void Generate(TransportSide value);

  void Generate(const RawIdentifier& value);
  void Generate(const AttributeArg& value);
  void Generate(const Attribute& value);
  void Generate(const AttributeList& value);
  void Generate(const RawOrdinal64& value);

  void Generate(const TypeShape& type_shape);
  void Generate(const FieldShape& field_shape);

  void GenerateDeclName(const Name& name);
  void Generate(const Name& name);
  void Generate(const Type* value);
  void Generate(const Constant& value);
  void Generate(const ConstantValue& value);
  void Generate(const Bits& value);
  void Generate(const Bits::Member& value);
  void Generate(const Const& value);
  void Generate(const Enum& value);
  void Generate(const Enum::Member& value);
  void Generate(const NewType& value);
  void Generate(const Protocol& value);
  void Generate(const Protocol::ComposedProtocol& value);
  void Generate(const Protocol::MethodWithInfo& method_with_info);
  void Generate(const LiteralConstant& value);
  void Generate(const Resource& value);
  void Generate(const Resource::Property& value);
  void Generate(const Service& value);
  void Generate(const Service::Member& value);
  void Generate(const Struct& value);
  void Generate(const Struct::Member& value);
  void Generate(const Table& value);
  void Generate(const Table::Member& value);
  void Generate(const Union& value);
  void Generate(const Union::Member& value);
  void Generate(const Overlay& value);
  void Generate(const Overlay::Member& value);
  void Generate(const LayoutInvocation& value);
  void Generate(const TypeConstructor& value);
  void Generate(const Alias& value);
  void Generate(const Compilation::Dependency& dependency);

 private:
  enum TypeKind : uint8_t {
    kConcrete,
    kParameterized,
    kRequestPayload,
    kResponsePayload,
  };
  void GenerateTypeAndFromAlias(TypeKind parent_type_kind, const TypeConstructor* value,
                                Position position = Position::kSubsequent);
  void GenerateTypeAndFromAlias(const TypeConstructor* value,
                                Position position = Position::kSubsequent);

  // This is a generator for the builtin generics: array, vector, and request.
  // The "type" argument is the resolved type of the parameterized type to be
  // generated, and the "type_ctor" argument is the de-aliased constructor for
  // that type.  For example, consider the following FIDL
  //
  //   alias Foo = vector<bool>:5;
  //
  //   struct Example {
  //     bar Foo?;
  //   };
  //
  // When GenerateParameterizedType is called for Example.bar, the "type" will
  // be a nullable vector of size 5, but the de-aliased constructor passed in
  // will be the underlying type for just Foo, in this is case "vector<bool:5>."
  void GenerateParameterizedType(TypeKind parent_type_kind, const Type* type,
                                 const TypeConstructor* type_ctor,
                                 Position position = Position::kSubsequent);
  void GenerateExperimentalMaybeFromAlias(const LayoutInvocation& invocation);
  void GenerateDeclarationsEntry(int count, const Name& name, std::string_view decl_kind);
  void GenerateDeclarationsMember(const Compilation::Declarations& declarations,
                                  Position position = Position::kSubsequent);
  void GenerateExternalDeclarationsEntry(
      int count, const Name& name, std::string_view decl_kind,
      std::optional<Resourceness> maybe_resourceness = std::nullopt,
      const std::optional<TypeShape>& maybe_type_shape = std::nullopt);
  void GenerateExternalDeclarationsMember(const Compilation::Declarations& declarations,
                                          Position position = Position::kSubsequent);
  void GenerateProtocolImplementationLocations(const Protocol& value);
  void GenerateEndpointImplementationLocations(const std::string_view& endpoint,
                                               const Attribute* discoverable, Position position);
  const Compilation* compilation_;
  ExperimentalFlagSet experimental_flags_;
  std::ostringstream json_file_;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_JSON_GENERATOR_H_
