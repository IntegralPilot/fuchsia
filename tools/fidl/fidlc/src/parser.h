// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_PARSER_H_
#define TOOLS_FIDL_FIDLC_SRC_PARSER_H_

#include <zircon/assert.h>

#include <memory>
#include <optional>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/experimental_flags.h"
#include "tools/fidl/fidlc/src/lexer.h"
#include "tools/fidl/fidlc/src/properties.h"
#include "tools/fidl/fidlc/src/raw_ast.h"
#include "tools/fidl/fidlc/src/reporter.h"
#include "tools/fidl/fidlc/src/token.h"

namespace fidlc {

// See https://fuchsia.dev/fuchsia-src/development/languages/fidl/reference/compiler#_parsing
// for additional context
class Parser {
 public:
  Parser(Lexer* lexer, Reporter* reporter, ExperimentalFlagSet experimental_flags);

  // Returns the parsed raw AST, or null if there were unrecoverable errors.
  std::unique_ptr<File> Parse() { return ParseFile(); }

  // Returns true if there were no errors, not even recovered ones.
  bool Success() const { return checkpoint_.NoNewErrors(); }

 private:
  // currently the only use case for this enum is to identify the case where the parser
  // has seen a doc comment block, followed by a regular comment block, followed by
  // a doc comment block
  enum class State : uint8_t {
    // the parser is currently in a doc comment block
    kDocCommentLast,
    // the parser is currently in a regular comment block, which directly followed a
    // doc comment block
    kDocCommentThenComment,
    // the parser is in kNormal for all other cases
    kNormal,
  };

  Token Lex() {
    for (;;) {
      auto token = lexer_->Lex();
      tokens_.emplace_back(token);

      switch (token.kind()) {
        case Token::Kind::kComment:
          if (state_ == State::kDocCommentLast)
            state_ = State::kDocCommentThenComment;
          break;
        case Token::Kind::kDocComment:
          if (state_ == State::kDocCommentThenComment)
            reporter_->Warn(WarnCommentWithinDocCommentBlock, last_token_.span());
          state_ = State::kDocCommentLast;
          return token;
        default:
          state_ = State::kNormal;
          return token;
      }
    }
  }

  Token::KindAndSubkind Peek() { return last_token_.kind_and_subkind(); }

  // ASTScope is a tool to track the start and end source location of each
  // node automatically.  The parser associates each node with the start and
  // end of its source location.  It also tracks the "gap" in between the
  // start and the previous interesting source element.  As we walk the tree,
  // we create ASTScope objects that can track the beginning and end of the
  // text associated with the Node being built.  The ASTScope object then
  // colludes with the Parser to figure out where the beginning and end of
  // that node are.
  //
  // ASTScope should only be created on the stack, when starting to parse
  // something that will result in a new AST node.
  class ASTScope {
   public:
    explicit ASTScope(Parser* parser) : parser_(parser) {
      parser_->active_ast_scopes_.emplace_back(Token(), Token());
    }
    SourceElement GetSourceElement() {
      parser_->active_ast_scopes_.back().end_token = parser_->previous_token_;
      return SourceElement(parser_->active_ast_scopes_.back());
    }
    ~ASTScope() { parser_->active_ast_scopes_.pop_back(); }

    ASTScope(const ASTScope&) = delete;
    ASTScope& operator=(const ASTScope&) = delete;

   private:
    Parser* parser_;
  };

  void UpdateMarks(Token& token) {
    // There should always be at least one of these - the outermost.
    ZX_ASSERT_MSG(!active_ast_scopes_.empty(), "unbalanced parse tree");

    for (auto& scope : active_ast_scopes_) {
      if (scope.start_token.kind() == Token::Kind::kNotAToken) {
        scope.start_token = token;
      }
    }

    previous_token_ = token;
  }

  bool ConsumedEOF() const { return previous_token_.kind() == Token::Kind::kEndOfFile; }

  enum class OnNoMatch : uint8_t {
    kReportAndConsume,  // on failure, report error and return consumed token
    kReportAndRecover,  // on failure, report error and return std::nullopt
    kIgnore,            // on failure, return std::nullopt
  };

