// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/tree_visitor.h"

#include <map>

#include "tools/fidl/fidlc/src/raw_ast.h"

namespace fidlc {

void TreeVisitor::OnLayoutParameterList(const std::unique_ptr<RawLayoutParameterList>& element) {
  element->Accept(this);
}

void DeclarationOrderTreeVisitor::OnFile(const std::unique_ptr<File>& element) {
  OnSourceElementStart(*element);
  OnLibraryDeclaration(element->library_decl);

  auto alias_decls_it = element->alias_list.begin();
  auto const_decls_it = element->const_declaration_list.begin();
  auto protocol_decls_it = element->protocol_declaration_list.begin();
  auto resource_decls_it = element->resource_declaration_list.begin();
  auto service_decls_it = element->service_declaration_list.begin();
  auto type_decls_it = element->type_decls.begin();
  auto using_decls_it = element->using_list.begin();

  enum Next {
    kAlias,
    kConst,
    kProtocol,
    kResource,
    kService,
    kTypeDecl,
    kUsing,
  };

  std::map<const char*, Next> m;
  for (;;) {
    // We want to visit these in declaration order, rather than grouped by type
    // of declaration, so we put them in a map keyed by source position.
    m.clear();
    if (alias_decls_it != element->alias_list.end()) {
      m[(*alias_decls_it)->start_token.ptr()] = kAlias;
    }
    if (const_decls_it != element->const_declaration_list.end()) {
      m[(*const_decls_it)->start_token.ptr()] = kConst;
    }
    if (protocol_decls_it != element->protocol_declaration_list.end()) {
      if (*protocol_decls_it == nullptr) {
        // Used to indicate empty, so let's wind it forward.
        protocol_decls_it = element->protocol_declaration_list.end();
      } else {
        m[(*protocol_decls_it)->start_token.ptr()] = kProtocol;
      }
    }
    if (resource_decls_it != element->resource_declaration_list.end()) {
      m[(*resource_decls_it)->start_token.ptr()] = kResource;
    }
    if (service_decls_it != element->service_declaration_list.end()) {
      m[(*service_decls_it)->start_token.ptr()] = kService;
    }
    if (type_decls_it != element->type_decls.end()) {
      m[(*type_decls_it)->start_token.ptr()] = kTypeDecl;
    }
    if (using_decls_it != element->using_list.end()) {
      m[(*using_decls_it)->start_token.ptr()] = kUsing;
    }
    if (m.empty())
      break;

    // And the earliest top level declaration is...
    switch (m.begin()->second) {
      case kAlias:
        OnAliasDeclaration(*alias_decls_it);
        ++alias_decls_it;
        break;
      case kConst:
        OnConstDeclaration(*const_decls_it);
        ++const_decls_it;
        break;
      case kProtocol:
        OnProtocolDeclaration(*protocol_decls_it);
        ++protocol_decls_it;
        break;
      case kResource:
        OnResourceDeclaration(*resource_decls_it);
        ++resource_decls_it;
        break;
      case kService:
        OnServiceDeclaration(*service_decls_it);
        ++service_decls_it;
        break;
      case kTypeDecl:
        OnTypeDeclaration(*type_decls_it);
        ++type_decls_it;
        break;
      case kUsing:
        OnUsing(*using_decls_it);
        ++using_decls_it;
        break;
    }
  }
  OnSourceElementEnd(*element);
}

void DeclarationOrderTreeVisitor::OnProtocolDeclaration(
    const std::unique_ptr<RawProtocolDeclaration>& element) {
  SourceElementMark sem(this, *element);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }
  if (element->modifiers != nullptr) {
    OnModifierList(element->modifiers);
  }
  OnIdentifier(element->identifier);

  auto compose_it = element->composed_protocols.begin();
  auto methods_it = element->methods.begin();

  enum Next {
    kCompose,
    kMethod,
  };

  std::map<const char*, Next> m;
  for (;;) {
    // Sort in declaration order.
    m.clear();
    if (compose_it != element->composed_protocols.end()) {
      m[(*compose_it)->start_token.ptr()] = kCompose;
    }
    if (methods_it != element->methods.end()) {
      m[(*methods_it)->start_token.ptr()] = kMethod;
    }
    if (m.empty())
      return;

    // And the earliest declaration is...
    switch (m.begin()->second) {
      case kCompose:
        OnProtocolCompose(*compose_it);
        ++compose_it;
        break;
      case kMethod:
        OnProtocolMethod(*methods_it);
        ++methods_it;
        break;
    }
  }
}

}  // namespace fidlc
