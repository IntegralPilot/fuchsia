// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// See https://fuchsia.dev/fuchsia-src/development/languages/fidl/reference/compiler#compilation
// for documentation

#ifndef TOOLS_FIDL_FIDLC_SRC_FLAT_AST_H_
#define TOOLS_FIDL_FIDLC_SRC_FLAT_AST_H_

#include <lib/fit/function.h>
#include <zircon/assert.h>

#include <cstdint>
#include <map>
#include <optional>
#include <set>
#include <string_view>
#include <utility>
#include <vector>

#include "tools/fidl/fidlc/src/attributes.h"
#include "tools/fidl/fidlc/src/name.h"
#include "tools/fidl/fidlc/src/properties.h"
#include "tools/fidl/fidlc/src/type_shape.h"
#include "tools/fidl/fidlc/src/types.h"
#include "tools/fidl/fidlc/src/values.h"
#include "tools/fidl/fidlc/src/versioning_types.h"

namespace fidlc {

class AttributeSchema;
class Reporter;
class Typespace;
class VirtualSourceFile;

struct Decl;
struct Library;
struct Modifier;
struct ModifierList;
struct RawIdentifier;
struct RawOrdinal64;

// Kinds of values that can determine an element's identity for ABI purposes.
enum class AbiKind : uint8_t {
  // Bits/enum members
  kValue,
  // Struct members
  kOffset,
  // Table/union/overlay members
  kOrdinal,
  // Protocol methods
  kSelector,
};

// A variant that can represent all AbiKind values.
using AbiValue = std::variant<uint64_t, int64_t, std::string_view>;

struct Element {
  enum class Kind : uint8_t {
    // Special
    kLibrary,
    kModifier,
    // Decls
    kAlias,
    kBits,
    kBuiltin,
    kConst,
    kEnum,
    kNewType,
    kOverlay,
    kProtocol,
    kResource,
    kService,
    kStruct,
    kTable,
    kUnion,
    // Members
    kBitsMember,
    kEnumMember,
    kOverlayMember,
    kProtocolCompose,
    kProtocolMethod,
    kResourceProperty,
    kServiceMember,
    kStructMember,
    kTableMember,
    kUnionMember,
  };

  Element(const Element&) = delete;
  Element(Element&&) = default;
  Element& operator=(Element&&) = default;
  virtual ~Element() = default;

  Element(Kind kind, std::unique_ptr<AttributeList> attributes)
      : kind(kind), attributes(std::move(attributes)) {}

  // Returns true if this element is a decl.
  bool IsDecl() const;
  // Asserts that this element is a decl.
  Decl* AsDecl();

  // Returns the element's modifiers, or null if it has none.
  ModifierList* GetModifiers();

  // Runs a function on every modifier of the element, if it has any.
  void ForEachModifier(const fit::function<void(Modifier*)>& fn);

  // Returns true if this is an anonymous layout (i.e. a layout not
  // directly bound to a type declaration as in `type Foo = struct { ... };`).
  bool IsAnonymousLayout() const;

  // Returns the element's unqualified name, e.g. "MyProtocol" or "MyMethod".
  std::string_view GetName() const;

  // Returns the source where GetName() comes from, to use in error messages.
  // Its contents are different from GetName() is the case of anonymous layouts.
  SourceSpan GetNameSource() const;

  // Returns the element's ABI kind, if it has one.
  std::optional<AbiKind> abi_kind() const;
  // Returns the element's ABI value, if it has one.
  std::optional<AbiValue> abi_value() const;

  Kind kind;
  std::unique_ptr<AttributeList> attributes;
  Availability availability;
};

struct Decl : public Element {
  enum class Kind : uint8_t {
    kAlias,
    kBits,
    kBuiltin,
    kConst,
    kEnum,
    kNewType,
    kOverlay,
    kProtocol,
    kResource,
    kService,
    kStruct,
    kTable,
    kUnion,
  };

  static Element::Kind ElementKind(Kind kind) {
    switch (kind) {
      case Kind::kBits:
        return Element::Kind::kBits;
      case Kind::kBuiltin:
        return Element::Kind::kBuiltin;
      case Kind::kConst:
        return Element::Kind::kConst;
      case Kind::kEnum:
        return Element::Kind::kEnum;
      case Kind::kNewType:
        return Element::Kind::kNewType;
      case Kind::kProtocol:
        return Element::Kind::kProtocol;
      case Kind::kResource:
        return Element::Kind::kResource;
      case Kind::kService:
        return Element::Kind::kService;
      case Kind::kStruct:
        return Element::Kind::kStruct;
      case Kind::kTable:
        return Element::Kind::kTable;
      case Kind::kAlias:
        return Element::Kind::kAlias;
      case Kind::kUnion:
        return Element::Kind::kUnion;
      case Kind::kOverlay:
        return Element::Kind::kOverlay;
    }
  }

