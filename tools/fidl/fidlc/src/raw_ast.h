// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_RAW_AST_H_
#define TOOLS_FIDL_FIDLC_SRC_RAW_AST_H_

#include <zircon/assert.h>

#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <variant>
#include <vector>

#include "tools/fidl/fidlc/src/properties.h"
#include "tools/fidl/fidlc/src/source_span.h"
#include "tools/fidl/fidlc/src/token.h"
#include "tools/fidl/fidlc/src/utils.h"

// ASTs fresh out of the oven. This is a tree-shaped bunch of nodes
// pretty much exactly corresponding to the grammar of a single fidl
// file. File is the root of the tree, and consists of lists of
// Declarations, and so on down to individual SourceSpans.
// See
// https://fuchsia.dev/fuchsia-src/development/languages/fidl/reference/compiler#compiler_internals
// for additional context

// Each node owns its children via unique_ptr and vector. All tokens
// here, like everywhere in the fidl compiler, are backed by a string
// view whose contents are owned by a SourceManager.

// This class has a tight coupling with the TreeVisitor class.  Each node has a
// corresponding method in that class.  Each node type also has an Accept()
// method to help visitors visit the node.  When you add a new node, or add a
// field to an existing node, you must ensure the Accept method works.

// A File is produced by parsing a token stream. All of the
// Files in a library are then flattened out into a Library.

namespace fidlc {

class TreeVisitor;

// A collection of one or more consecutive |Token|s from a single |SourceFile|.
// Each AST node has a SourceElement to keep track of where it came from.
struct SourceElement {
  SourceElement(Token start, Token end) : start_token(start), end_token(end) {}
  virtual ~SourceElement() = default;

  bool has_span() const {
    return start_token.span().valid() && end_token.span().valid() &&
           &start_token.span().source_file() == &end_token.span().source_file();
  }

  SourceSpan span() const {
    if (!start_token.span().valid() || !end_token.span().valid()) {
      return SourceSpan();
    }

    ZX_ASSERT(has_span());
    const char* start_pos = start_token.ptr();
    const char* end_pos = end_token.ptr() + end_token.data().length();
    return SourceSpan(std::string_view(start_pos, end_pos - start_pos),
                      start_token.span().source_file());
  }

  Token start_token;
  Token end_token;
};

// RAII helper class for calling OnSourceElementStart and OnSourceElementEnd.
class SourceElementMark {
 public:
  SourceElementMark(TreeVisitor* tv, const SourceElement& element);
  ~SourceElementMark();

 private:
  TreeVisitor* tv_;
  const SourceElement& element_;
};

struct RawIdentifier final : public SourceElement {
  explicit RawIdentifier(const SourceElement& element) : SourceElement(element) {}

  void Accept(TreeVisitor* visitor) const;
};

struct RawCompoundIdentifier final : public SourceElement {
  RawCompoundIdentifier(const SourceElement& element,
                        std::vector<std::unique_ptr<RawIdentifier>> components)
      : SourceElement(element), components(std::move(components)) {}

  void Accept(TreeVisitor* visitor) const;
  std::string ToString() const;

  std::vector<std::unique_ptr<RawIdentifier>> components;
};

struct RawLiteral : public SourceElement {
  enum class Kind : uint8_t {
    kBool,
    kDocComment,
    kNumeric,
    kString,
  };

  RawLiteral(const SourceElement& element, Kind kind) : SourceElement(element), kind(kind) {}

  const Kind kind;
};

struct RawDocCommentLiteral final : public RawLiteral {
  explicit RawDocCommentLiteral(const SourceElement& element)
      : RawLiteral(element, Kind::kDocComment),
        value(StripDocCommentSlashes(element.span().data())) {}

  void Accept(TreeVisitor* visitor) const;

  std::string value;
};

struct RawStringLiteral final : public RawLiteral {
  explicit RawStringLiteral(const SourceElement& element)
      : RawLiteral(element, Kind::kString),
        value(StripStringLiteralQuotes(element.span().data())) {}

  void Accept(TreeVisitor* visitor) const;

