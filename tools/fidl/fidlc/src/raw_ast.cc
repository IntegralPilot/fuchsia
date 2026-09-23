// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file contains the implementations of the Accept methods for the AST
// nodes.  Generally, all they do is invoke the appropriate TreeVisitor method
// for each field of the node.

#include "tools/fidl/fidlc/src/raw_ast.h"

#include "tools/fidl/fidlc/src/tree_visitor.h"

namespace fidlc {

SourceElementMark::SourceElementMark(TreeVisitor* tv, const SourceElement& element)
    : tv_(tv), element_(element) {
  tv_->OnSourceElementStart(element_);
}

SourceElementMark::~SourceElementMark() { tv_->OnSourceElementEnd(element_); }

void RawIdentifier::Accept(TreeVisitor* visitor) const { SourceElementMark sem(visitor, *this); }

void RawCompoundIdentifier::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  for (auto& i : components) {
    visitor->OnIdentifier(i);
  }
}

std::string RawCompoundIdentifier::ToString() const {
  std::string str;
  str.reserve(span().data().size());
  for (auto it = components.begin(); it != components.end(); ++it) {
    if (it != components.begin())
      str.push_back('.');
    str.append((*it)->span().data());
  }
  return str;
}

void RawDocCommentLiteral::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
}

void RawStringLiteral::Accept(TreeVisitor* visitor) const { SourceElementMark sem(visitor, *this); }

void RawNumericLiteral::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
}

void RawBoolLiteral::Accept(TreeVisitor* visitor) const { SourceElementMark sem(visitor, *this); }

void RawIdentifierConstant::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  visitor->OnCompoundIdentifier(identifier);
}

void RawLiteralConstant::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  visitor->OnLiteral(literal);
}

void RawBinaryOperatorConstant::Accept(TreeVisitor* visitor) const {
  // TODO(https://fxbug.dev/42120097): Visit the operator as well.
  SourceElementMark sem(visitor, *this);
  visitor->OnConstant(left_operand);
  visitor->OnConstant(right_operand);
}

void RawOrdinal64::Accept(TreeVisitor* visitor) const { SourceElementMark sem(visitor, *this); }

void RawAttributeArg::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (maybe_name != nullptr) {
    visitor->OnIdentifier(maybe_name);
  }
  visitor->OnConstant(value);
}

void RawAttribute::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  for (auto& i : args) {
    visitor->OnAttributeArg(i);
  }
}

void RawAttributeList::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  for (auto& i : attributes) {
    visitor->OnAttribute(i);
  }
}

void RawModifier::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (auto& attribute = maybe_available_attribute) {
    visitor->OnAttribute(attribute);
  }
}

void RawModifierList::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  for (auto& i : modifiers) {
    visitor->OnModifier(i);
  }
}

void RawLibraryDeclaration::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }
  visitor->OnCompoundIdentifier(path);
}

void RawUsing::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }
  visitor->OnCompoundIdentifier(using_path);
  if (maybe_alias != nullptr) {
    visitor->OnIdentifier(maybe_alias);
  }
}

void RawAliasDeclaration::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }
  visitor->OnIdentifier(alias);
  visitor->OnTypeConstructor(type_ctor);
}

void RawConstDeclaration::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }
  visitor->OnIdentifier(identifier);
  visitor->OnTypeConstructor(type_ctor);
  visitor->OnConstant(constant);
}

void RawParameterList::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (type_ctor) {
    visitor->OnTypeConstructor(type_ctor);
  }
}

void RawProtocolMethod::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }
  if (modifiers != nullptr) {
    visitor->OnModifierList(modifiers);
  }
  visitor->OnIdentifier(identifier);
  if (maybe_request != nullptr) {
    visitor->OnParameterList(maybe_request);
  }
  if (maybe_response != nullptr) {
    visitor->OnParameterList(maybe_response);
  }
  if (maybe_error_ctor != nullptr) {
    visitor->OnTypeConstructor(maybe_error_ctor);
  }
}

void RawProtocolCompose::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }
  visitor->OnCompoundIdentifier(protocol_name);
}