  Decl(Kind kind, std::unique_ptr<AttributeList> attributes, Name name)
      : Element(ElementKind(kind), std::move(attributes)), kind(kind), name(std::move(name)) {}

  const Kind kind;
  const Name name;

  // Runs a function on every member of the decl, if it has any.
  void ForEachMember(const fit::function<void(Element*)>& fn);

  // Calls fn(this, modifier) for all modifiers, fn(this, member) for all
  // members, and fn(member, modifier) for all members that have modifiers.
  void ForEachEdge(const fit::function<void(Element* parent, Element* child)>& fn);

  // Returns a clone of this decl for the given range, only including members
  // that intersect the range. Narrows the returned decl's availability, and its
  // members' availabilities, to the range.
  std::unique_ptr<Decl> Split(VersionRange range) const;

  enum class State : uint8_t {
    kNotCompiled,
    kCompiling,
    kCompiled,
  };
  State state = State::kNotCompiled;

 private:
  // Helper to implement Split. Leaves the result's availability unset.
  virtual std::unique_ptr<Decl> SplitImpl(VersionRange range) const = 0;
};

struct Modifier final : public Element {
  Modifier(std::unique_ptr<AttributeList> attributes, SourceSpan name, ModifierValue value)
      : Element(Kind::kModifier, std::move(attributes)), name(name), value(value) {}
  std::unique_ptr<Modifier> Clone() const;

  SourceSpan name;
  ModifierValue value;
};

// In the flat AST, "no modifiers" is represented by an ModifierList
// containing an empty vector. (In the raw AST, null is used instead.)
struct ModifierList final {
  ModifierList() = default;
  explicit ModifierList(std::vector<std::unique_ptr<Modifier>> modifiers)
      : modifiers(std::move(modifiers)) {}

  std::unique_ptr<ModifierList> Split(VersionRange range) const;

  std::vector<std::unique_ptr<Modifier>> modifiers;
};

struct Builtin : public Decl {
  enum class Identity : uint8_t {
    // Layouts (primitive)
    kBool,
    kInt8,
    kInt16,
    kInt32,
    kInt64,
    kUint8,
    kZxUchar,
    kUint16,
    kUint32,
    kUint64,
    kZxUsize64,
    kZxUintptr64,
    kFloat32,
    kFloat64,
    // Layouts (other)
    kString,
    // Layouts (templated)
    kBox,
    kArray,
    kStringArray,
    kVector,
    kZxExperimentalPointer,
    kClientEnd,
    kServerEnd,
    // Layouts (aliases)
    kByte,
    // Layouts (internal)
    kFrameworkErr,
    // Constraints
    kOptional,
    kMax,
    // Version constants
    kNext,
    kHead,
  };

  Builtin(Identity id, Name name)
      : Decl(Decl::Kind::kBuiltin, std::make_unique<AttributeList>(), std::move(name)), id(id) {
    state = State::kCompiled;
  }

  const Identity id;

  // Return true if this decl is for an internal fidl type.
  bool IsInternal() const;

 private:
  std::unique_ptr<Decl> SplitImpl(VersionRange range) const override;
};

// A decl that defines a data type.
struct TypeDecl : public Decl {
  using Decl::Decl;

  // Set during the TypeShapeStep.
  std::optional<TypeShape> type_shape;
  bool type_shape_compiling = false;
};

struct TypeConstructor;
struct Alias;
struct Protocol;

// This is a struct used to group together all data produced during compilation
// that might be used by consumers that are downstream from type compilation
// (e.g. typeshape code, declaration sorting, JSON generator), that can't be
// obtained by looking at a type constructor's Type.
// Unlike TypeConstructor::Type which will always refer to the fully resolved/
// concrete (and eventually, canonicalized) type that the type constructor
// resolves to, this struct stores data about the actual parameters on this
// type constructor used to produce the type.
// These fields should be set in the same place where the parameters actually get
// resolved, i.e. Create (for layout parameters) and ApplyConstraints (for type
// constraints)
struct LayoutInvocation {
  // set if this type constructor refers to an alias
  const Alias* from_alias = nullptr;

