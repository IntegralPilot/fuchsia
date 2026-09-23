// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/raw_ast.h"

#ifndef TOOLS_FIDL_FIDLC_SRC_TREE_VISITOR_H_
#define TOOLS_FIDL_FIDLC_SRC_TREE_VISITOR_H_

namespace fidlc {

// A TreeVisitor is an API that walks a FIDL AST.  The default implementation
// does nothing but walk the AST.  To make it interesting, subclass TreeVisitor
// and override behaviors with the ones you want.
class TreeVisitor {
 public:
  virtual ~TreeVisitor() = default;

  virtual void OnSourceElementStart(const SourceElement& element) {}
  virtual void OnSourceElementEnd(const SourceElement& element) {}
  virtual void OnIdentifier(const std::unique_ptr<RawIdentifier>& element) {
    element->Accept(this);
  }
  virtual void OnCompoundIdentifier(const std::unique_ptr<RawCompoundIdentifier>& element) {
    element->Accept(this);
  }

  virtual void OnLiteral(const std::unique_ptr<RawLiteral>& element) {
    switch (element->kind) {
      case RawLiteral::Kind::kDocComment:
        OnDocCommentLiteral(*static_cast<RawDocCommentLiteral*>(element.get()));
        break;
      case RawLiteral::Kind::kString:
        OnStringLiteral(*static_cast<RawStringLiteral*>(element.get()));
        break;
      case RawLiteral::Kind::kNumeric:
        OnNumericLiteral(*static_cast<RawNumericLiteral*>(element.get()));
        break;
      case RawLiteral::Kind::kBool:
        OnBoolLiteral(*static_cast<RawBoolLiteral*>(element.get()));
        break;
    }
  }
  virtual void OnDocCommentLiteral(RawDocCommentLiteral& element) { element.Accept(this); }

  virtual void OnStringLiteral(RawStringLiteral& element) { element.Accept(this); }

  virtual void OnNumericLiteral(RawNumericLiteral& element) { element.Accept(this); }

  virtual void OnBoolLiteral(RawBoolLiteral& element) { element.Accept(this); }

  virtual void OnOrdinal64(RawOrdinal64& element) { element.Accept(this); }

#ifdef DISPATCH_TO
#error "Cannot define macro DISPATCH_TO: already defined"
#endif
#define DISPATCH_TO(TYPE, SUPERTYPE, ELEMENT)                        \
  do {                                                               \
    std::unique_ptr<SUPERTYPE>& unconst_element =                    \
        const_cast<std::unique_ptr<SUPERTYPE>&>(element);            \
    Raw##TYPE* ptr = static_cast<Raw##TYPE*>(unconst_element.get()); \
    std::unique_ptr<Raw##TYPE> uptr(ptr);                            \
    On##TYPE(uptr);                                                  \
    static_cast<void>(uptr.release());                               \
  } while (0)

  virtual void OnConstant(const std::unique_ptr<RawConstant>& element) {
    RawConstant::Kind kind = element->kind;
    switch (kind) {
      case RawConstant::Kind::kIdentifier: {
        DISPATCH_TO(IdentifierConstant, RawConstant, element);
        break;
      }
      case RawConstant::Kind::kLiteral: {
        DISPATCH_TO(LiteralConstant, RawConstant, element);
        break;
      }
      case RawConstant::Kind::kBinaryOperator: {
        DISPATCH_TO(BinaryOperatorConstant, RawConstant, element);
        break;
      }
    }
  }

  virtual void OnIdentifierConstant(const std::unique_ptr<RawIdentifierConstant>& element) {
    element->Accept(this);
  }
  virtual void OnLiteralConstant(const std::unique_ptr<RawLiteralConstant>& element) {
    element->Accept(this);
  }
  virtual void OnBinaryOperatorConstant(const std::unique_ptr<RawBinaryOperatorConstant>& element) {
    element->Accept(this);
  }

  virtual void OnAttributeArg(const std::unique_ptr<RawAttributeArg>& element) {
    element->Accept(this);
  }

  virtual void OnAttribute(const std::unique_ptr<RawAttribute>& element) { element->Accept(this); }

  virtual void OnAttributeList(const std::unique_ptr<RawAttributeList>& element) {
    element->Accept(this);
  }

  virtual void OnModifier(const std::unique_ptr<RawModifier>& element) { element->Accept(this); }

  virtual void OnModifierList(const std::unique_ptr<RawModifierList>& element) {
    element->Accept(this);
  }

  virtual void OnAliasDeclaration(const std::unique_ptr<RawAliasDeclaration>& element) {
    element->Accept(this);
  }

  virtual void OnLibraryDeclaration(const std::unique_ptr<RawLibraryDeclaration>& element) {
    element->Accept(this);
  }

  virtual void OnUsing(const std::unique_ptr<RawUsing>& element) { element->Accept(this); }

  virtual void OnConstDeclaration(const std::unique_ptr<RawConstDeclaration>& element) {
    element->Accept(this);
  }

