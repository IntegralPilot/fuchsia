// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_CONSUME_STEP_H_
#define TOOLS_FIDL_FIDLC_SRC_CONSUME_STEP_H_

#include "tools/fidl/fidlc/src/compiler.h"

namespace fidlc {

// We run a separate ConsumeStep for each file in the library.
class ConsumeStep : public Compiler::Step {
 public:
  ConsumeStep(Compiler* compiler, std::unique_ptr<File> file);

 private:
  void RunImpl() override;

  // Returns a pointer to the registered decl, or null on failure.
  Decl* RegisterDecl(std::unique_ptr<Decl> decl);

  // Top level declarations
  void ConsumeAliasDeclaration(std::unique_ptr<RawAliasDeclaration> alias_declaration);
  void ConsumeConstDeclaration(std::unique_ptr<RawConstDeclaration> const_declaration);
  void ConsumeProtocolDeclaration(std::unique_ptr<RawProtocolDeclaration> protocol_declaration);
  void ConsumeResourceDeclaration(std::unique_ptr<RawResourceDeclaration> resource_declaration);
  void ConsumeServiceDeclaration(std::unique_ptr<RawServiceDeclaration> service_decl);
  void ConsumeTypeDeclaration(std::unique_ptr<RawTypeDeclaration> type_decl);
  void ConsumeNewType(std::unique_ptr<RawTypeDeclaration> type_decl);
  void ConsumeUsing(std::unique_ptr<RawUsing> using_directive);

  // Layouts
  template <typename T>  // T should be Table, Union or Overlay
  bool ConsumeOrdinaledLayout(std::unique_ptr<RawLayout> layout,
                              const std::shared_ptr<NamingContext>& context,
                              std::unique_ptr<RawAttributeList> raw_attribute_list,
                              Decl** out_decl);
  bool ConsumeStructLayout(std::unique_ptr<RawLayout> layout,
                           const std::shared_ptr<NamingContext>& context,
                           std::unique_ptr<RawAttributeList> raw_attribute_list, Decl** out_decl);
  template <typename T>  // T should be Bits or Enum
  bool ConsumeValueLayout(std::unique_ptr<RawLayout> layout,
                          const std::shared_ptr<NamingContext>& context,
                          std::unique_ptr<RawAttributeList> raw_attribute_list, Decl** out_decl);
  bool ConsumeLayout(std::unique_ptr<RawLayout> layout,
                     const std::shared_ptr<NamingContext>& context,
                     std::unique_ptr<RawAttributeList> raw_attribute_list, Decl** out_decl);

  // Other elements
  void ConsumeAttribute(std::unique_ptr<RawAttribute> raw_attribute,
                        std::unique_ptr<Attribute>* out_attribute);
  void ConsumeAttributeList(std::unique_ptr<RawAttributeList> raw_attribute_list,
                            std::unique_ptr<AttributeList>* out_attribute_list);
  void ConsumeModifier(std::unique_ptr<RawModifier> raw_modifier,
                       std::unique_ptr<Modifier>* out_modifier);
  void ConsumeModifierList(std::unique_ptr<RawModifierList> raw_modifier_list,
                           std::unique_ptr<ModifierList>* out_modifier_list);
  bool ConsumeConstant(std::unique_ptr<RawConstant> raw_constant,
                       std::unique_ptr<Constant>* out_constant);
  void ConsumeLiteralConstant(RawLiteralConstant* raw_constant,
                              std::unique_ptr<LiteralConstant>* out_constant);
  bool ConsumeTypeConstructor(std::unique_ptr<RawTypeConstructor> raw_type_ctor,
                              const std::shared_ptr<NamingContext>& context,
                              std::unique_ptr<TypeConstructor>* out_type_ctor);

  // Method result unions
  bool NeedMethodResultUnion(const RawModifierList* raw_modifiers, Protocol::Method::Kind kind,
                             bool has_error);
  void ConsumeMethodResultUnion(const Name& protocol_name, std::string_view method_name,
                                SourceSpan response_span,
                                const std::shared_ptr<NamingContext>& response_context,
                                std::unique_ptr<RawTypeConstructor> raw_success_type_ctor,
                                std::unique_ptr<RawTypeConstructor> raw_error_type_ctor,
                                Union** out_union);

  // Elements stored in the library
  const RawLiteral* ConsumeLiteral(std::unique_ptr<RawLiteral> raw_literal);
  const RawOrdinal64* ConsumeOrdinal(std::unique_ptr<RawOrdinal64> raw_ordinal);

  // Sets the naming context's generated name override to the @generated_name
  // attribute's value if present, otherwise does nothing.
  void MaybeOverrideName(AttributeList& attributes, NamingContext* context);

  std::unique_ptr<File> file_;

  // Decl for default underlying type to use for bits and enums.
  Decl* default_underlying_type_;

  // Decl for the type to use for framework_err.
  Decl* framework_err_type_;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_CONSUME_STEP_H_