  // Parameter data below: if a foo_resolved form is set, then its corresponding
  // foo_raw form must be defined as well (and vice versa).

  // resolved form of this type constructor's arguments
  const Type* element_type_resolved = nullptr;
  const SizeValue* size_resolved = nullptr;
  // This has no users, probably because it's missing in the JSON IR (it is not
  // yet generated for experimental_maybe_from_alias)
  HandleSubtype subtype_resolved = HandleSubtype::kHandle;
  // This has no users, probably because it's missing in the JSON IR (it is not
  // yet generated for experimental_maybe_from_alias).
  const HandleRightsValue* rights_resolved = nullptr;
  // This has no users, probably because it's missing in the JSON IR (it is not
  // yet generated for experimental_maybe_from_alias).
  const Protocol* protocol_decl = nullptr;
  // This has no users, probably because it's missing in the JSON IR (it is not
  // yet generated for experimental_maybe_from_alias).
  const Type* boxed_type_resolved = nullptr;

  // raw form of this type constructor's arguments
  const TypeConstructor* element_type_raw = {};
  const TypeConstructor* boxed_type_raw = {};
  const Constant* size_raw = nullptr;
  // This has no users, probably because it's missing in the JSON IR (it is not
  // yet generated for partial_type_ctors).
  const Constant* subtype_raw = nullptr;
  const Constant* rights_raw = nullptr;
  const Constant* protocol_decl_raw = nullptr;

  // Nullability is represented differently because there's only one degree of
  // freedom: if it was specified, this value is equal to kNullable
  Nullability nullability = Nullability::kNonnullable;

  // Utf8 is similarly just a boolean.
  bool utf8 = false;
};

struct LayoutParameter {
 public:
  virtual ~LayoutParameter() = default;
  enum Kind : uint8_t {
    kIdentifier,
    kLiteral,
    kType,
  };

  LayoutParameter(Kind kind, SourceSpan span) : kind(kind), span(span) {}

  // A layout parameter is either a type constructor or a constant. One of these
  // two methods must return non-null, and the other one must return null.
  virtual TypeConstructor* AsTypeCtor() const = 0;
  virtual Constant* AsConstant() const = 0;

  virtual std::unique_ptr<LayoutParameter> Clone() const = 0;

  const Kind kind;
  SourceSpan span;
};

struct LayoutParameterList final {
  LayoutParameterList() = default;
  LayoutParameterList(std::vector<std::unique_ptr<LayoutParameter>> items,
                      std::optional<SourceSpan> span)
      : items(std::move(items)), span(span) {}

  std::unique_ptr<LayoutParameterList> Clone() const;

  std::vector<std::unique_ptr<LayoutParameter>> items;
  const std::optional<SourceSpan> span;
};

struct TypeConstraints final {
  TypeConstraints() = default;
  TypeConstraints(std::vector<std::unique_ptr<Constant>> items, std::optional<SourceSpan> span)
      : items(std::move(items)), span(span) {}

  std::unique_ptr<TypeConstraints> Clone() const;

  std::vector<std::unique_ptr<Constant>> items;
  const std::optional<SourceSpan> span;
};

// Unlike RawTypeConstructor which will either store a name referencing a
// layout or an anonymous layout directly, in the flat AST all type constructors
// store a Reference. In the case where the type constructor represents an
// anonymous layout, the data of the anonymous layout is consumed and stored in
// the library and the corresponding type constructor contains a Reference
// whose name has AnonymousNameContext and a span covering the anonymous layout.
//
// This allows all type compilation to share the code paths through the consume
// step (i.e. RegisterDecl) and the compilation step (i.e. Typespace::Create),
// while ensuring that users cannot refer to anonymous layouts by name.
struct TypeConstructor final {
  TypeConstructor(SourceSpan span, Reference layout,
                  std::unique_ptr<LayoutParameterList> parameters,
                  std::unique_ptr<TypeConstraints> constraints)
      : span(span),
        layout(std::move(layout)),
        parameters(std::move(parameters)),
        constraints(std::move(constraints)) {}

  std::unique_ptr<TypeConstructor> Clone() const;

  // Set during construction.
  SourceSpan span;
  Reference layout;
  std::unique_ptr<LayoutParameterList> parameters;
  std::unique_ptr<TypeConstraints> constraints;