  std::string value;
};

struct RawNumericLiteral final : public RawLiteral {
  explicit RawNumericLiteral(const SourceElement& element) : RawLiteral(element, Kind::kNumeric) {}

  void Accept(TreeVisitor* visitor) const;
};

struct RawBoolLiteral final : public RawLiteral {
  RawBoolLiteral(const SourceElement& element, bool value)
      : RawLiteral(element, Kind::kBool), value(value) {}

  void Accept(TreeVisitor* visitor) const;

  const bool value;
};

struct RawOrdinal64 final : public SourceElement {
  RawOrdinal64(const SourceElement& element, uint64_t value)
      : SourceElement(element), value(value) {}

  void Accept(TreeVisitor* visitor) const;

  const uint64_t value;
};

struct RawConstant : public SourceElement {
  enum class Kind : uint8_t { kIdentifier, kLiteral, kBinaryOperator };

  RawConstant(Token start, Token end, Kind kind) : SourceElement(start, end), kind(kind) {}
  RawConstant(const SourceElement& element, Kind kind) : SourceElement(element), kind(kind) {}

  const Kind kind;
};

struct RawIdentifierConstant final : public RawConstant {
  explicit RawIdentifierConstant(std::unique_ptr<RawCompoundIdentifier> identifier)
      : RawConstant(SourceElement(identifier->start_token, identifier->end_token),
                    Kind::kIdentifier),
        identifier(std::move(identifier)) {}

  std::unique_ptr<RawCompoundIdentifier> identifier;

  void Accept(TreeVisitor* visitor) const;
};

struct RawLiteralConstant final : public RawConstant {
  explicit RawLiteralConstant(std::unique_ptr<RawLiteral> literal)
      : RawConstant(literal->start_token, literal->end_token, Kind::kLiteral),
        literal(std::move(literal)) {}

  std::unique_ptr<RawLiteral> literal;

  void Accept(TreeVisitor* visitor) const;
};

struct RawBinaryOperatorConstant final : public RawConstant {
  enum class Operator : uint8_t { kOr };
  explicit RawBinaryOperatorConstant(std::unique_ptr<RawConstant> left_operand,
                                     std::unique_ptr<RawConstant> right_operand, Operator op)
      : RawConstant(SourceElement(left_operand->start_token, right_operand->end_token),
                    Kind::kBinaryOperator),
        left_operand(std::move(left_operand)),
        right_operand(std::move(right_operand)),
        op(op) {}

  std::unique_ptr<RawConstant> left_operand;
  std::unique_ptr<RawConstant> right_operand;
  Operator op;

  void Accept(TreeVisitor* visitor) const;
};

struct RawAttributeArg final : public SourceElement {
  // Constructor for cases where the arg name has been explicitly defined in the text.
  RawAttributeArg(const SourceElement& element, std::unique_ptr<RawIdentifier> name,
                  std::unique_ptr<RawConstant> value)
      : SourceElement(element), maybe_name(std::move(name)), value(std::move(value)) {}

  // Constructor for cases where the arg name is inferred.
  RawAttributeArg(const SourceElement& element, std::unique_ptr<RawConstant> value)
      : SourceElement(element), maybe_name(nullptr), value(std::move(value)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawIdentifier> maybe_name;
  std::unique_ptr<RawConstant> value;
};

struct RawAttribute final : public SourceElement {
  enum class Provenance : uint8_t {
    // The normal attribute syntax, e.g. `@some_attribute`.
    kDefault,
    // A @doc attribute derived from doc comment syntax, e.g. `/// Documentation`.
    kDocComment,
    // An @available attribute derived from modifier change syntax, e.g. `flexible(added=5)`.
    kModifierAvailability,
  };

  RawAttribute(const SourceElement& element, Provenance provenance,
               std::unique_ptr<RawIdentifier> name,
               std::vector<std::unique_ptr<RawAttributeArg>> args)
      : SourceElement(element),
        provenance(provenance),
        maybe_name(std::move(name)),
        args(std::move(args)) {
    bool has_name = maybe_name != nullptr;
    bool is_default = provenance == Provenance::kDefault;
    ZX_ASSERT(has_name == is_default);
  }

