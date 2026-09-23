// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_TYPES_H_
#define TOOLS_FIDL_FIDLC_SRC_TYPES_H_

#include <zircon/assert.h>

#include <utility>

#include "tools/fidl/fidlc/src/constraints.h"
#include "tools/fidl/fidlc/src/name.h"
#include "tools/fidl/fidlc/src/properties.h"
#include "tools/fidl/fidlc/src/type_shape.h"
#include "tools/fidl/fidlc/src/values.h"

namespace fidlc {

class TypeResolver;

struct Decl;
struct LayoutInvocation;
struct Resource;
struct Struct;
struct TypeConstraints;
struct TypeDecl;

struct Type {
  enum class Kind : uint8_t {
    kArray,
    kBox,
    kVector,
    kZxExperimentalPointer,
    kString,
    kHandle,
    kTransportSide,
    kPrimitive,
    kInternal,
    kUntypedNumeric,
    kIdentifier,
  };

  Type(Name name, Kind kind) : name(std::move(name)), kind(kind) {}
  virtual ~Type() = default;

  const Name name;
  const Kind kind;

  // Set during the TypeShapeStep.
  std::optional<TypeShape> type_shape;
  bool type_shape_compiling = false;

  virtual bool IsNullable() const { return false; }

  // Returns the nominal resourceness of the type per the FTP-057 definition.
  // For IdentifierType, can only be called after the Decl has been compiled.
  Resourceness Resourceness() const;

  // Apply the provided constraints to this type, returning the newly constrained
  // Type and recording the invocation inside resolved_args.
  // For types in the new syntax, we receive the unresolved TypeConstraints.
  virtual bool ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                                const TypeConstraints& constraints, const Reference& layout,
                                std::unique_ptr<Type>* out_type,
                                LayoutInvocation* out_params) const = 0;
};

struct RejectOptionalConstraints : public Constraints<> {
  using Constraints::Constraints;
  bool OnUnexpectedConstraint(TypeResolver* resolver, Reporter* reporter,
                              std::optional<SourceSpan> params_span, const Name& layout_name,
                              Resource* resource, size_t num_constraints,
                              const std::vector<std::unique_ptr<Constant>>& params,
                              size_t param_index) const override;
};

struct ArrayConstraints : public Constraints<ConstraintKind::kUtf8> {
  using Constraints::Constraints;
  bool OnUnexpectedConstraint(TypeResolver* resolver, Reporter* reporter,
                              std::optional<SourceSpan> params_span, const Name& layout_name,
                              Resource* resource, size_t num_constraints,
                              const std::vector<std::unique_ptr<Constant>>& params,
                              size_t param_index) const override;
};

struct ArrayType final : public Type, public ArrayConstraints {
  using Constraints = ArrayConstraints;

  ArrayType(const Name& name, Type* element_type, const SizeValue* element_count)
      : Type(name, Kind::kArray), element_type(element_type), element_count(element_count) {}
  ArrayType(const Name& name, Type* element_type, const SizeValue* element_count,
            Constraints constraints)
      : Type(name, Kind::kArray),
        Constraints(std::move(constraints)),
        element_type(element_type),
        element_count(element_count) {}

  Type* element_type;
  const SizeValue* element_count;

  bool ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                        const TypeConstraints& constraints, const Reference& layout,
                        std::unique_ptr<Type>* out_type,
                        LayoutInvocation* out_params) const override;

  bool IsStringArray() const { return utf8; }
};

struct VectorConstraints : public Constraints<ConstraintKind::kSize, ConstraintKind::kNullability> {
  using Constraints::Constraints;
  bool OnUnexpectedConstraint(TypeResolver* resolver, Reporter* reporter,
                              std::optional<SourceSpan> params_span, const Name& layout_name,
                              Resource* resource, size_t num_constraints,
                              const std::vector<std::unique_ptr<Constant>>& params,
                              size_t param_index) const override;
};

struct VectorType final : public Type, public VectorConstraints {
  using Constraints = VectorConstraints;

  VectorType(const Name& name, Type* element_type)
      : Type(name, Kind::kVector), element_type(element_type) {}
  VectorType(const Name& name, Type* element_type, Constraints constraints)
      : Type(name, Kind::kVector),
        Constraints(std::move(constraints)),
        element_type(element_type) {}

  Type* element_type;

  uint32_t ElementCount() const { return size ? size->value : kMaxSize; }
  bool IsNullable() const override { return nullability == Nullability::kNullable; }

  bool ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                        const TypeConstraints& constraints, const Reference& layout,
                        std::unique_ptr<Type>* out_type,
                        LayoutInvocation* out_params) const override;
};

struct StringType final : public Type, public VectorConstraints {
  using Constraints = VectorConstraints;

  explicit StringType(const Name& name) : Type(name, Kind::kString) {}
  StringType(const Name& name, Constraints constraints)
      : Type(name, Kind::kString), Constraints(std::move(constraints)) {}

  uint32_t MaxSize() const { return size ? size->value : kMaxSize; }
  bool IsNullable() const override { return nullability == Nullability::kNullable; }

  bool ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                        const TypeConstraints& constraints, const Reference& layout,
                        std::unique_ptr<Type>* out_type,
                        LayoutInvocation* out_params) const override;
};

using HandleConstraints = Constraints<ConstraintKind::kHandleSubtype, ConstraintKind::kHandleRights,
                                      ConstraintKind::kNullability>;
struct HandleType final : public Type, HandleConstraints {
  using Constraints = HandleConstraints;

  HandleType(const Name& name, Resource* resource_decl)
      // TODO(https://fxbug.dev/42143256): The default obj_type and rights should be
      // determined by the resource_definition, not hardcoded here.
      : HandleType(name, resource_decl, Constraints()) {}