  // ReadToken matches on the next token using the predicate |p|, which returns
  // a unique_ptr<Diagnostic> on failure, or nullptr on a match.
  // See #OfKind, and #IdentifierOfSubkind for the two most common predicates.
  // If the predicate doesn't match, ReadToken follows the OnNoMatch enum.
  // Must not be called again after returning Token::Kind::kEndOfFile.
  template <class Predicate>
  std::optional<Token> ReadToken(Predicate p, OnNoMatch on_no_match) {
    ZX_ASSERT_MSG(!ConsumedEOF(), "already consumed EOF");
    std::unique_ptr<Diagnostic> error = p(last_token_);
    if (error) {
      switch (on_no_match) {
        case OnNoMatch::kReportAndConsume:
          reporter_->Report(std::move(error));
          break;
        case OnNoMatch::kReportAndRecover:
          reporter_->Report(std::move(error));
          RecoverOneError();
          return std::nullopt;
        case OnNoMatch::kIgnore:
          return std::nullopt;
      }
    }
    auto token = previous_token_ = last_token_;
    // Don't lex any more if we hit EOF. Note: This means that after consuming
    // EOF, Peek() will make it seem as if there's a second EOF.
    if (token.kind() != Token::Kind::kEndOfFile) {
      last_token_ = Lex();
    }
    UpdateMarks(token);
    return token;
  }

  // ConsumeToken consumes a token whether or not it matches, and if it doesn't
  // match, it reports an error.
  template <class Predicate>
  std::optional<Token> ConsumeToken(Predicate p) {
    return ReadToken(p, OnNoMatch::kReportAndConsume);
  }

  // ConsumeTokenOrRecover consumes a token if-and-only-if it matches the given
  // Predicate |p|. If it doesn't match, it reports an error, then marks that
  // error as recovered, essentially continuing as if the token had been there.
  template <class Predicate>
  std::optional<Token> ConsumeTokenOrRecover(Predicate p) {
    return ReadToken(p, OnNoMatch::kReportAndRecover);
  }

  // MaybeConsumeToken consumes a token if-and-only-if it matches the given
  // predicate |p|.
  template <class Predicate>
  std::optional<Token> MaybeConsumeToken(Predicate p) {
    return ReadToken(p, OnNoMatch::kIgnore);
  }

  auto OfKind(Token::Kind expected_kind) {
    return [expected_kind](const Token& actual) -> std::unique_ptr<Diagnostic> {
      if (actual.kind() != expected_kind) {
        return Diagnostic::MakeError(ErrUnexpectedTokenOfKind, actual.span(),
                                     actual.kind_and_subkind(),
                                     Token::KindAndSubkind(expected_kind));
      }
      return nullptr;
    };
  }

  auto IdentifierOfSubkind(Token::Subkind expected_subkind) {
    return [expected_subkind](const Token& actual) -> std::unique_ptr<Diagnostic> {
      auto expected = Token::KindAndSubkind(expected_subkind);
      if (actual.kind_and_subkind().combined() != expected.combined()) {
        return Diagnostic::MakeError(ErrUnexpectedIdentifier, actual.span(),
                                     actual.kind_and_subkind(), expected);
      }
      return nullptr;
    };
  }

  // Parser defines these methods rather than using Reporter directly because:
  // * They skip reporting if there are already unrecovered errors.
  // * They use a default error, ErrUnexpectedToken.
  // * They use a default span, last_token_.span().
  // * They return nullptr rather than false.
  std::nullptr_t Fail();
  template <ErrorId Id, typename... Args>
  std::nullptr_t Fail(const ErrorDef<Id, Args...>& err,
                      const cpp20::type_identity_t<Args>&... args);
  template <ErrorId Id, typename... Args>
  std::nullptr_t Fail(const ErrorDef<Id, Args...>& err, Token token,
                      const cpp20::type_identity_t<Args>&... args);
  template <ErrorId Id, typename... Args>
  std::nullptr_t Fail(const ErrorDef<Id, Args...>& err, SourceSpan span,
                      const cpp20::type_identity_t<Args>&... args);