  void Accept(TreeVisitor* visitor) const;

  Provenance provenance;
  std::unique_ptr<RawIdentifier> maybe_name;
  std::vector<std::unique_ptr<RawAttributeArg>> args;
};

// In the raw AST, "no attributes" is represented by a null AttributeList*,
// because every SourceElement must have a valid span. (In the flat AST, it is
// the opposite: never null, but the vector can be empty.)
struct RawAttributeList final : public SourceElement {
  RawAttributeList(const SourceElement& element,
                   std::vector<std::unique_ptr<RawAttribute>> attributes)
      : SourceElement(element), attributes(std::move(attributes)) {}

  void Accept(TreeVisitor* visitor) const;

  std::vector<std::unique_ptr<RawAttribute>> attributes;
};

struct RawLayoutReference : public SourceElement {
  enum class Kind : uint8_t {
    kInline,
    kNamed,
  };

  RawLayoutReference(const SourceElement& element, Kind kind)
      : SourceElement(element), kind(kind) {}

  void Accept(TreeVisitor* visitor) const;
  const Kind kind;
};

struct RawTypeConstraints final : public SourceElement {
  RawTypeConstraints(const SourceElement& element, std::vector<std::unique_ptr<RawConstant>> items)
      : SourceElement(element), items(std::move(items)) {}

  void Accept(TreeVisitor* visitor) const;

  std::vector<std::unique_ptr<RawConstant>> items;
};

struct RawLayoutParameter : public SourceElement {
  enum class Kind : uint8_t {
    kIdentifier,
    kLiteral,
    kType,
  };

  RawLayoutParameter(const SourceElement& element, Kind kind)
      : SourceElement(element), kind(kind) {}

  void Accept(TreeVisitor* visitor) const;

  const Kind kind;
};

struct RawLayoutParameterList final : public SourceElement {
  RawLayoutParameterList(const SourceElement& element,
                         std::vector<std::unique_ptr<RawLayoutParameter>> items)
      : SourceElement(element), items(std::move(items)) {}

  void Accept(TreeVisitor* visitor) const;