void RawProtocolDeclaration::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }
  if (modifiers != nullptr) {
    visitor->OnModifierList(modifiers);
  }
  visitor->OnIdentifier(identifier);
  for (const auto& composed_protocol : composed_protocols) {
    visitor->OnProtocolCompose(composed_protocol);
  }
  for (const auto& method : methods) {
    visitor->OnProtocolMethod(method);
  }
}

void RawResourceProperty::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }
  visitor->OnIdentifier(identifier);
  visitor->OnTypeConstructor(type_ctor);
}

void RawResourceDeclaration::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }
  visitor->OnIdentifier(identifier);
  if (maybe_type_ctor != nullptr) {
    visitor->OnTypeConstructor(maybe_type_ctor);
  }
  for (const auto& property : properties) {
    visitor->OnResourceProperty(property);
  }
}

void RawServiceMember::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }
  visitor->OnIdentifier(identifier);
  visitor->OnTypeConstructor(type_ctor);
}

void RawServiceDeclaration::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }
  visitor->OnIdentifier(identifier);
  for (const auto& member : members) {
    visitor->OnServiceMember(member);
  }
}

void RawIdentifierLayoutParameter::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  visitor->OnCompoundIdentifier(identifier);
}

void RawLiteralLayoutParameter::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  visitor->OnLiteralConstant(literal);
}

void RawTypeLayoutParameter::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  visitor->OnTypeConstructor(type_ctor);
}

void RawLayoutParameterList::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  for (auto& item : items) {
    visitor->OnLayoutParameter(item);
  }
}

void RawOrdinaledLayoutMember::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }

  visitor->OnOrdinal64(*ordinal);
  visitor->OnIdentifier(identifier);
  if (type_ctor != nullptr) {
    visitor->OnTypeConstructor(type_ctor);
  }
}

void RawStructLayoutMember::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }

  visitor->OnIdentifier(identifier);
  visitor->OnTypeConstructor(type_ctor);
  if (default_value != nullptr) {
    visitor->OnConstant(default_value);
  }
}

void RawValueLayoutMember::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }

  visitor->OnIdentifier(identifier);
  visitor->OnConstant(value);
}

void RawLayout::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (modifiers != nullptr) {
    visitor->OnModifierList(modifiers);
  }
  if (subtype_ctor != nullptr) {
    visitor->OnTypeConstructor(subtype_ctor);
  }
  for (auto& member : members) {
    visitor->OnLayoutMember(member);
  }
}

void RawInlineLayoutReference::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }
  visitor->OnLayout(layout);
}

void RawNamedLayoutReference::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  visitor->OnCompoundIdentifier(identifier);
}

void RawTypeConstraints::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  for (auto& item : items) {
    visitor->OnConstant(item);
  }
}

void RawTypeConstructor::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  visitor->OnLayoutReference(layout_ref);
  if (parameters != nullptr) {
    visitor->OnLayoutParameterList(parameters);
  }
  if (constraints != nullptr) {
    visitor->OnTypeConstraints(constraints);
  }
}

void RawTypeDeclaration::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  if (attributes != nullptr) {
    visitor->OnAttributeList(attributes);
  }

  visitor->OnIdentifier(identifier);
  visitor->OnTypeConstructor(type_ctor);
}

void File::Accept(TreeVisitor* visitor) const {
  SourceElementMark sem(visitor, *this);
  visitor->OnLibraryDeclaration(library_decl);
  for (auto& i : using_list) {
    visitor->OnUsing(i);
  }
  for (auto& i : alias_list) {
    visitor->OnAliasDeclaration(i);
  }
  for (auto& i : const_declaration_list) {
    visitor->OnConstDeclaration(i);
  }
  for (auto& i : protocol_declaration_list) {
    visitor->OnProtocolDeclaration(i);
  }
  for (auto& i : resource_declaration_list) {
    visitor->OnResourceDeclaration(i);
  }
  for (auto& i : service_declaration_list) {
    visitor->OnServiceDeclaration(i);
  }
  for (auto& i : type_decls) {
    visitor->OnTypeDeclaration(i);
  }
}

}  // namespace fidlc