  // Reports an error if |modifiers| contains a modifier whose type is not
  // included in |Allowlist|. The |decl_token| should be "struct", "enum", etc.
  // Marks the error as recovered so that parsing will continue.
  template <typename... Allowlist>
  void ValidateModifiers(const std::unique_ptr<RawModifierList>& modifiers, Token decl_token);

  std::unique_ptr<RawIdentifier> ParseIdentifier();
  std::unique_ptr<RawCompoundIdentifier> ParseCompoundIdentifier();
  std::unique_ptr<RawCompoundIdentifier> ParseCompoundIdentifier(
      ASTScope& scope, std::unique_ptr<RawIdentifier> first_identifier);
  std::unique_ptr<RawLibraryDeclaration> ParseLibraryDeclaration();

  std::unique_ptr<RawStringLiteral> ParseStringLiteral();
  std::unique_ptr<RawNumericLiteral> ParseNumericLiteral();
  std::unique_ptr<RawBoolLiteral> ParseBoolLiteral(Token::Subkind subkind);
  std::unique_ptr<RawLiteral> ParseLiteral();
  std::unique_ptr<RawOrdinal64> ParseOrdinal64();

  std::unique_ptr<RawConstant> ParseConstant();
  std::unique_ptr<RawConstDeclaration> ParseConstDeclaration(
      std::unique_ptr<RawAttributeList> attributes, ASTScope&);

  std::unique_ptr<RawAliasDeclaration> ParseAliasDeclaration(
      std::unique_ptr<RawAttributeList> attributes, ASTScope&);
  std::unique_ptr<RawUsing> ParseUsing(std::unique_ptr<RawAttributeList> attributes, ASTScope&);

  std::unique_ptr<RawParameterList> ParseParameterList();
  std::unique_ptr<RawProtocolMethod> ParseProtocolEvent(
      std::unique_ptr<RawAttributeList> attributes, std::unique_ptr<RawModifierList> modifiers,
      ASTScope& scope);
  std::unique_ptr<RawProtocolMethod> ParseProtocolMethod(
      std::unique_ptr<RawAttributeList> attributes, std::unique_ptr<RawModifierList> modifiers,
      std::unique_ptr<RawIdentifier> method_name, ASTScope& scope);
  std::unique_ptr<RawProtocolCompose> ParseProtocolCompose(
      std::unique_ptr<RawAttributeList> attributes, ASTScope& scope);
  // ParseProtocolMember parses any one protocol member, i.e. an event,
  // a method, or a compose stanza.
  void ParseProtocolMember(std::vector<std::unique_ptr<RawProtocolCompose>>* composed_protocols,
                           std::vector<std::unique_ptr<RawProtocolMethod>>* methods);
  std::unique_ptr<RawProtocolDeclaration> ParseProtocolDeclaration(
      std::unique_ptr<RawAttributeList>, ASTScope&);
  std::unique_ptr<RawResourceProperty> ParseResourcePropertyDeclaration();
  // TODO(https://fxbug.dev/42143256): When we properly generalize handles, we will most
  // likely alter the name of a resource declaration, and how it looks
  // syntactically. While we rely on this feature in `library zx;`, it should
  // be considered experimental for all other intents and purposes.
  std::unique_ptr<RawResourceDeclaration> ParseResourceDeclaration(
      std::unique_ptr<RawAttributeList>, ASTScope&);
  std::unique_ptr<RawServiceMember> ParseServiceMember();
  // This method may be used to parse the second attribute argument onward - the first argument in
  // the list is handled separately in ParseAttribute().
  std::unique_ptr<RawAttributeArg> ParseSubsequentAttributeArg();
  std::unique_ptr<RawServiceDeclaration> ParseServiceDeclaration(std::unique_ptr<RawAttributeList>,
                                                                 ASTScope&);
  std::unique_ptr<RawAttribute> ParseAttribute();
  void ParseAttributeArgs(std::vector<std::unique_ptr<RawAttributeArg>>* out_args);
  std::unique_ptr<RawAttribute> ParseDocComment();
  std::unique_ptr<RawAttributeList> ParseAttributeList(std::unique_ptr<RawAttribute> doc_comment,
                                                       ASTScope& scope);
  std::unique_ptr<RawAttributeList> MaybeParseAttributeList();
  std::unique_ptr<RawModifierList> ParseModifierList(ASTScope& scope, Token first_token);
  std::unique_ptr<RawLayoutParameter> ParseLayoutParameter();
  std::unique_ptr<RawLayoutParameterList> MaybeParseLayoutParameterList();
  std::unique_ptr<RawLayoutMember> ParseLayoutMember(RawLayoutMember::Kind);
  std::unique_ptr<RawLayout> ParseLayout(ASTScope& scope,
                                         std::unique_ptr<RawModifierList> modifiers,
                                         std::unique_ptr<RawCompoundIdentifier> compound_identifier,
                                         std::unique_ptr<RawTypeConstructor> subtype_ctor);
  std::unique_ptr<RawTypeConstraints> ParseTypeConstraints();
  ConstraintOrSubtype ParseTokenAfterColon();

