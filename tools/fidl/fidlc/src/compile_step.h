// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_COMPILE_STEP_H_
#define TOOLS_FIDL_FIDLC_SRC_COMPILE_STEP_H_

#include "tools/fidl/fidlc/src/compiler.h"

namespace fidlc {

// We run one main CompileStep for the whole library. Some attributes are
// compiled before that via the CompileAttributeEarly method. To avoid kicking
// off other compilations, these attributes only allow literal arguments.
class CompileStep : public Compiler::Step {
 public:
  using Step::Step;

  friend class AttributeSchema;
  friend class AttributeArgSchema;
  friend class TypeResolver;

  // Compiles an attribute early, before the main CompileStep has started. The
  // attribute must support this (see AttributeSchema::IsCompileEarly).
  static void CompileAttributeEarly(Compiler* compiler, Attribute* attribute);

 private:
  void RunImpl() override;

  // Output parameters for compiling modifiers.
  struct OutModifiers {
    std::optional<Strictness>* strictness = nullptr;
    std::optional<Resourceness>* resourceness = nullptr;
    std::optional<Openness>* openness = nullptr;
  };

  // Compile methods
  void CompileAlias(Alias* alias);
  void CompileAttribute(Attribute* attribute, bool early = false);
  void CompileAttributeList(AttributeList* attributes);
  void CompileModifierList(ModifierList* modifiers, OutModifiers out);
  void CompileBits(Bits* bits_declaration);
  void CompileConst(Const* const_declaration);
  void CompileDecl(Decl* decl);
  void CompileEnum(Enum* enum_declaration);
  void CompileNewType(NewType* new_type);
  void CompileProtocol(Protocol* protocol_declaration);
  void CompileResource(Resource* resource_declaration);
  void CompileResultUnion(Protocol::Method* method);
  void CompileService(Service* service_decl);
  void CompileStruct(Struct* struct_declaration);
  void CompileTable(Table* table_declaration);
  // If compile_decls is true, recursively compiles TypeDecls the type refers to.
  void CompileTypeConstructor(TypeConstructor* type_ctor, bool compile_decls = true);
  void CompileUnion(Union* union_declaration);
  void CompileOverlay(Overlay* overlay_declaration);

  // Resolve methods
  bool ResolveHandleRightsConstant(Resource* resource, Constant* constant,
                                   const HandleRightsValue** out_rights);
  bool ResolveHandleSubtypeIdentifier(Resource* resource, Constant* constant,
                                      HandleSubtype* out_obj_type);
  bool ResolveSizeBound(Constant* size_constant, const SizeValue** out_size);
  bool ResolveOrOperatorConstant(Constant* constant, std::optional<const Type*> opt_type,
                                 const ConstantValue& left_operand,
                                 const ConstantValue& right_operand);
  bool ResolveConstant(Constant* constant, std::optional<const Type*> opt_type);
  bool ResolveIdentifierConstant(IdentifierConstant* identifier_constant,
                                 std::optional<const Type*> opt_type);
  bool ResolveLiteralConstant(LiteralConstant* literal_constant,
                              std::optional<const Type*> opt_type);
  bool ResolveAsOptional(Constant* constant);
  bool ResolveLiteralConstantNumeric(LiteralConstant* literal_constant,
                                     const PrimitiveType* primitive_type);
  template <typename NumericType>
  bool ResolveLiteralConstantNumericImpl(LiteralConstant* literal_constant,
                                         const PrimitiveType* primitive_type);

  // Type methods
  bool TypeCanBeConst(const Type* type);
  bool TypeIsConvertibleTo(const Type* from_type, const Type* to_type);
  const Type* UnderlyingType(const Type* type);
  const Type* InferType(Constant* constant);
  ConstantValue::Kind ConstantValuePrimitiveKind(PrimitiveSubtype primitive_subtype);

  // Validates a single member of a bits or enum. On success, returns nullptr,
  // and on failure returns an error. The caller will set the diagnostic span.
  template <typename MemberType>
  using MemberValidator = fit::function<std::unique_ptr<Diagnostic>(
      const MemberType& member, const AttributeList* attributes, SourceSpan span)>;

  // Validation methods
  template <typename DeclType, typename MemberType>
  bool ValidateMembers(DeclType* decl, MemberValidator<MemberType> validator);
  template <typename MemberType>
  bool ValidateBitsMembersAndCalcMask(Bits* bits_decl, MemberType* out_mask);
  template <typename MemberType>
  bool ValidateEnumMembersAndCalcUnknownValue(Enum* enum_decl, MemberType* out_unknown_value);
  void ValidateSelectorAndCalcOrdinal(const Name& protocol_name, Protocol::Method* method);
  void ValidatePayload(const TypeConstructor* type_ctor);
  void ValidateDomainError(const TypeConstructor* type_ctor);

  // Decl for the HEAD constant, used in attribute_schema.cc.
  Decl* head_decl;

  // Stack of decls in the kCompiling state, used to detect cycles.
  std::vector<const Decl*> decl_stack_;

  // Number of @no_resource attributes we are nested within.
  size_t no_resource_count_ = 0;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_COMPILE_STEP_H_