  HandleType(const Name& name, Resource* resource_decl, Constraints constraints)
      : Type(name, Kind::kHandle),
        Constraints(std::move(constraints)),
        resource_decl(resource_decl) {}

  Resource* resource_decl;

  bool IsNullable() const override { return nullability == Nullability::kNullable; }

  bool ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                        const TypeConstraints& constraints, const Reference& layout,
                        std::unique_ptr<Type>* out_type,
                        LayoutInvocation* out_params) const override;

  const static HandleRightsValue kSameRights;
};

struct PrimitiveType final : public Type, public RejectOptionalConstraints {
  using Constraints = RejectOptionalConstraints;

  PrimitiveType(const Name& name, PrimitiveSubtype subtype)
      : Type(name, Kind::kPrimitive), subtype(subtype) {}

  PrimitiveSubtype subtype;

  bool ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                        const TypeConstraints& constraints, const Reference& layout,
                        std::unique_ptr<Type>* out_type,
                        LayoutInvocation* out_params) const override;

  static uint32_t SubtypeSize(PrimitiveSubtype subtype);
};

// Internal types are types which are used internally by the bindings but not
// exposed for FIDL libraries to use.
struct InternalType final : public Type, public Constraints<> {
  using Constraints = Constraints<>;

  InternalType(const Name& name, InternalSubtype subtype)
      : Type(name, Kind::kInternal), subtype(subtype) {}

  InternalSubtype subtype;

  bool ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                        const TypeConstraints& constraints, const Reference& layout,
                        std::unique_ptr<Type>* out_type,
                        LayoutInvocation* out_params) const override;

 private:
  static uint32_t SubtypeSize(InternalSubtype subtype);
};

struct IdentifierType final : public Type, public Constraints<ConstraintKind::kNullability> {
  using Constraints = Constraints<ConstraintKind::kNullability>;

  explicit IdentifierType(TypeDecl* type_decl) : IdentifierType(type_decl, Constraints()) {}
  IdentifierType(TypeDecl* type_decl, Constraints constraints);

  TypeDecl* type_decl;

  bool IsNullable() const override { return nullability == Nullability::kNullable; }

  bool ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                        const TypeConstraints& constraints, const Reference& layout,
                        std::unique_ptr<Type>* out_type,
                        LayoutInvocation* out_params) const override;
};

enum class TransportSide : uint8_t {
  kClient,
  kServer,
};

struct TransportSideConstraints
    : public Constraints<ConstraintKind::kProtocol, ConstraintKind::kNullability> {
  using Constraints::Constraints;
  bool OnUnexpectedConstraint(TypeResolver* resolver, Reporter* reporter,
                              std::optional<SourceSpan> params_span, const Name& layout_name,
                              Resource* resource, size_t num_constraints,
                              const std::vector<std::unique_ptr<Constant>>& params,
                              size_t param_index) const override;
};

struct TransportSideType final : public Type, public TransportSideConstraints {
  using Constraints = TransportSideConstraints;

  TransportSideType(const Name& name, TransportSide end, std::string_view protocol_transport)
      : TransportSideType(name, Constraints(), end, protocol_transport) {}
  TransportSideType(const Name& name, Constraints constraints, TransportSide end,
                    std::string_view protocol_transport)
      : Type(name, Kind::kTransportSide),
        Constraints(std::move(constraints)),
        end(end),
        protocol_transport(protocol_transport) {}

  bool IsNullable() const override { return nullability == Nullability::kNullable; }

  const TransportSide end;
  // TODO(https://fxbug.dev/42134495): Eventually, this will need to point to a transport
  // declaration.
  const std::string_view protocol_transport;

  bool ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                        const TypeConstraints& constraints, const Reference& layout,
                        std::unique_ptr<Type>* out_type,
                        LayoutInvocation* out_params) const override;
};

struct BoxConstraints : public Constraints<> {
  using Constraints::Constraints;
  bool OnUnexpectedConstraint(TypeResolver* resolver, Reporter* reporter,
                              std::optional<SourceSpan> params_span, const Name& layout_name,
                              Resource* resource, size_t num_constraints,
                              const std::vector<std::unique_ptr<Constant>>& params,
                              size_t param_index) const override;
};

struct BoxType final : public Type, public BoxConstraints {
  using Constraints = BoxConstraints;

  BoxType(const Name& name, Type* boxed_type) : Type(name, Kind::kBox), boxed_type(boxed_type) {}

  Type* boxed_type;

  // All boxes are implicitly nullable.
  bool IsNullable() const override { return true; }

  bool ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                        const TypeConstraints& constraints, const Reference& layout,
                        std::unique_ptr<Type>* out_type,
                        LayoutInvocation* out_params) const override;
};

struct UntypedNumericType final : public Type, public Constraints<> {
  using Constraints = Constraints<>;

  explicit UntypedNumericType(const Name& name) : Type(name, Kind::kUntypedNumeric) {}
  bool ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                        const TypeConstraints& constraints, const Reference& layout,
                        std::unique_ptr<Type>* out_type,
                        LayoutInvocation* out_params) const override;
};

struct ZxExperimentalPointerType final : public Type, public Constraints<> {
  using Constraints = Constraints<>;

  ZxExperimentalPointerType(const Name& name, Type* pointee_type)
      : Type(name, Kind::kZxExperimentalPointer), pointee_type(pointee_type) {}

  Type* pointee_type;

  bool ApplyConstraints(TypeResolver* resolver, Reporter* reporter,
                        const TypeConstraints& constraints, const Reference& layout,
                        std::unique_ptr<Type>* out_type,
                        LayoutInvocation* out_params) const override;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_TYPES_H_