  std::unique_ptr<RawTypeConstructor> ParseTypeConstructor();
  std::unique_ptr<RawTypeDeclaration> ParseTypeDeclaration(
      std::unique_ptr<RawAttributeList> attributes, ASTScope&);
  std::unique_ptr<File> ParseFile();

  enum class RecoverResult : uint8_t {
    Failure,
    Continue,
    EndOfScope,
  };

  // Called when an error is encountered in parsing. Attempts to get the parser
  // back to a valid state, where parsing can continue. Possible results:
  //  * Failure: recovery failed. we are still in an invalid state and cannot
  //    continue.
  //    A signal to `return` a failure from the current parsing function.
  //  * Continue: recovery succeeded. we are in a valid state to continue, at
  //    the same parsing scope as when this was called (e.g. if we just parsed a
  //    decl with an error, we can now parse another decl. If we just parsed a
  //    member of a decl with an error, we can now parse another member.
  //    A signal to `continue` in the current parsing loop.
  //  * EndOfScope: recovery succeeded, but we are now outside the current
  //    parsing scope. For example, we just parsed a decl with an error, and
  //    recovered, but are now at the end of the file.
  //    A signal to `break` out of the current parsing loop.
  RecoverResult RecoverToEndOfAttribute();
  RecoverResult RecoverToEndOfDecl();
  RecoverResult RecoverToEndOfMember();
  template <Token::Kind ClosingToken>
  RecoverResult RecoverToEndOfListItem();
  RecoverResult RecoverToEndOfAttributeArg();
  RecoverResult RecoverToEndOfParam();
  RecoverResult RecoverToEndOfParamList();

  // Utility function used by RecoverTo* methods
  bool ConsumeTokensUntil(const std::set<Token::Kind>& tokens);

  // Indicates whether we are currently able to continue parsing.
  // Typically when the parser reports an error, it then attempts to recover
  // (get back into a valid state). If this is successful, it updates
  // recovered_errors_ to reflect how many errors are considered "recovered
  // from".
  // Not to be confused with Parser::Success, which is called after parsing to
  // check if any errors were reported during parsing, regardless of recovery.
  bool Ok() const { return checkpoint_.NumNewErrors() == recovered_errors_; }
  void RecoverOneError() { recovered_errors_++; }
  void RecoverAllErrors() { recovered_errors_ = checkpoint_.NumNewErrors(); }
  size_t recovered_errors_ = 0;

  Lexer* lexer_;
  Reporter* reporter_;
  const Reporter::Counts checkpoint_;
  const ExperimentalFlagSet experimental_flags_;

  // The stack of information interesting to the currently active ASTScope objects.
  std::vector<SourceElement> active_ast_scopes_;

  // The token before last_token_ (below).
  Token previous_token_;

  Token last_token_;
  State state_;

  // An ordered list of all tokens (including comments) in the source file.
  std::vector<Token> tokens_;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_PARSER_H_