  // Set during compilation.
  Type* type = nullptr;
  LayoutInvocation resolved_params;
};

struct LiteralLayoutParameter final : public LayoutParameter {
  LiteralLayoutParameter(std::unique_ptr<LiteralConstant> literal, SourceSpan span)
      : LayoutParameter(Kind::kLiteral, span), literal(std::move(literal)) {}

  TypeConstructor* AsTypeCtor() const override;
  Constant* AsConstant() const override;
  std::unique_ptr<LayoutParameter> Clone() const override;

  std::unique_ptr<LiteralConstant> literal;
};

struct TypeLayoutParameter final : public LayoutParameter {
  TypeLayoutParameter(std::unique_ptr<TypeConstructor> type_ctor, SourceSpan span)
      : LayoutParameter(Kind::kType, span), type_ctor(std::move(type_ctor)) {}

  TypeConstructor* AsTypeCtor() const override;
  Constant* AsConstant() const override;
  std::unique_ptr<LayoutParameter> Clone() const override;

  std::unique_ptr<TypeConstructor> type_ctor;
};

struct IdentifierLayoutParameter final : public LayoutParameter {
  IdentifierLayoutParameter(Reference reference, SourceSpan span)
      : LayoutParameter(Kind::kIdentifier, span), reference(std::move(reference)) {}

  // Disambiguates between type constructor and constant. Must call after
  // resolving the reference, but before calling AsTypeCtor or AsConstant.
  void Disambiguate();

  TypeConstructor* AsTypeCtor() const override;
  Constant* AsConstant() const override;
  std::unique_ptr<LayoutParameter> Clone() const override;

  Reference reference;

  std::unique_ptr<TypeConstructor> as_type_ctor;
  std::unique_ptr<Constant> as_constant;
};

// Const represents the _declaration_ of a constant. (For the _use_, see
// Constant. For the _value_, see ConstantValue.) A Const consists of a
// left-hand-side Name (found in Decl) and a right-hand-side Constant.
struct Const final : public Decl {
  Const(std::unique_ptr<AttributeList> attributes, Name name,
        std::unique_ptr<TypeConstructor> type_ctor, std::unique_ptr<Constant> value)
      : Decl(Kind::kConst, std::move(attributes), std::move(name)),
        type_ctor(std::move(type_ctor)),
        value(std::move(value)) {}

  std::unique_ptr<TypeConstructor> type_ctor;
  std::unique_ptr<Constant> value;

 private:
  std::unique_ptr<Decl> SplitImpl(VersionRange range) const override;
};

struct Enum final : public TypeDecl {
  struct Member : public Element {
    Member(SourceSpan name, std::unique_ptr<Constant> value,
           std::unique_ptr<AttributeList> attributes)
        : Element(Element::Kind::kEnumMember, std::move(attributes)),
          name(name),
          value(std::move(value)) {}
    Member Clone() const;

    SourceSpan name;
    std::unique_ptr<Constant> value;
  };

  Enum(std::unique_ptr<AttributeList> attributes, std::unique_ptr<ModifierList> modifiers,
       Name name, std::unique_ptr<TypeConstructor> subtype_ctor, std::vector<Member> members)
      : TypeDecl(Kind::kEnum, std::move(attributes), std::move(name)),
        modifiers(std::move(modifiers)),
        subtype_ctor(std::move(subtype_ctor)),
        members(std::move(members)) {}

  // Set during construction.
  std::unique_ptr<ModifierList> modifiers;
  std::unique_ptr<TypeConstructor> subtype_ctor;
  std::vector<Member> members;

  // Set during compilation.
  std::optional<Strictness> strictness;
  const PrimitiveType* type = nullptr;
  // Set only for flexible enums, and either is set depending on signedness of
  // underlying enum type.
  std::optional<int64_t> unknown_value_signed;
  std::optional<uint64_t> unknown_value_unsigned;

 private:
  std::unique_ptr<Decl> SplitImpl(VersionRange range) const override;
};

struct Bits final : public TypeDecl {
  struct Member : public Element {
    Member(SourceSpan name, std::unique_ptr<Constant> value,
           std::unique_ptr<AttributeList> attributes)
        : Element(Element::Kind::kBitsMember, std::move(attributes)),
          name(name),
          value(std::move(value)) {}
    Member Clone() const;

    SourceSpan name;
    std::unique_ptr<Constant> value;
  };