  std::vector<std::unique_ptr<RawLayoutParameter>> items;
};

struct RawTypeConstructor;

// The monostate variant is used to represent a parse failure.
using ConstraintOrSubtype = std::variant<std::unique_ptr<RawTypeConstraints>,
                                         std::unique_ptr<RawTypeConstructor>, std::monostate>;

struct RawTypeConstructor final : public SourceElement {
  RawTypeConstructor(const SourceElement& element, std::unique_ptr<RawLayoutReference> layout_ref,
                     std::unique_ptr<RawLayoutParameterList> parameters,
                     std::unique_ptr<RawTypeConstraints> constraints)
      : SourceElement(element),
        layout_ref(std::move(layout_ref)),
        parameters(std::move(parameters)),
        constraints(std::move(constraints)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawLayoutReference> layout_ref;
  std::unique_ptr<RawLayoutParameterList> parameters;
  std::unique_ptr<RawTypeConstraints> constraints;
};

struct RawAliasDeclaration final : public SourceElement {
  RawAliasDeclaration(const SourceElement& element, std::unique_ptr<RawAttributeList> attributes,
                      std::unique_ptr<RawIdentifier> alias,
                      std::unique_ptr<RawTypeConstructor> type_ctor)
      : SourceElement(element),
        attributes(std::move(attributes)),
        alias(std::move(alias)),
        type_ctor(std::move(type_ctor)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawAttributeList> attributes;
  std::unique_ptr<RawIdentifier> alias;
  std::unique_ptr<RawTypeConstructor> type_ctor;
};

struct RawLibraryDeclaration final : public SourceElement {
  RawLibraryDeclaration(const SourceElement& element, std::unique_ptr<RawAttributeList> attributes,
                        std::unique_ptr<RawCompoundIdentifier> path)
      : SourceElement(element), attributes(std::move(attributes)), path(std::move(path)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawAttributeList> attributes;
  std::unique_ptr<RawCompoundIdentifier> path;
};

struct RawUsing final : public SourceElement {
  RawUsing(const SourceElement& element, std::unique_ptr<RawAttributeList> attributes,
           std::unique_ptr<RawCompoundIdentifier> using_path,
           std::unique_ptr<RawIdentifier> maybe_alias)
      : SourceElement(element),
        attributes(std::move(attributes)),
        using_path(std::move(using_path)),
        maybe_alias(std::move(maybe_alias)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawAttributeList> attributes;
  std::unique_ptr<RawCompoundIdentifier> using_path;
  std::unique_ptr<RawIdentifier> maybe_alias;
};

struct RawConstDeclaration final : public SourceElement {
  RawConstDeclaration(const SourceElement& element, std::unique_ptr<RawAttributeList> attributes,
                      std::unique_ptr<RawTypeConstructor> type_ctor,
                      std::unique_ptr<RawIdentifier> identifier,
                      std::unique_ptr<RawConstant> constant)
      : SourceElement(element),
        attributes(std::move(attributes)),
        type_ctor(std::move(type_ctor)),
        identifier(std::move(identifier)),
        constant(std::move(constant)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawAttributeList> attributes;
  std::unique_ptr<RawTypeConstructor> type_ctor;
  std::unique_ptr<RawIdentifier> identifier;
  std::unique_ptr<RawConstant> constant;
};

// A single modifier applied to a layout, protocol, or method.
struct RawModifier final : SourceElement {
  RawModifier(const SourceElement& element, ModifierValue value, Token token,
              std::unique_ptr<RawAttribute> maybe_available_attribute)
      : SourceElement(element),
        value(value),
        token(token),
        maybe_available_attribute(std::move(maybe_available_attribute)) {}

  void Accept(TreeVisitor* visitor) const;

  // Value of the modifier
  ModifierValue value;
  // Token that the modifier is from.
  Token token;
  // Modifier availability, e.g. `resource(added=2)` or `strict(added=3, removed=5)`.
  std::unique_ptr<RawAttribute> maybe_available_attribute;
};

// In the raw AST, "no modifiers" is represented by a null ModifierList*,
// because every SourceElement must have a valid span. (In the flat AST, it is
// the opposite: never null, but the vector can be empty.)
struct RawModifierList final : public SourceElement {
  explicit RawModifierList(const SourceElement& element,
                           std::vector<std::unique_ptr<RawModifier>> modifiers)
      : SourceElement(element), modifiers(std::move(modifiers)) {}

  void Accept(TreeVisitor* visitor) const;

