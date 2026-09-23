// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/linting_tree_callbacks.h"

#include <zircon/assert.h>

#include "lib/stdcompat/span.h"
#include "re2/re2.h"

namespace fidlc {

LintingTreeCallbacks::LintingTreeCallbacks() {
  // Anonymous derived class; unique to the LintingTreeCallbacks
  class CallbackTreeVisitor : public DeclarationOrderTreeVisitor {
   private:
   public:
    explicit CallbackTreeVisitor(const LintingTreeCallbacks& callbacks) : callbacks_(callbacks) {}

    void OnFile(const std::unique_ptr<File>& element) override {
      tokens_ = element->tokens;

      for (auto& callback : callbacks_.file_callbacks_) {
        callback(*element);
      }
      DeclarationOrderTreeVisitor::OnFile(element);
      for (auto& callback : callbacks_.exit_file_callbacks_) {
        callback(*element);
      }
    }
    void OnSourceElementStart(const SourceElement& element) override {
      ProcessGaps(element.start_token);
      for (auto& callback : callbacks_.source_element_callbacks_) {
        callback(element);
      }
    }
    void OnSourceElementEnd(const SourceElement& element) override {
      ProcessGaps(element.end_token);
    }
    void OnAliasDeclaration(const std::unique_ptr<RawAliasDeclaration>& element) override {
      ProcessGaps(element->start_token);
      for (auto& callback : callbacks_.alias_callbacks_) {
        callback(*element);
      }
      DeclarationOrderTreeVisitor::OnAliasDeclaration(element);
      ProcessGaps(element->end_token);
    }
    void OnUsing(const std::unique_ptr<RawUsing>& element) override {
      ProcessGaps(element->start_token);
      for (auto& callback : callbacks_.using_callbacks_) {
        callback(*element);
      }
      DeclarationOrderTreeVisitor::OnUsing(element);
      ProcessGaps(element->end_token);
    }
    void OnConstDeclaration(const std::unique_ptr<RawConstDeclaration>& element) override {
      ProcessGaps(element->start_token);
      for (auto& callback : callbacks_.const_declaration_callbacks_) {
        callback(*element);
      }
      DeclarationOrderTreeVisitor::OnConstDeclaration(element);
      for (auto& callback : callbacks_.exit_const_declaration_callbacks_) {
        callback(*element);
      }
      ProcessGaps(element->end_token);
    }
    void OnProtocolDeclaration(const std::unique_ptr<RawProtocolDeclaration>& element) override {
      ProcessGaps(element->start_token);
      for (auto& callback : callbacks_.protocol_declaration_callbacks_) {
        callback(*element);
      }
      DeclarationOrderTreeVisitor::OnProtocolDeclaration(element);
      for (auto& callback : callbacks_.exit_protocol_declaration_callbacks_) {
        callback(*element);
      }
      ProcessGaps(element->end_token);
    }
    void OnProtocolMethod(const std::unique_ptr<RawProtocolMethod>& element) override {
      ProcessGaps(element->start_token);
      if (element->maybe_request != nullptr) {
        for (auto& callback : callbacks_.method_callbacks_) {
          callback(*element);
        }
      } else {
        for (auto& callback : callbacks_.event_callbacks_) {
          callback(*element);
        }
      }
      DeclarationOrderTreeVisitor::OnProtocolMethod(element);
      ProcessGaps(element->end_token);
    }
    void OnAttribute(const std::unique_ptr<RawAttribute>& element) override {
      for (auto& callback : callbacks_.attribute_callbacks_) {
        callback(*element);
      }
    }
    void OnOrdinaledLayoutMember(
        const std::unique_ptr<RawOrdinaledLayoutMember>& element) override {
      ProcessGaps(element->start_token);
      for (auto& callback : callbacks_.ordinaled_layout_member_callbacks_) {
        callback(*element);
      }
      DeclarationOrderTreeVisitor::OnOrdinaledLayoutMember(element);
      ProcessGaps(element->end_token);
    }
    void OnStructLayoutMember(const std::unique_ptr<RawStructLayoutMember>& element) override {
      ProcessGaps(element->start_token);
      for (auto& callback : callbacks_.struct_layout_member_callbacks_) {
        callback(*element);
      }
      DeclarationOrderTreeVisitor::OnStructLayoutMember(element);
      ProcessGaps(element->end_token);
    }
    void OnValueLayoutMember(const std::unique_ptr<RawValueLayoutMember>& element) override {
      ProcessGaps(element->start_token);
      for (auto& callback : callbacks_.value_layout_member_callbacks_) {
        callback(*element);
      }
      DeclarationOrderTreeVisitor::OnValueLayoutMember(element);
      ProcessGaps(element->end_token);
    }
    void OnLayout(const std::unique_ptr<RawLayout>& element) override {
      ProcessGaps(element->start_token);
      for (auto& callback : callbacks_.layout_callbacks_) {
        callback(*element);
      }
      DeclarationOrderTreeVisitor::OnLayout(element);
      for (auto& callback : callbacks_.exit_layout_callbacks_) {
        callback(*element);
      }
      ProcessGaps(element->end_token);
    }
    void OnTypeDeclaration(const std::unique_ptr<RawTypeDeclaration>& element) override {
      ProcessGaps(element->start_token);
      for (auto& callback : callbacks_.type_decl_callbacks_) {
        callback(*element);
      }
      DeclarationOrderTreeVisitor::OnTypeDeclaration(element);
      for (auto& callback : callbacks_.exit_type_decl_callbacks_) {
        callback(*element);
      }
      ProcessGaps(element->end_token);
    }
    void OnIdentifierLayoutParameter(
        const std::unique_ptr<RawIdentifierLayoutParameter>& element) override {
      // For the time being, the the first type parameter in a layout must either be a
      // TypeConstructor (like `vector<uint8>`), or else a reference to on (like `vector<Foo>`).
      // Because of this, we can treat an IdentifierLayoutParameter as a TypeConstructor for the
      // purposes of linting.
      for (auto& callback : callbacks_.identifier_layout_parameter_callbacks_) {
        callback(*element);
      }
      DeclarationOrderTreeVisitor::OnIdentifierLayoutParameter(element);
    }
    void OnTypeConstructor(const std::unique_ptr<RawTypeConstructor>& element) override {
      for (auto& callback : callbacks_.type_constructor_callbacks_) {
        callback(*element);
      }
      DeclarationOrderTreeVisitor::OnTypeConstructor(element);
    }

   private:
    void OnComment(const cpp20::span<const SourceSpan> comment_lines) {
      for (auto& callback : callbacks_.comment_callbacks_) {
        callback(comment_lines);
      }
    }

    void ProcessGaps(const Token& next_non_gap_token) {
      std::vector<SourceSpan> current_comment_block;
      while (tokens_[next_token_index_].ptr() < next_non_gap_token.ptr()) {
        const Token current_token = tokens_[next_token_index_];
        if (current_token.kind() == Token::kComment) {
          if (current_token.leading_newlines() > 1) {
            OnComment(current_comment_block);
            current_comment_block.clear();
          }
          current_comment_block.emplace_back(current_token.span());
        } else {
          if (!current_comment_block.empty()) {
            OnComment(current_comment_block);
            current_comment_block.clear();
          }
          for (auto& callback : callbacks_.ignored_token_callbacks_) {
            // Includes (but may not be limited to): "as" : ; , { } [ ] ( )
            callback(current_token);
          }
        }
        next_token_index_++;
      }

      if (!current_comment_block.empty()) {
        OnComment(current_comment_block);
        current_comment_block.clear();
      }
    }

    const LintingTreeCallbacks& callbacks_;

    // An ordered list of all tokens (including comments) in the current source file.
    cpp20::span<Token> tokens_;

    // The index of the next token to be visited.
    size_t next_token_index_ = 0;
  };

  tree_visitor_ = std::make_unique<CallbackTreeVisitor>(*this);
}  // namespace linter

void LintingTreeCallbacks::Visit(const std::unique_ptr<File>& element) {
  tree_visitor_->OnFile(element);
}

}  // namespace fidlc