  virtual void OnParameterList(const std::unique_ptr<RawParameterList>& element) {
    element->Accept(this);
  }
  virtual void OnProtocolMethod(const std::unique_ptr<RawProtocolMethod>& element) {
    element->Accept(this);
  }
  virtual void OnProtocolCompose(const std::unique_ptr<RawProtocolCompose>& element) {
    element->Accept(this);
  }
  virtual void OnProtocolDeclaration(const std::unique_ptr<RawProtocolDeclaration>& element) {
    element->Accept(this);
  }
  virtual void OnResourceProperty(const std::unique_ptr<RawResourceProperty>& element) {
    element->Accept(this);
  }
  virtual void OnResourceDeclaration(const std::unique_ptr<RawResourceDeclaration>& element) {
    element->Accept(this);
  }
  virtual void OnServiceMember(const std::unique_ptr<RawServiceMember>& element) {
    element->Accept(this);
  }
  virtual void OnServiceDeclaration(const std::unique_ptr<RawServiceDeclaration>& element) {
    element->Accept(this);
  }

  virtual void OnLayoutParameter(const std::unique_ptr<RawLayoutParameter>& element) {
    RawLayoutParameter::Kind kind = element->kind;
    switch (kind) {
      case RawLayoutParameter::Kind::kIdentifier: {
        DISPATCH_TO(IdentifierLayoutParameter, RawLayoutParameter, element);
        break;
      }
      case RawLayoutParameter::Kind::kLiteral: {
        DISPATCH_TO(LiteralLayoutParameter, RawLayoutParameter, element);
        break;
      }
      case RawLayoutParameter::Kind::kType: {
        DISPATCH_TO(TypeLayoutParameter, RawLayoutParameter, element);
        break;
      }
    }
  }

  virtual void OnLayoutParameterList(const std::unique_ptr<RawLayoutParameterList>& element);

  virtual void OnIdentifierLayoutParameter(
      const std::unique_ptr<RawIdentifierLayoutParameter>& element) {
    element->Accept(this);
  }
  virtual void OnLiteralLayoutParameter(const std::unique_ptr<RawLiteralLayoutParameter>& element) {
    element->Accept(this);
  }
  virtual void OnTypeLayoutParameter(const std::unique_ptr<RawTypeLayoutParameter>& element) {
    element->Accept(this);
  }

  virtual void OnLayoutMember(const std::unique_ptr<RawLayoutMember>& element) {
    RawLayoutMember::Kind kind = element->kind;
    switch (kind) {
      case RawLayoutMember::Kind::kOrdinaled: {
        DISPATCH_TO(OrdinaledLayoutMember, RawLayoutMember, element);
        break;
      }
      case RawLayoutMember::Kind::kStruct: {
        DISPATCH_TO(StructLayoutMember, RawLayoutMember, element);
        break;
      }
      case RawLayoutMember::Kind::kValue: {
        DISPATCH_TO(ValueLayoutMember, RawLayoutMember, element);
        break;
      }
    }
  }

  virtual void OnOrdinaledLayoutMember(const std::unique_ptr<RawOrdinaledLayoutMember>& element) {
    element->Accept(this);
  }
  virtual void OnStructLayoutMember(const std::unique_ptr<RawStructLayoutMember>& element) {
    element->Accept(this);
  }
  virtual void OnValueLayoutMember(const std::unique_ptr<RawValueLayoutMember>& element) {
    element->Accept(this);
  }

  virtual void OnLayout(const std::unique_ptr<RawLayout>& element) { element->Accept(this); }

  virtual void OnLayoutReference(const std::unique_ptr<RawLayoutReference>& element) {
    RawLayoutReference::Kind kind = element->kind;
    switch (kind) {
      case RawLayoutReference::Kind::kInline: {
        DISPATCH_TO(InlineLayoutReference, RawLayoutReference, element);
        break;
      }
      case RawLayoutReference::Kind::kNamed: {
        DISPATCH_TO(NamedLayoutReference, RawLayoutReference, element);
        break;
      }
    }
  }

  virtual void OnInlineLayoutReference(const std::unique_ptr<RawInlineLayoutReference>& element) {
    element->Accept(this);
  }
  virtual void OnNamedLayoutReference(const std::unique_ptr<RawNamedLayoutReference>& element) {
    element->Accept(this);
  }

  virtual void OnTypeConstraints(const std::unique_ptr<RawTypeConstraints>& element) {
    element->Accept(this);
  }

  virtual void OnTypeConstructor(const std::unique_ptr<RawTypeConstructor>& element) {
    element->Accept(this);
  }

  virtual void OnTypeDeclaration(const std::unique_ptr<RawTypeDeclaration>& element) {
    element->Accept(this);
  }

  virtual void OnFile(const std::unique_ptr<File>& element) { element->Accept(this); }
};

#undef DISPATCH_TO

// AST node contents are not stored in declaration order in the tree, so we
// have a special visitor for code that needs to visit in declaration order.
class DeclarationOrderTreeVisitor : public TreeVisitor {
 public:
  void OnFile(const std::unique_ptr<File>& element) override;
  void OnProtocolDeclaration(const std::unique_ptr<RawProtocolDeclaration>& element) override;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_TREE_VISITOR_H_