  Bits(std::unique_ptr<AttributeList> attributes, std::unique_ptr<ModifierList> modifiers,
       Name name, std::unique_ptr<TypeConstructor> subtype_ctor, std::vector<Member> members)
      : TypeDecl(Kind::kBits, std::move(attributes), std::move(name)),
        modifiers(std::move(modifiers)),
        subtype_ctor(std::move(subtype_ctor)),
        members(std::move(members)) {}

  // Set during construction.
  std::unique_ptr<ModifierList> modifiers;
  std::unique_ptr<TypeConstructor> subtype_ctor;
  std::vector<Member> members;

  // Set during compilation.
  std::optional<Strictness> strictness;
  uint64_t mask = 0;

 private:
  std::unique_ptr<Decl> SplitImpl(VersionRange range) const override;
};

struct Service final : public Decl {
  struct Member : public Element {
    Member(std::unique_ptr<TypeConstructor> type_ctor, SourceSpan name,
           std::unique_ptr<AttributeList> attributes)
        : Element(Element::Kind::kServiceMember, std::move(attributes)),
          type_ctor(std::move(type_ctor)),
          name(name) {}
    Member Clone() const;

    std::unique_ptr<TypeConstructor> type_ctor;
    SourceSpan name;
  };

  Service(std::unique_ptr<AttributeList> attributes, Name name, std::vector<Member> members)
      : Decl(Kind::kService, std::move(attributes), std::move(name)), members(std::move(members)) {}

  std::vector<Member> members;

 private:
  std::unique_ptr<Decl> SplitImpl(VersionRange range) const override;
};

struct Struct final : public TypeDecl {
  struct Member : public Element {
    Member(std::unique_ptr<TypeConstructor> type_ctor, SourceSpan name,
           std::unique_ptr<Constant> maybe_default_value, std::unique_ptr<AttributeList> attributes)
        : Element(Element::Kind::kStructMember, std::move(attributes)),
          type_ctor(std::move(type_ctor)),
          name(name),
          maybe_default_value(std::move(maybe_default_value)) {}
    Member Clone() const;

    std::unique_ptr<TypeConstructor> type_ctor;
    SourceSpan name;
    std::unique_ptr<Constant> maybe_default_value;

    // Set during the TypeShapeStep.
    FieldShape field_shape;
  };

  Struct(std::unique_ptr<AttributeList> attributes, std::unique_ptr<ModifierList> modifiers,
         Name name, std::vector<Member> members)
      : TypeDecl(Kind::kStruct, std::move(attributes), std::move(name)),
        modifiers(std::move(modifiers)),
        members(std::move(members)) {}

  // Set during construction.
  std::unique_ptr<ModifierList> modifiers;
  std::vector<Member> members;

  // Set during compilation.
  std::optional<Resourceness> resourceness;

 private:
  std::unique_ptr<Decl> SplitImpl(VersionRange range) const override;
};

struct Table final : public TypeDecl {
  struct Member : public Element {
    Member(const RawOrdinal64* ordinal, std::unique_ptr<TypeConstructor> type_ctor, SourceSpan name,
           std::unique_ptr<AttributeList> attributes)
        : Element(Element::Kind::kTableMember, std::move(attributes)),
          ordinal(ordinal),
          type_ctor(std::move(type_ctor)),
          name(name) {}

    Member Clone() const;

    // Owned by Library::raw_ordinals.
    const RawOrdinal64* ordinal;
    std::unique_ptr<TypeConstructor> type_ctor;
    SourceSpan name;
  };

  Table(std::unique_ptr<AttributeList> attributes, std::unique_ptr<ModifierList> modifiers,
        Name name, std::vector<Member> members)
      : TypeDecl(Kind::kTable, std::move(attributes), std::move(name)),
        modifiers(std::move(modifiers)),
        members(std::move(members)) {}

  // Set during construction.
  std::unique_ptr<ModifierList> modifiers;
  std::vector<Member> members;

  // Set during compilation.
  // Tables are always flexible, but it simplifies generic code to also store
  // strictness on it (and we could implement strict tables in the future).
  std::optional<Strictness> strictness;
  std::optional<Resourceness> resourceness;

 private:
  std::unique_ptr<Decl> SplitImpl(VersionRange range) const override;
};

struct Union final : public TypeDecl {
  struct Member : public Element {
    Member(const RawOrdinal64* ordinal, std::unique_ptr<TypeConstructor> type_ctor, SourceSpan name,
           std::unique_ptr<AttributeList> attributes)
        : Element(Element::Kind::kUnionMember, std::move(attributes)),
          ordinal(ordinal),
          type_ctor(std::move(type_ctor)),
          name(name) {}

