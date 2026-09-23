// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_LINTING_TREE_CALLBACKS_H_
#define TOOLS_FIDL_FIDLC_SRC_LINTING_TREE_CALLBACKS_H_

#include <lib/fit/function.h>

#include <vector>

#include "lib/stdcompat/span.h"
#include "tools/fidl/fidlc/src/source_span.h"
#include "tools/fidl/fidlc/src/tree_visitor.h"

namespace fidlc {

// Supports TreeVisitor actions via delegation instead of inheritance, by
// wrapping a TreeVisitor subclass that calls a list of callbacks for
// each visitor method. In other words, this class implements a hardcoded "map"
// from each source node type (represented by its TreeVisitor method) to a set
// of callbacks, rather than implementing the callback logic directly inside
// the overridden method.
class LintingTreeCallbacks {
 public:
  // Construct a new callbacks container. Call "On" methods, for each event
  // type (such as "OnAttribute"), to register a callback for that event.
  LintingTreeCallbacks();

  // Process a file (initiates the callbacks as each element is visited for
  // the given parsed source file).
  void Visit(const std::unique_ptr<File>& element);

  // Register a callback for a "File" event. All of the remaining "On"
  // functions similarly match their corresponding TreeVisitor methods.
  void OnFile(fit::function<void(const File&)> callback) {
    file_callbacks_.push_back(std::move(callback));
  }
  void OnExitFile(fit::function<void(const File&)> callback) {
    exit_file_callbacks_.push_back(std::move(callback));
  }

  void OnSourceElement(fit::function<void(const SourceElement&)> callback) {
    source_element_callbacks_.push_back(std::move(callback));
  }

  // If comments lines are broken by more than one newline, they are treated as separate comments.
  void OnComment(fit::function<void(const cpp20::span<const SourceSpan>)> callback) {
    comment_callbacks_.push_back(std::move(callback));
  }
  void OnIgnoredToken(fit::function<void(const Token&)> callback) {
    ignored_token_callbacks_.push_back(std::move(callback));
  }

  void OnAliasDeclaration(fit::function<void(const RawAliasDeclaration&)> callback) {
    alias_callbacks_.push_back(std::move(callback));
  }
  void OnUsing(fit::function<void(const RawUsing&)> callback) {
    using_callbacks_.push_back(std::move(callback));
  }
  void OnConstDeclaration(fit::function<void(const RawConstDeclaration&)> callback) {
    const_declaration_callbacks_.push_back(std::move(callback));
  }
  void OnExitConstDeclaration(fit::function<void(const RawConstDeclaration&)> callback) {
    exit_const_declaration_callbacks_.push_back(std::move(callback));
  }
  void OnProtocolDeclaration(fit::function<void(const RawProtocolDeclaration&)> callback) {
    protocol_declaration_callbacks_.push_back(std::move(callback));
  }
  void OnExitProtocolDeclaration(fit::function<void(const RawProtocolDeclaration&)> callback) {
    exit_protocol_declaration_callbacks_.push_back(std::move(callback));
  }
  void OnMethod(fit::function<void(const RawProtocolMethod&)> callback) {
    method_callbacks_.push_back(std::move(callback));
  }
  void OnEvent(fit::function<void(const RawProtocolMethod&)> callback) {
    event_callbacks_.push_back(std::move(callback));
  }
  void OnAttribute(fit::function<void(const RawAttribute&)> callback) {
    attribute_callbacks_.push_back(std::move(callback));
  }
  void OnOrdinaledLayoutMember(fit::function<void(const RawOrdinaledLayoutMember&)> callback) {
    ordinaled_layout_member_callbacks_.push_back(std::move(callback));
  }
  void OnStructLayoutMember(fit::function<void(const RawStructLayoutMember&)> callback) {
    struct_layout_member_callbacks_.push_back(std::move(callback));
  }
  void OnValueLayoutMember(fit::function<void(const RawValueLayoutMember&)> callback) {
    value_layout_member_callbacks_.push_back(std::move(callback));
  }
  void OnLayout(fit::function<void(const RawLayout&)> callback) {
    layout_callbacks_.push_back(std::move(callback));
  }
  void OnExitLayout(fit::function<void(const RawLayout&)> callback) {
    exit_layout_callbacks_.push_back(std::move(callback));
  }
  void OnTypeDeclaration(fit::function<void(const RawTypeDeclaration&)> callback) {
    type_decl_callbacks_.push_back(std::move(callback));
  }
  void OnExitTypeDeclaration(fit::function<void(const RawTypeDeclaration&)> callback) {
    exit_type_decl_callbacks_.push_back(std::move(callback));
  }
  void OnIdentifierLayoutParameter(
      fit::function<void(const RawIdentifierLayoutParameter&)> callback) {
    identifier_layout_parameter_callbacks_.push_back(std::move(callback));
  }
  void OnTypeConstructor(fit::function<void(const RawTypeConstructor&)> callback) {
    type_constructor_callbacks_.push_back(std::move(callback));
  }