  std::vector<std::unique_ptr<RawModifier>> modifiers;
};

struct RawParameterList final : public SourceElement {
  RawParameterList(const SourceElement& element, std::unique_ptr<RawTypeConstructor> type_ctor)
      : SourceElement(element), type_ctor(std::move(type_ctor)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawTypeConstructor> type_ctor;
};

struct RawProtocolMethod : public SourceElement {
  RawProtocolMethod(const SourceElement& element, std::unique_ptr<RawAttributeList> attributes,
                    std::unique_ptr<RawModifierList> modifiers,
                    std::unique_ptr<RawIdentifier> identifier,
                    std::unique_ptr<RawParameterList> maybe_request,
                    std::unique_ptr<RawParameterList> maybe_response,
                    std::unique_ptr<RawTypeConstructor> maybe_error_ctor)
      : SourceElement(element),
        attributes(std::move(attributes)),
        modifiers(std::move(modifiers)),
        identifier(std::move(identifier)),
        maybe_request(std::move(maybe_request)),
        maybe_response(std::move(maybe_response)),
        maybe_error_ctor(std::move(maybe_error_ctor)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawAttributeList> attributes;
  std::unique_ptr<RawModifierList> modifiers;
  std::unique_ptr<RawIdentifier> identifier;
  std::unique_ptr<RawParameterList> maybe_request;
  std::unique_ptr<RawParameterList> maybe_response;
  std::unique_ptr<RawTypeConstructor> maybe_error_ctor;
};

struct RawProtocolCompose final : public SourceElement {
  RawProtocolCompose(const SourceElement& element, std::unique_ptr<RawAttributeList> attributes,
                     std::unique_ptr<RawCompoundIdentifier> protocol_name)
      : SourceElement(element),
        attributes(std::move(attributes)),
        protocol_name(std::move(protocol_name)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawAttributeList> attributes;
  std::unique_ptr<RawCompoundIdentifier> protocol_name;
};

struct RawProtocolDeclaration final : public SourceElement {
  RawProtocolDeclaration(const SourceElement& element, std::unique_ptr<RawAttributeList> attributes,
                         std::unique_ptr<RawModifierList> modifiers,
                         std::unique_ptr<RawIdentifier> identifier,
                         std::vector<std::unique_ptr<RawProtocolCompose>> composed_protocols,
                         std::vector<std::unique_ptr<RawProtocolMethod>> methods)
      : SourceElement(element),
        attributes(std::move(attributes)),
        modifiers(std::move(modifiers)),
        identifier(std::move(identifier)),
        composed_protocols(std::move(composed_protocols)),
        methods(std::move(methods)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawAttributeList> attributes;
  std::unique_ptr<RawModifierList> modifiers;
  std::unique_ptr<RawIdentifier> identifier;
  std::vector<std::unique_ptr<RawProtocolCompose>> composed_protocols;
  std::vector<std::unique_ptr<RawProtocolMethod>> methods;
};

struct RawResourceProperty final : public SourceElement {
  RawResourceProperty(const SourceElement& element, std::unique_ptr<RawTypeConstructor> type_ctor,
                      std::unique_ptr<RawIdentifier> identifier,
                      std::unique_ptr<RawAttributeList> attributes)
      : SourceElement(element),
        type_ctor(std::move(type_ctor)),
        identifier(std::move(identifier)),
        attributes(std::move(attributes)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawTypeConstructor> type_ctor;
  std::unique_ptr<RawIdentifier> identifier;
  std::unique_ptr<RawAttributeList> attributes;
};

struct RawResourceDeclaration final : public SourceElement {
  RawResourceDeclaration(const SourceElement& element, std::unique_ptr<RawAttributeList> attributes,
                         std::unique_ptr<RawIdentifier> identifier,
                         std::unique_ptr<RawTypeConstructor> maybe_type_ctor,
                         std::vector<std::unique_ptr<RawResourceProperty>> properties)
      : SourceElement(element),
        attributes(std::move(attributes)),
        identifier(std::move(identifier)),
        maybe_type_ctor(std::move(maybe_type_ctor)),
        properties(std::move(properties)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawAttributeList> attributes;
  std::unique_ptr<RawIdentifier> identifier;
  std::unique_ptr<RawTypeConstructor> maybe_type_ctor;
  std::vector<std::unique_ptr<RawResourceProperty>> properties;
};

struct RawServiceMember final : public SourceElement {
  RawServiceMember(const SourceElement& element, std::unique_ptr<RawTypeConstructor> type_ctor,
                   std::unique_ptr<RawIdentifier> identifier,
                   std::unique_ptr<RawAttributeList> attributes)
      : SourceElement(element),
        type_ctor(std::move(type_ctor)),
        identifier(std::move(identifier)),
        attributes(std::move(attributes)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawTypeConstructor> type_ctor;
  std::unique_ptr<RawIdentifier> identifier;
  std::unique_ptr<RawAttributeList> attributes;
};

struct RawServiceDeclaration final : public SourceElement {
  RawServiceDeclaration(const SourceElement& element, std::unique_ptr<RawAttributeList> attributes,
                        std::unique_ptr<RawIdentifier> identifier,
                        std::vector<std::unique_ptr<RawServiceMember>> members)
      : SourceElement(element),
        attributes(std::move(attributes)),
        identifier(std::move(identifier)),
        members(std::move(members)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawAttributeList> attributes;
  std::unique_ptr<RawIdentifier> identifier;
  std::vector<std::unique_ptr<RawServiceMember>> members;
};

struct RawLayoutMember : public SourceElement {
  enum class Kind : uint8_t {
    kOrdinaled,
    kStruct,
    kValue,
  };