    Member Clone() const;

    // Owned by Library::raw_ordinals.
    const RawOrdinal64* ordinal;
    std::unique_ptr<TypeConstructor> type_ctor;
    SourceSpan name;
  };

  Union(std::unique_ptr<AttributeList> attributes, std::unique_ptr<ModifierList> modifiers,
        Name name, std::vector<Member> members)
      : TypeDecl(Kind::kUnion, std::move(attributes), std::move(name)),
        modifiers(std::move(modifiers)),
        members(std::move(members)) {}

  // Set during construction.
  std::unique_ptr<ModifierList> modifiers;
  std::vector<Member> members;

  // Set during compilation.
  std::optional<Strictness> strictness;
  std::optional<Resourceness> resourceness;

 private:
  std::unique_ptr<Decl> SplitImpl(VersionRange range) const override;
};

struct Overlay final : public TypeDecl {
  struct Member final : public Element {
    Member(const RawOrdinal64* ordinal, std::unique_ptr<TypeConstructor> type_ctor, SourceSpan name,
           std::unique_ptr<AttributeList> attributes)
        : Element(Element::Kind::kOverlayMember, std::move(attributes)),
          ordinal(ordinal),
          type_ctor(std::move(type_ctor)),
          name(name) {}

    Member Clone() const;

    // Owned by Library::raw_ordinals.
    const RawOrdinal64* ordinal;
    std::unique_ptr<TypeConstructor> type_ctor;
    SourceSpan name;
  };

  Overlay(std::unique_ptr<AttributeList> attributes, std::unique_ptr<ModifierList> modifiers,
          Name name, std::vector<Member> members)
      : TypeDecl(Kind::kOverlay, std::move(attributes), std::move(name)),
        modifiers(std::move(modifiers)),
        members(std::move(members)) {}

  // Set during construction.
  std::unique_ptr<ModifierList> modifiers;
  std::vector<Member> members;

  // Set during compilation.
  std::optional<Strictness> strictness;
  std::optional<Resourceness> resourceness;

 private:
  std::unique_ptr<Decl> SplitImpl(VersionRange range) const override;
};

struct Protocol final : public Decl {
  struct Method : public Element {
    enum class Kind : uint8_t { kOneWay, kTwoWay, kEvent };

    Method(std::unique_ptr<AttributeList> attributes, std::unique_ptr<ModifierList> modifiers,
           Kind kind, SourceSpan name, std::unique_ptr<TypeConstructor> maybe_request,
           std::unique_ptr<TypeConstructor> maybe_response, const Union* maybe_result_union,
           bool has_error)
        : Element(Element::Kind::kProtocolMethod, std::move(attributes)),
          modifiers(std::move(modifiers)),
          kind(kind),
          name(name),
          maybe_request(std::move(maybe_request)),
          maybe_response(std::move(maybe_response)),
          maybe_result_union(maybe_result_union),
          has_error(has_error) {}

    Method Clone(VersionRange range) const;

    enum ResultUnionOrdinal : uint64_t {
      kSuccess = 1,
      kDomainError = 2,
      kFrameworkError = 3,
    };

    std::unique_ptr<ModifierList> modifiers;
    Kind kind;
    SourceSpan name;
    std::unique_ptr<TypeConstructor> maybe_request;
    std::unique_ptr<TypeConstructor> maybe_response;
    const Union* maybe_result_union;
    bool has_error;

    // Set during compilation
    std::optional<Strictness> strictness;
    std::string selector;
    uint64_t ordinal = 0;
    const TypeConstructor* result_success_type_ctor = nullptr;
    const TypeConstructor* result_domain_error_type_ctor = nullptr;
  };

  struct ComposedProtocol : public Element {
    ComposedProtocol(std::unique_ptr<AttributeList> attributes, Reference reference)
        : Element(Element::Kind::kProtocolCompose, std::move(attributes)),
          reference(std::move(reference)) {}
    ComposedProtocol Clone() const;

    Reference reference;
  };

  // Used to keep track of all methods, including composed methods.
  struct MethodWithInfo {
    // Pointer into owning_protocol->methods.
    const Method* method;
    const Protocol* owning_protocol;
    // Pointer into this->composed_protocols, or null if not composed.
    // In the transitive case A -> B -> C, this is A's `compose B;`.
    const ComposedProtocol* composed;
  };