 private:
  // tree_visitor_ is initialized to a locally-defined class
  // |CallbackTreeVisitor| defined in the out-of-line implementation of the
  // LintingTreeCallbacks constructor.
  //
  // The CallbackTreeVisitor overrides TreeVisitor to call the registered
  // callback methods. It is not necessary to define the class inline here.
  // We avoid having to declare all of the overridden methods unnecessarily
  // in this header; and avoid the alternative--defining the methods inline,
  // in this header--which would make including the header a costly thing to
  // do.
  std::unique_ptr<TreeVisitor> tree_visitor_;

  std::vector<fit::function<void(const File&)>> file_callbacks_;
  std::vector<fit::function<void(const File&)>> exit_file_callbacks_;
  std::vector<fit::function<void(const SourceElement&)>> source_element_callbacks_;
  std::vector<fit::function<void(const cpp20::span<const SourceSpan>)>> comment_callbacks_;
  std::vector<fit::function<void(const Token&)>> ignored_token_callbacks_;
  std::vector<fit::function<void(const RawAliasDeclaration&)>> alias_callbacks_;
  std::vector<fit::function<void(const RawUsing&)>> using_callbacks_;
  std::vector<fit::function<void(const RawConstDeclaration&)>> const_declaration_callbacks_;
  std::vector<fit::function<void(const RawConstDeclaration&)>> exit_const_declaration_callbacks_;
  std::vector<fit::function<void(const RawProtocolDeclaration&)>> protocol_declaration_callbacks_;
  std::vector<fit::function<void(const RawProtocolDeclaration&)>>
      exit_protocol_declaration_callbacks_;
  std::vector<fit::function<void(const RawProtocolMethod&)>> method_callbacks_;
  std::vector<fit::function<void(const RawProtocolMethod&)>> event_callbacks_;

  std::vector<fit::function<void(const RawAttribute&)>> attribute_callbacks_;
  std::vector<fit::function<void(const RawOrdinaledLayoutMember&)>>
      ordinaled_layout_member_callbacks_;
  std::vector<fit::function<void(const RawStructLayoutMember&)>> struct_layout_member_callbacks_;
  std::vector<fit::function<void(const RawValueLayoutMember&)>> value_layout_member_callbacks_;
  std::vector<fit::function<void(const RawLayout&)>> layout_callbacks_;
  std::vector<fit::function<void(const RawLayout&)>> exit_layout_callbacks_;
  std::vector<fit::function<void(const RawIdentifierLayoutParameter&)>>
      identifier_layout_parameter_callbacks_;
  std::vector<fit::function<void(const RawTypeDeclaration&)>> type_decl_callbacks_;
  std::vector<fit::function<void(const RawTypeDeclaration&)>> exit_type_decl_callbacks_;
  std::vector<fit::function<void(const RawTypeConstructor&)>> type_constructor_callbacks_;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_LINTING_TREE_CALLBACKS_H_