  RawLayoutMember(const SourceElement& element, Kind kind,
                  std::unique_ptr<RawAttributeList> attributes,
                  std::unique_ptr<RawIdentifier> identifier)
      : SourceElement(element),
        kind(kind),
        attributes(std::move(attributes)),
        identifier(std::move(identifier)) {}

  void Accept(TreeVisitor* visitor) const;

  const Kind kind;
  std::unique_ptr<RawAttributeList> attributes;
  std::unique_ptr<RawIdentifier> identifier;
};

struct RawLayout final : public SourceElement {
  enum class Kind : uint8_t {
    kBits,
    kEnum,
    kStruct,
    kTable,
    kUnion,
    kOverlay,
  };

  RawLayout(const SourceElement& element, Kind kind,
            std::vector<std::unique_ptr<RawLayoutMember>> members,
            std::unique_ptr<RawModifierList> modifiers,
            std::unique_ptr<RawTypeConstructor> subtype_ctor)
      : SourceElement(element),
        kind(kind),
        members(std::move(members)),
        modifiers(std::move(modifiers)),
        subtype_ctor(std::move(subtype_ctor)) {}

  void Accept(TreeVisitor* visitor) const;

  Kind kind;
  std::vector<std::unique_ptr<RawLayoutMember>> members;
  std::unique_ptr<RawModifierList> modifiers;
  // Only used for Kind::kBits and Kind::kEnum.
  std::unique_ptr<RawTypeConstructor> subtype_ctor;
};

struct RawOrdinaledLayoutMember final : public RawLayoutMember {
  RawOrdinaledLayoutMember(const SourceElement& element,
                           std::unique_ptr<RawAttributeList> attributes,
                           std::unique_ptr<RawOrdinal64> ordinal,
                           std::unique_ptr<RawIdentifier> identifier,
                           std::unique_ptr<RawTypeConstructor> type_ctor)
      : RawLayoutMember(element, Kind::kOrdinaled, std::move(attributes), std::move(identifier)),
        ordinal(std::move(ordinal)),
        type_ctor(std::move(type_ctor)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawOrdinal64> ordinal;
  std::unique_ptr<RawTypeConstructor> type_ctor;
};

struct RawValueLayoutMember final : public RawLayoutMember {
  RawValueLayoutMember(const SourceElement& element, std::unique_ptr<RawAttributeList> attributes,
                       std::unique_ptr<RawIdentifier> identifier,
                       std::unique_ptr<RawConstant> value)
      : RawLayoutMember(element, Kind::kValue, std::move(attributes), std::move(identifier)),
        value(std::move(value)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawConstant> value;
};

struct RawStructLayoutMember final : public RawLayoutMember {
  RawStructLayoutMember(const SourceElement& element, std::unique_ptr<RawAttributeList> attributes,
                        std::unique_ptr<RawIdentifier> identifier,
                        std::unique_ptr<RawTypeConstructor> type_ctor,
                        std::unique_ptr<RawConstant> default_value)
      : RawLayoutMember(element, Kind::kStruct, std::move(attributes), std::move(identifier)),
        type_ctor(std::move(type_ctor)),
        default_value(std::move(default_value)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawTypeConstructor> type_ctor;
  std::unique_ptr<RawConstant> default_value;
};

struct RawInlineLayoutReference final : public RawLayoutReference {
  RawInlineLayoutReference(const SourceElement& element,
                           std::unique_ptr<RawAttributeList> attributes,
                           std::unique_ptr<RawLayout> layout)
      : RawLayoutReference(element, Kind::kInline),
        attributes(std::move(attributes)),
        layout(std::move(layout)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawAttributeList> attributes;
  std::unique_ptr<RawLayout> layout;
};

struct RawNamedLayoutReference final : public RawLayoutReference {
  RawNamedLayoutReference(const SourceElement& element,
                          std::unique_ptr<RawCompoundIdentifier> identifier)
      : RawLayoutReference(element, Kind::kNamed), identifier(std::move(identifier)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawCompoundIdentifier> identifier;
};

struct RawLiteralLayoutParameter final : public RawLayoutParameter {
  RawLiteralLayoutParameter(const SourceElement& element,
                            std::unique_ptr<RawLiteralConstant> literal)
      : RawLayoutParameter(element, Kind::kLiteral), literal(std::move(literal)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawLiteralConstant> literal;
};

struct RawTypeLayoutParameter final : public RawLayoutParameter {
  RawTypeLayoutParameter(const SourceElement& element,
                         std::unique_ptr<RawTypeConstructor> type_ctor)
      : RawLayoutParameter(element, Kind::kType), type_ctor(std::move(type_ctor)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawTypeConstructor> type_ctor;
};

struct RawIdentifierLayoutParameter final : public RawLayoutParameter {
  RawIdentifierLayoutParameter(const SourceElement& element,
                               std::unique_ptr<RawCompoundIdentifier> identifier)
      : RawLayoutParameter(element, Kind::kIdentifier), identifier(std::move(identifier)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawCompoundIdentifier> identifier;
};

struct RawTypeDeclaration final : public SourceElement {
  RawTypeDeclaration(const SourceElement& element, std::unique_ptr<RawAttributeList> attributes,
                     std::unique_ptr<RawIdentifier> identifier,
                     std::unique_ptr<RawTypeConstructor> type_ctor)
      : SourceElement(element),
        attributes(std::move(attributes)),
        identifier(std::move(identifier)),
        type_ctor(std::move(type_ctor)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawAttributeList> attributes;
  std::unique_ptr<RawIdentifier> identifier;
  std::unique_ptr<RawTypeConstructor> type_ctor;
};

struct File final : public SourceElement {
  File(const SourceElement& element, std::unique_ptr<RawLibraryDeclaration> library_decl,
       std::vector<std::unique_ptr<RawAliasDeclaration>> alias_list,
       std::vector<std::unique_ptr<RawUsing>> using_list,
       std::vector<std::unique_ptr<RawConstDeclaration>> const_declaration_list,
       std::vector<std::unique_ptr<RawProtocolDeclaration>> protocol_declaration_list,
       std::vector<std::unique_ptr<RawResourceDeclaration>> resource_declaration_list,
       std::vector<std::unique_ptr<RawServiceDeclaration>> service_declaration_list,
       std::vector<std::unique_ptr<RawTypeDeclaration>> type_decls, std::vector<Token> tokens)
      : SourceElement(element),
        library_decl(std::move(library_decl)),
        alias_list(std::move(alias_list)),
        using_list(std::move(using_list)),
        const_declaration_list(std::move(const_declaration_list)),
        protocol_declaration_list(std::move(protocol_declaration_list)),
        resource_declaration_list(std::move(resource_declaration_list)),
        service_declaration_list(std::move(service_declaration_list)),
        type_decls(std::move(type_decls)),
        tokens(std::move(tokens)) {}

  void Accept(TreeVisitor* visitor) const;

  std::unique_ptr<RawLibraryDeclaration> library_decl;
  std::vector<std::unique_ptr<RawAliasDeclaration>> alias_list;
  std::vector<std::unique_ptr<RawUsing>> using_list;
  std::vector<std::unique_ptr<RawConstDeclaration>> const_declaration_list;
  std::vector<std::unique_ptr<RawProtocolDeclaration>> protocol_declaration_list;
  std::vector<std::unique_ptr<RawResourceDeclaration>> resource_declaration_list;
  std::vector<std::unique_ptr<RawServiceDeclaration>> service_declaration_list;
  std::vector<std::unique_ptr<RawTypeDeclaration>> type_decls;

  // An ordered list of all tokens (including comments) in the source file.
  std::vector<Token> tokens;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_RAW_AST_H_