  Protocol(std::unique_ptr<AttributeList> attributes, std::unique_ptr<ModifierList> modifiers,
           Name name, std::vector<ComposedProtocol> composed_protocols, std::vector<Method> methods)
      : Decl(Kind::kProtocol, std::move(attributes), std::move(name)),
        modifiers(std::move(modifiers)),
        composed_protocols(std::move(composed_protocols)),
        methods(std::move(methods)) {}

  // Set during construction.
  std::unique_ptr<ModifierList> modifiers;
  std::vector<ComposedProtocol> composed_protocols;
  std::vector<Method> methods;

  // Set during compilation.
  std::optional<Openness> openness;
  std::vector<MethodWithInfo> all_methods;

 private:
  std::unique_ptr<Decl> SplitImpl(VersionRange range) const override;
};

struct Resource final : public Decl {
  struct Property : public Element {
    Property(std::unique_ptr<TypeConstructor> type_ctor, SourceSpan name,
             std::unique_ptr<AttributeList> attributes)
        : Element(Element::Kind::kResourceProperty, std::move(attributes)),
          type_ctor(std::move(type_ctor)),
          name(name) {}
    Property Clone() const;

    std::unique_ptr<TypeConstructor> type_ctor;
    SourceSpan name;
  };

  Resource(std::unique_ptr<AttributeList> attributes, Name name,
           std::unique_ptr<TypeConstructor> subtype_ctor, std::vector<Property> properties)
      : Decl(Kind::kResource, std::move(attributes), std::move(name)),
        subtype_ctor(std::move(subtype_ctor)),
        properties(std::move(properties)) {}

  // Set during construction.
  std::unique_ptr<TypeConstructor> subtype_ctor;
  std::vector<Property> properties;

  Property* LookupProperty(std::string_view name);

 private:
  std::unique_ptr<Decl> SplitImpl(VersionRange range) const override;
};

struct Alias final : public Decl {
  Alias(std::unique_ptr<AttributeList> attributes, Name name,
        std::unique_ptr<TypeConstructor> partial_type_ctor)
      : Decl(Kind::kAlias, std::move(attributes), std::move(name)),
        partial_type_ctor(std::move(partial_type_ctor)) {}

  // The shape of this type constructor is more constrained than just being a
  // "partial" type constructor - it is either a normal type constructor
  // referring directly to a non-type-alias with all layout parameters fully
  // specified (e.g. alias foo = array<T, 3>), or it is a type constructor
  // referring to another alias that has no layout parameters (e.g. alias
  // bar = foo).
  // The constraints on the other hand are indeed "partial" - any alias
  // at any point in an "alias chain" can specify a constraint, but any
  // constraint can only specified once. This behavior will change in
  // https://fxbug.dev/42153849.
  std::unique_ptr<TypeConstructor> partial_type_ctor;

 private:
  std::unique_ptr<Decl> SplitImpl(VersionRange range) const override;
};

struct NewType final : public TypeDecl {
  NewType(std::unique_ptr<AttributeList> attributes, Name name,
          std::unique_ptr<TypeConstructor> type_ctor)
      : TypeDecl(Kind::kNewType, std::move(attributes), std::move(name)),
        type_ctor(std::move(type_ctor)) {}

  // Note that unlike in Alias, we are not calling this partial type constructor. Whether or
  // not all the constraints for this type are applied is irrelevant to us down the line - all
  // we care is that we have a type constructor to define a type.
  std::unique_ptr<TypeConstructor> type_ctor;

 private:
  std::unique_ptr<Decl> SplitImpl(VersionRange range) const override;
};

// This class is used to manage a library's set of direct dependencies, i.e.
// those imported with "using" statements.
class Dependencies {
 public:
  enum class RegisterResult : uint8_t {
    kSuccess,
    kDuplicate,
    kCollision,
  };

  // Registers a dependency to a library. The registration name is |maybe_alias|
  // if provided, otherwise the library's name. Afterwards, Dependencies::Lookup
  // will return |dep_library| given the registration name.
  RegisterResult Register(const SourceSpan& span, std::string_view filename, Library* dep_library,
                          const std::unique_ptr<RawIdentifier>& maybe_alias);

  // Returns true if this dependency set contains a library with the given name and filename.
  bool Contains(std::string_view filename, std::string_view library_name);

  // Looks up a dependency by filename (within the importing library, since
  // "using" statements are file-scoped) and name (of the imported library).
  // Also marks the library as used. Returns null if no library is found.
  Library* LookupAndMarkUsed(std::string_view filename, std::string_view library_name) const;

  // VerifyAllDependenciesWereUsed reports an error for each dependency imported
  // with `using` that was never used in the file.
  void VerifyAllDependenciesWereUsed(const Library* for_library, Reporter* reporter);

  // Returns all the dependencies.
  const std::set<Library*>& all() const { return dependencies_aggregate_; }

  std::vector<std::pair<Library*, SourceSpan>> library_references() {
    std::vector<std::pair<Library*, SourceSpan>> references;
    references.reserve(refs_.size());
    for (auto& ref : refs_) {
      auto library_ref = std::make_pair(ref->library, ref->span);
      references.emplace_back(library_ref);
    }
    return references;
  }

 private:
  // A reference to a library, derived from a "using" statement.
  struct LibraryRef {
    LibraryRef(SourceSpan span, Library* library) : span(span), library(library) {}

    const SourceSpan span;
    Library* const library;
    bool used = false;
  };

  // Per-file information about imports.
  struct PerFile {
    // References to dependencies, keyed by library name or by alias.
    std::map<std::string_view, LibraryRef*> refs;
    // Set containing ref->library for every ref in |refs|.
    std::set<Library*> libraries;
  };

  std::vector<std::unique_ptr<LibraryRef>> refs_;
  // The string keys are owned by SourceFile objects.
  std::map<std::string_view, std::unique_ptr<PerFile>> by_filename_;
  std::set<Library*> dependencies_aggregate_;
};

struct LibraryComparator;

struct Library final : public Element {
  Library() : Element(Element::Kind::kLibrary, std::make_unique<AttributeList>()) {}

  // Creates the root library which holds all Builtin decls.
  static std::unique_ptr<Library> CreateRootLibrary();

  // Runs a function on every element in the library via depth-first traversal.
  // Runs it on the library itself, on all Decls, and on all their members.
  void ForEachElement(const fit::function<void(Element*)>& fn);

  struct Declarations {
    // Inserts a declaration. When inserting builtins, this must be called in
    // order of Builtin::Identity. For other decls, the order doesn't matter.
    Decl* Insert(std::unique_ptr<Decl> decl);
    // Looks up a builtin. Must have inserted it already with InsertBuiltin.
    Builtin* LookupBuiltin(Builtin::Identity id) const;

    // Contains all the declarations owned by the vectors below. It preserves
    // insertion order for equal keys, which is source order (ConsumeStep) and
    // then decomposed version range order (ResolveStep).
    std::multimap<std::string_view, Decl*> all;

    std::vector<std::unique_ptr<Alias>> aliases;
    std::vector<std::unique_ptr<Bits>> bits;
    std::vector<std::unique_ptr<Builtin>> builtins;
    std::vector<std::unique_ptr<Const>> consts;
    std::vector<std::unique_ptr<Enum>> enums;
    std::vector<std::unique_ptr<NewType>> new_types;
    std::vector<std::unique_ptr<Protocol>> protocols;
    std::vector<std::unique_ptr<Resource>> resources;
    std::vector<std::unique_ptr<Service>> services;
    std::vector<std::unique_ptr<Struct>> structs;
    std::vector<std::unique_ptr<Table>> tables;
    std::vector<std::unique_ptr<Union>> unions;
    std::vector<std::unique_ptr<Overlay>> overlays;
  };

  std::string name;
  std::vector<SourceSpan> name_spans;
  // Set during AvailabilityStep.
  std::optional<Platform> platform;
  Dependencies dependencies;
  // Populated by ConsumeStep, and then rewritten by ResolveStep.
  Declarations declarations;
  // Contains the same decls as `declarations`, but in a topologically sorted
  // order (later decls only depend on earlier ones). Populated in CompileStep.
  std::vector<const Decl*> declaration_order;
  // Raw AST objects pointed to by certain flat AST nodes. We store them on the
  // Library because there is no unique ownership (e.g. multiple Table::Member
  // instances can point to the same RawOrdinal64 after decomposition).
  std::vector<std::unique_ptr<RawLiteral>> raw_literals;
  std::vector<std::unique_ptr<RawOrdinal64>> raw_ordinals;
};

struct LibraryComparator {
  bool operator()(const Library* lhs, const Library* rhs) const {
    ZX_ASSERT(!lhs->name.empty());
    ZX_ASSERT(!rhs->name.empty());
    return lhs->name < rhs->name;
  }
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_FLAT_AST_H_
