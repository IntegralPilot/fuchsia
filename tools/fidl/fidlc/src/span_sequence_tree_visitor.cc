// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/span_sequence_tree_visitor.h"

#include <zircon/assert.h>

#include "tools/fidl/fidlc/src/raw_ast.h"
#include "tools/fidl/fidlc/src/span_sequence.h"
#include "tools/fidl/fidlc/src/tree_visitor.h"

namespace fidlc {

namespace {

// Is the last leaf of the SpanSequence tree with its root at the provided SpanSequence a
// CommentSpanSequence?
bool EndsWithComment(const std::unique_ptr<SpanSequence>& span_sequence) {
  if (span_sequence->IsComposite()) {
    auto as_composite = static_cast<CompositeSpanSequence*>(span_sequence.get());
    if (as_composite->IsEmpty())
      return false;

    const auto& children = as_composite->GetChildren();
    return EndsWithComment(children.back());
  }

  return span_sequence->IsComment();
}

// Alters all spaces between all of the non-comment children of a list of SpanSequences.  This means
// that a trailing space is added to every non-comment child SpanSequence, except the last one.
void SetSpacesBetweenChildren(const std::vector<std::unique_ptr<SpanSequence>>& list, bool spaces) {
  std::optional<size_t> last_non_comment_index;
  const auto last_non_comment_it =
      std::find_if(list.crbegin(), list.crend(),
                   [&](const std::unique_ptr<SpanSequence>& s) { return !s->IsComment(); });
  if (last_non_comment_it != list.rend()) {
    last_non_comment_index = std::distance(last_non_comment_it, list.rend()) - 1;
  }

  for (size_t i = 0; i < list.size(); i++) {
    const auto& child = list[i];
    if (!EndsWithComment(child) && i < last_non_comment_index.value_or(0)) {
      child->SetTrailingSpace(spaces);
    }
  }
}

// Used to ensure that there are no leading blank lines for the SpanSequence tree with its root at
// the provided SpanSequence.  This means recursing down the leftmost branch of the tree, setting
// each "leading_new_lines_" value to 0 as we go.
void ClearLeadingBlankLines(std::unique_ptr<SpanSequence>& span_sequence) {
  if (span_sequence->IsComposite()) {
    // If the first item in the list is a CompositeSpanSequence, its first child's
    // leading_blank_lines_ value will be "hoisted" up to the parent when it's closed.  To ensure
    // that the CompositeSpanSequence retains a zero in this position when that happens, we must
    // set that leading_blank_line_ value to 0 as well.  We need to repeat this process recursively.
    auto as_composite = static_cast<CompositeSpanSequence*>(span_sequence.get());
    if (!as_composite->IsEmpty() && !as_composite->GetChildren()[0]->IsComment()) {
      ClearLeadingBlankLines(as_composite->EditChildren()[0]);
    }
  }

  span_sequence->SetLeadingBlankLines(0);
}

// Consider the following FIDL:
//
//   @foo
//
//   type Foo = ...;
//
// We want to ensure that attribute-carrying declarations like the one above never have a blank line
// between the attribute block and the declaration itself.  To accomplish this goal this function
// checks to see if an attribute block exists for the raw AST node currently being processed.  If it
// does, the first element in the currently open SpanSequence list has its leading_blank_lines
// overwritten to 0.
void ClearBlankLinesAfterAttributeList(const std::unique_ptr<RawAttributeList>& attrs,
                                       SpanSequenceList& list) {
  if (attrs != nullptr && !list.empty()) {
    ClearLeadingBlankLines(list[0]);
  }
}

// This function is called on a token that represents an entire line (if comment_style ==
// kStandalone), or at least the trailing portion of it (if comment_style == kInline), that is a
// comment.  This function ingests up to the end of that line.  The text passed to this function
// must include and start with the `//` character pair that triggered this function call (ie,
// comment lines are ingested with their leading double slashes).
void IngestCommentToken(const Token& comment_token, const Token& prev_token,
                        size_t leading_newlines, AtomicSpanSequence* out) {
  // Figure out where this comment_token line fits into the bigger picture: its either an inline
  // comment, the first line of a standalone comment, or a continuing line of a standalone
  // comment.
  auto line = comment_token.span().data();
  if (leading_newlines == 0 && prev_token.kind() != Token::kComment &&
      prev_token.kind() != Token::kDocComment) {
    // The first part of this line was source code, so the last SpanSequence must be an
    // AtomicSpanSequence.  Add the inline comment to that node.
    std::unique_ptr<InlineCommentSpanSequence> inline_comment =
        std::make_unique<InlineCommentSpanSequence>(comment_token.span().data());
    inline_comment->Close();
    out->AddChild(std::move(inline_comment));
    return;
  }

  auto last_child = out->GetLastChild();
  size_t leading_blank_lines = leading_newlines > 0 ? leading_newlines - 1 : 0;
  if (last_child != nullptr && last_child->GetKind() == SpanSequence::Kind::kStandaloneComment) {
    // There was only a comment on this line, but it is part of a larger, still open comment
    // block.
    auto open_standalone_comment = static_cast<StandaloneCommentSpanSequence*>(last_child);
    open_standalone_comment->AddLine(line, leading_blank_lines);
    return;
  }

  // This line commences a new standalone comment block of one or more lines. That means that the
  // currently open SpanSequence, if one exists, needs to be closed.
  auto standalone_comment = std::make_unique<StandaloneCommentSpanSequence>(leading_blank_lines);
  standalone_comment->AddLine(line);
  out->AddChild(std::move(standalone_comment));
}

void IngestToken(const Token token, const Token prev_token, size_t leading_newlines,
                 AtomicSpanSequence* out) {
  const Token::Kind kind = token.kind();
  if (kind == Token::kComment || kind == Token::kDocComment) {
    IngestCommentToken(token, prev_token, leading_newlines, out);
    return;
  }

  auto token_span_sequence =
      std::make_unique<TokenSpanSequence>(token, leading_newlines > 0 ? leading_newlines - 1 : 0);
  switch (kind) {
    case Token::kEndOfFile:
      return;
    case Token::kArrow:
    case Token::kComma:
    case Token::kEqual:
    case Token::kPipe: {
      token_span_sequence->SetTrailingSpace(true);
      break;
    }
    case Token::kIdentifier: {
      token_span_sequence->SetTrailingSpace(true);
      break;
    }
    default:
      break;
  }

  token_span_sequence->Close();
  out->AddChild(std::move(token_span_sequence));
}

}  // namespace

std::optional<std::unique_ptr<SpanSequence>> SpanSequenceTreeVisitor::IngestUpTo(
    const std::optional<Token> until, SpanSequence::Position position) {
  auto atomic = std::make_unique<AtomicSpanSequence>(position);
  while (next_token_index_ < tokens_.size()) {
    const Token& token = tokens_[next_token_index_];
    if (until.has_value() && token.ptr() >= until.value().ptr()) {
      break;
    }

    ZX_ASSERT(next_token_index_ > 0);
    const Token& prev_token = tokens_[next_token_index_ - 1];
    IngestToken(token, prev_token, token.leading_newlines(), atomic.get());
    next_token_index_ += 1;
  }

  if (atomic->IsEmpty()) {
    return std::nullopt;
  }
  return std::move(atomic);
}

std::optional<std::unique_ptr<SpanSequence>> SpanSequenceTreeVisitor::IngestUpToAndIncluding(
    const std::optional<Token> until, SpanSequence::Position position) {
  auto atomic = std::make_unique<AtomicSpanSequence>(position);
  while (next_token_index_ < tokens_.size()) {
    const Token& token = tokens_[next_token_index_];
    if (until.has_value() && until.value().ptr() < token.ptr()) {
      break;
    }

    ZX_ASSERT(next_token_index_ > 0);
    const Token& prev_token = tokens_[next_token_index_ - 1];
    IngestToken(token, prev_token, token.leading_newlines(), atomic.get());
    next_token_index_ += 1;

    if (until.has_value() && token.ptr() == until.value().ptr()) {
      break;
    }
  }

  if (atomic->IsEmpty()) {
    return std::nullopt;
  }
  return std::move(atomic);
}

std::optional<std::unique_ptr<SpanSequence>>
SpanSequenceTreeVisitor::IngestUpToAndIncludingTokenKind(
    const std::optional<Token::Kind> until_kind, SpanSequence::Position position) {
  auto atomic = std::make_unique<AtomicSpanSequence>(position);
  bool found = false;
  while (next_token_index_ < tokens_.size()) {
    const Token& token = tokens_[next_token_index_];
    ZX_ASSERT(next_token_index_ > 0);
    const Token& prev_token = tokens_[next_token_index_ - 1];

    // If we have found the token kind we're looking for, make sure to capture any trailing inline
    // comments!
    if (found && (token.leading_newlines() > 0 ||
                  (token.kind() != Token::kComment || token.kind() != Token::kComment))) {
      break;
    }
    IngestToken(token, prev_token, token.leading_newlines(), atomic.get());

    next_token_index_ += 1;
    if (token.kind() == until_kind) {
      found = true;
    }
  }

  if (atomic->IsEmpty()) {
    return std::nullopt;
  }
  return std::move(atomic);
}

std::optional<std::unique_ptr<SpanSequence>> SpanSequenceTreeVisitor::IngestRestOfFile() {
  return IngestUpToAndIncluding(std::nullopt);
}

std::optional<std::unique_ptr<SpanSequence>>
SpanSequenceTreeVisitor::IngestUpToAndIncludingSemicolon() {
  return IngestUpToAndIncludingTokenKind(Token::kSemicolon);
}

bool SpanSequenceTreeVisitor::IsInsideOf(VisitorKind visitor_kind) {
  return std::find(ast_path_.begin(), ast_path_.end(), visitor_kind) != ast_path_.end();
}

bool SpanSequenceTreeVisitor::IsDirectlyInsideOf(VisitorKind visitor_kind) {
  return !ast_path_.empty() && ast_path_[ast_path_.size() - 1] == visitor_kind;
}

SpanSequenceTreeVisitor::Visiting::Visiting(SpanSequenceTreeVisitor* ftv, VisitorKind visitor_kind)
    : ftv_(ftv) {
  this->ftv_->ast_path_.push_back(visitor_kind);
}

SpanSequenceTreeVisitor::Visiting::~Visiting() { this->ftv_->ast_path_.pop_back(); }

template <typename T>
SpanSequenceTreeVisitor::Builder<T>::Builder(SpanSequenceTreeVisitor* ftv, const Token& start,
                                             const Token& end, bool new_list)
    : ftv_(ftv), start_(start), end_(end) {
  if (new_list)
    this->GetFormattingTreeVisitor()->building_.push(SpanSequenceList());

  auto prelude = ftv_->IngestUpTo(start_);
  if (prelude.has_value())
    ftv_->building_.top().push_back(std::move(prelude.value()));
}

SpanSequenceTreeVisitor::TokenBuilder::TokenBuilder(SpanSequenceTreeVisitor* ftv,
                                                    const Token& token, bool has_trailing_space)
    : Builder<TokenSpanSequence>(ftv, token, token, false) {
  auto token_span_sequence = std::make_unique<TokenSpanSequence>(
      token, token.leading_newlines() == 0 ? 0 : token.leading_newlines() - 1);
  token_span_sequence->SetTrailingSpace(has_trailing_space);
  token_span_sequence->Close();

  this->GetFormattingTreeVisitor()->building_.top().push_back(std::move(token_span_sequence));
  this->GetFormattingTreeVisitor()->next_token_index_ += 1;
}

template <typename T>
SpanSequenceTreeVisitor::SpanBuilder<T>::~SpanBuilder<T>() {
  // Ingest any remaining text between the last processed child and the end token of the span.  This
  // text may not retain any leading blank lines or trailing spaces.
  const Token end = this->GetEndToken();
  auto postscript = this->GetFormattingTreeVisitor()->IngestUpToAndIncluding(
      end, SpanSequence::Position::kNewlineUnindented);
  if (postscript.has_value()) {
    const auto& top = this->GetFormattingTreeVisitor()->building_.top();
    if (top.empty() || (!top.empty() && !EndsWithComment(top.back()))) {
      ClearLeadingBlankLines(postscript.value());
    }

    postscript.value()->SetTrailingSpace(false);
    this->GetFormattingTreeVisitor()->building_.top().push_back(std::move(postscript.value()));
  }

  auto parts = std::move(this->GetFormattingTreeVisitor()->building_.top());
  auto composite_span_sequence = std::make_unique<T>(std::move(parts), this->position_);
  composite_span_sequence->CloseChildren();

  this->GetFormattingTreeVisitor()->building_.pop();
  this->GetFormattingTreeVisitor()->building_.top().push_back(std::move(composite_span_sequence));
}

template <typename T>
SpanSequenceTreeVisitor::StatementBuilder<T>::~StatementBuilder<T>() {
  auto parts = std::move(this->GetFormattingTreeVisitor()->building_.top());
  auto composite_span_sequence = std::make_unique<T>(std::move(parts), this->position_);
  auto semicolon_span_sequence =
      this->GetFormattingTreeVisitor()->IngestUpToAndIncludingSemicolon().value();

  // Append the semicolon_span_sequence to the last child in the composite_span_sequence, if it
  // exists.
  auto last_child = composite_span_sequence->GetLastChild();
  if (last_child != nullptr && !last_child->IsClosed()) {
    ZX_ASSERT_MSG(last_child->IsComposite(),
                  "cannot append semicolon to non-composite SpanSequence");
    auto last_child_as_composite = static_cast<CompositeSpanSequence*>(last_child);
    last_child_as_composite->AddChild(std::move(semicolon_span_sequence));
  }
  composite_span_sequence->CloseChildren();

  this->GetFormattingTreeVisitor()->building_.pop();
  this->GetFormattingTreeVisitor()->building_.top().push_back(std::move(composite_span_sequence));
}

void SpanSequenceTreeVisitor::OnAliasDeclaration(
    const std::unique_ptr<RawAliasDeclaration>& element) {
  const auto visiting = Visiting(this, VisitorKind::kAliasDeclaration);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }

  const auto builder = StatementBuilder<DivisibleSpanSequence>(
      this, *element, SpanSequence::Position::kNewlineUnindented);
  TreeVisitor::OnAliasDeclaration(element);
  SetSpacesBetweenChildren(building_.top(), true);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

void SpanSequenceTreeVisitor::OnAttributeArg(const std::unique_ptr<RawAttributeArg>& element) {
  const auto visiting = Visiting(this, VisitorKind::kAttributeArg);
  const auto builder = SpanBuilder<AtomicSpanSequence>(this, *element);

  if (element->maybe_name != nullptr) {
    OnIdentifier(element->maybe_name);
    // IngestToken() puts a trailing space after "=" tokens because that's
    // usually what we want, but for attribute arguments we don't want it.
    auto postscript = IngestUpToAndIncludingTokenKind(Token::Kind::kEqual);
    if (postscript.has_value()) {
      building_.top().push_back(std::move(postscript.value()));
      auto as_atomic = static_cast<AtomicSpanSequence*>(building_.top().back().get());
      as_atomic->GetLastChild()->SetTrailingSpace(false);
    }
  }
  OnConstant(element->value);
}

void SpanSequenceTreeVisitor::OnAttribute(const std::unique_ptr<RawAttribute>& element) {
  const auto visiting = Visiting(this, VisitorKind::kAttribute);

  // Special case: this attribute is actually a doc comment.  Treat it like any other comment type,
  // and ingest until the last newline in the doc comment.
  if (element->provenance == RawAttribute::Provenance::kDocComment) {
    auto doc_comment = IngestUpToAndIncluding(element->end_token);
    if (doc_comment.has_value())
      building_.top().push_back(std::move(doc_comment.value()));
    return;
  }

  // Special case: attribute with no arguments.  Just make a TokenSpanSequence out of the @ string
  // and exit.
  if (element->args.empty()) {
    const auto builder =
        SpanBuilder<AtomicSpanSequence>(this, *element, SpanSequence::Position::kNewlineUnindented);
    const auto token_builder = TokenBuilder(this, element->start_token, false);
    return;
  }

  // This attribute has at least one argument.  For each argument, first ingest the prelude (usually
  // the preceding comment), but add it as a suffix to the previous attribute instead of as a prefix
  // to the current one.  If we did not do this, we'd end up with formatting like:
  //
  //   @foo
  //           ("my very very ... very long arg 1"
  //           , "my very very ... very long arg 2")
  const auto builder = SpanBuilder<DivisibleSpanSequence>(
      this, element->args[0]->start_token, SpanSequence::Position::kNewlineUnindented);
  auto as_atomic = static_cast<AtomicSpanSequence*>(building_.top().front().get());
  SetSpacesBetweenChildren(as_atomic->EditChildren(), false);

  std::optional<SpanBuilder<AtomicSpanSequence>> arg_builder;
  for (const auto& arg : element->args) {
    auto postscript = IngestUpTo(arg->start_token);
    if (postscript.has_value())
      building_.top().push_back(std::move(postscript.value()));

    arg_builder.emplace(this, *arg);
    OnAttributeArg(arg);
  }

  // Make sure to delete the last argument, so that its destructor is called and it is properly
  // added to the "building_" stack.
  arg_builder.reset();

  // Ingest the closing ")" token, and append it to the final argument.
  auto postscript = IngestUpToAndIncluding(element->end_token);
  if (postscript.has_value()) {
    postscript.value()->SetTrailingSpace(true);
    auto last_argument_span_sequence =
        static_cast<AtomicSpanSequence*>(building_.top().back().get());
    last_argument_span_sequence->AddChild(std::move(postscript.value()));
  }

  // At this point, we should have a set of atomic span sequences with children like:
  //
  //   «@foo(»«"arg1",»«"arg2"»,«"..."»,«"argN")»
  //
  // We want to make sure there is a space between each of these child elements, except for the
  // first to, to produce an output like:
  //
  //   @foo("arg1", "arg2", "...", "argN")
  //
  // To accomplish this, we simply add the trailing spaces to every non-comment element except the
  // last, then remove the trailing space from the first element.
  SetSpacesBetweenChildren(building_.top(), true);
  building_.top()[0]->SetTrailingSpace(false);
}

void SpanSequenceTreeVisitor::OnAttributeList(const std::unique_ptr<RawAttributeList>& element) {
  if (already_seen_.insert(element.get()).second) {
    // Special case: attributes on anonymous layouts do not go on newlines.  Instead, they are put
    // into a DivisibleSpanSequence and kept on the same line if possible.
    if (IsDirectlyInsideOf(VisitorKind::kTypeConstructor)) {
      const auto visiting = Visiting(this, VisitorKind::kAttributeList);
      const auto builder = SpanBuilder<DivisibleSpanSequence>(this, *element);
      TreeVisitor::OnAttributeList(element);
      return;
    }

    const auto visiting = Visiting(this, VisitorKind::kAttributeList);
    const auto indent = IsInsideOf(VisitorKind::kLayoutMember) ||
                                IsInsideOf(VisitorKind::kProtocolMethod) ||
                                IsInsideOf(VisitorKind::kProtocolCompose) ||
                                IsInsideOf(VisitorKind::kServiceMember) ||
                                IsInsideOf(VisitorKind::kResourceProperty)
                            ? SpanSequence::Position::kNewlineIndented
                            : SpanSequence::Position::kNewlineUnindented;
    const auto builder = SpanBuilder<MultilineSpanSequence>(this, *element, indent);
    TreeVisitor::OnAttributeList(element);

    // Remove all blank lines between attributes.
    auto& attr_span_sequences = building_.top();
    for (size_t i = 1; i < attr_span_sequences.size(); ++i) {
      auto& child_span_sequence = attr_span_sequences[i];
      if (!child_span_sequence->IsComment()) {
        ClearLeadingBlankLines(child_span_sequence);
      }
    }
  }
}

void SpanSequenceTreeVisitor::OnModifier(const std::unique_ptr<RawModifier>& element) {
  const auto visiting = Visiting(this, VisitorKind::kModifier);
  TreeVisitor::OnModifier(element);
}

void SpanSequenceTreeVisitor::OnModifierList(const std::unique_ptr<RawModifierList>& element) {
  if (already_seen_.insert(element.get()).second) {
    const auto visiting = Visiting(this, VisitorKind::kModifierList);
    TreeVisitor::OnModifierList(element);
  }
}

void SpanSequenceTreeVisitor::OnBinaryOperatorConstant(
    const std::unique_ptr<RawBinaryOperatorConstant>& element) {
  // We need a separate scope, so that each operand receives a different visitor kind.  This is
  // important because OnLiteral visitor behaves different for the last constant in the chain: it
  // requires trailing spaces on all constants except the last.
  {
    const auto visiting = Visiting(this, VisitorKind::kBinaryOperatorFirstConstant);
    const auto operand_builder = SpanBuilder<AtomicSpanSequence>(this, *element->left_operand);
    TreeVisitor::OnConstant(element->left_operand);
  }

  {
    const auto visiting = Visiting(this, VisitorKind::kBinaryOperatorSecondConstant);
    const auto operand_builder = SpanBuilder<AtomicSpanSequence>(this, *element->right_operand);
    TreeVisitor::OnConstant(element->right_operand);
  }
  SetSpacesBetweenChildren(building_.top(), true);
}

void SpanSequenceTreeVisitor::OnCompoundIdentifier(
    const std::unique_ptr<RawCompoundIdentifier>& element) {
  const auto visiting = Visiting(this, VisitorKind::kCompoundIdentifier);
  const auto builder = SpanBuilder<AtomicSpanSequence>(this, *element);
  TreeVisitor::OnCompoundIdentifier(element);
}

void SpanSequenceTreeVisitor::OnConstant(const std::unique_ptr<RawConstant>& element) {
  const auto visiting = Visiting(this, VisitorKind::kConstant);
  const auto span_builder = SpanBuilder<AtomicSpanSequence>(this, *element);
  TreeVisitor::OnConstant(element);
}

void SpanSequenceTreeVisitor::OnConstDeclaration(
    const std::unique_ptr<RawConstDeclaration>& element) {
  const auto visiting = Visiting(this, VisitorKind::kConstDeclaration);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }

  const auto builder = StatementBuilder<DivisibleSpanSequence>(
      this, *element, SpanSequence::Position::kNewlineUnindented);

  // We need a separate scope for these two nodes, as they are meant to be their own
  // DivisibleSpanSequence, but no raw AST node or visitor exists for grouping them.
  {
    const auto lhs_builder = SpanBuilder<DivisibleSpanSequence>(this, element->start_token);

    // Keep the "const" keyword atomic with the name of the declaration.
    {
      const auto name_builder = SpanBuilder<AtomicSpanSequence>(this, *element->identifier);
      OnIdentifier(element->identifier);
    }

    // Similarly, keep the type constructor atomic as well.
    {
      const auto type_ctor_new_builder = SpanBuilder<AtomicSpanSequence>(this, *element->type_ctor);
      OnTypeConstructor(element->type_ctor);
    }
    SetSpacesBetweenChildren(building_.top(), true);
  }

  OnConstant(element->constant);
  SetSpacesBetweenChildren(building_.top(), true);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

void SpanSequenceTreeVisitor::OnFile(const std::unique_ptr<File>& element) {
  const auto visiting = Visiting(this, VisitorKind::kFile);
  building_.emplace();

  // Handle the first token, which is always |kStartOfFile|. This also has the nice benefit of not
  // having to special-case dealing with the first token later in the visitor.
  next_token_index_++;

  DeclarationOrderTreeVisitor::OnFile(element);

  auto footer = IngestRestOfFile();
  if (footer.has_value())
    building_.top().push_back(std::move(footer.value()));
}

void SpanSequenceTreeVisitor::OnIdentifier(const std::unique_ptr<RawIdentifier>& element,
                                           bool ignore) {
  if (already_seen_.insert(element.get()).second && !ignore) {
    const auto visiting = Visiting(this, VisitorKind::kIdentifier);
    if (IsInsideOf(VisitorKind::kCompoundIdentifier)) {
      const auto builder = TokenBuilder(this, element->start_token, false);
      TreeVisitor::OnIdentifier(element);
    } else {
      const auto span_builder = SpanBuilder<AtomicSpanSequence>(this, *element);
      const auto token_builder = TokenBuilder(this, element->start_token, false);
      TreeVisitor::OnIdentifier(element);
    }
  }
}

void SpanSequenceTreeVisitor::OnLiteral(const std::unique_ptr<RawLiteral>& element) {
  const auto visiting = Visiting(this, VisitorKind::kLiteral);
  const auto trailing_space = IsInsideOf(VisitorKind::kBinaryOperatorFirstConstant);
  const auto builder = TokenBuilder(this, element->start_token, trailing_space);
  TreeVisitor::OnLiteral(element);
}

void SpanSequenceTreeVisitor::OnIdentifierConstant(
    const std::unique_ptr<RawIdentifierConstant>& element) {
  const auto visiting = Visiting(this, VisitorKind::kIdentifierConstant);
  TreeVisitor::OnIdentifierConstant(element);
}

void SpanSequenceTreeVisitor::OnInlineLayoutReference(
    const std::unique_ptr<RawInlineLayoutReference>& element) {
  const auto visiting = Visiting(this, VisitorKind::kInlineLayoutReference);

  // We deliberately ignore the AttributeList, as it has been handled by the OnTypeConstructor call
  // that called into this function.
  TreeVisitor::OnInlineLayoutReference(element);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

void SpanSequenceTreeVisitor::OnLayout(const std::unique_ptr<RawLayout>& element) {
  const auto visiting = Visiting(this, VisitorKind::kLayout);

  VisitorKind inner_kind;
  switch (element->kind) {
    case RawLayout::Kind::kBits:
    case RawLayout::Kind::kEnum: {
      inner_kind = VisitorKind::kValueLayout;
      break;
    }
    case RawLayout::Kind::kStruct: {
      inner_kind = VisitorKind::kStructLayout;
      break;
    }
    case RawLayout::Kind::kTable:
    case RawLayout::Kind::kOverlay:
    case RawLayout::Kind::kUnion: {
      inner_kind = VisitorKind::kOrdinaledLayout;
      break;
    }
  }
  const auto inner_visiting = Visiting(this, inner_kind);

  if (element->modifiers != nullptr) {
    OnModifierList(element->modifiers);
  }

  // Special case: an empty layout (ex: `struct {}`) should always be atomic.
  if (element->members.empty()) {
    if (element->subtype_ctor) {
      const auto subtype_builder =
          SpanBuilder<AtomicSpanSequence>(this, element->subtype_ctor->start_token);
      auto postscript = IngestUpToAndIncludingTokenKind(Token::Kind::kRightCurly);
      if (postscript.has_value())
        building_.top().push_back(std::move(postscript.value()));

      // By default, `:` tokens do not have a space following the token.  However, in the case of
      // sub-typed bits/enum like `handle : uint32 {...`, we need to add this space in. We can do
      // this by adding spaces between every child of the first element of the SpanSequence
      // currently being built.
      SetSpacesBetweenChildren(building_.top(), true);
    } else {
      const auto builder = SpanBuilder<AtomicSpanSequence>(this, *element);
    }
    return;
  }

  const auto builder = SpanBuilder<MultilineSpanSequence>(this, element->members[0]->start_token,
                                                          element->end_token);

  // By default, `:` tokens do not have a space following the token.  However, in the case of
  // sub-typed layouts like `enum : uint32 {...`, we need to add this space in.  We can do this by
  // adding spaces between every child of the first element of the MultilineSpanSequence currently
  // being built.
  auto as_composite = static_cast<CompositeSpanSequence*>(building_.top().front().get());
  SetSpacesBetweenChildren(as_composite->EditChildren(), true);

  TreeVisitor::OnLayout(element);
}

void SpanSequenceTreeVisitor::OnLayoutMember(const std::unique_ptr<RawLayoutMember>& element) {
  const auto visiting = Visiting(this, VisitorKind::kLayoutMember);
  TreeVisitor::OnLayoutMember(element);
}

void SpanSequenceTreeVisitor::OnLibraryDeclaration(
    const std::unique_ptr<RawLibraryDeclaration>& element) {
  const auto visiting = Visiting(this, VisitorKind::kLibraryDeclaration);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }

  const auto builder = StatementBuilder<AtomicSpanSequence>(
      this, *element, SpanSequence::Position::kNewlineUnindented);
  TreeVisitor::OnLibraryDeclaration(element);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

void SpanSequenceTreeVisitor::OnLiteralConstant(
    const std::unique_ptr<RawLiteralConstant>& element) {
  const auto visiting = Visiting(this, VisitorKind::kLiteralConstant);
  TreeVisitor::OnLiteralConstant(element);
}

void SpanSequenceTreeVisitor::OnNamedLayoutReference(
    const std::unique_ptr<RawNamedLayoutReference>& element) {
  const auto visiting = Visiting(this, VisitorKind::kNamedLayoutReference);
  const auto builder = SpanBuilder<AtomicSpanSequence>(this, *element);
  TreeVisitor::OnNamedLayoutReference(element);
}

void SpanSequenceTreeVisitor::OnOrdinal64(RawOrdinal64& element) {
  const auto visiting = Visiting(this, VisitorKind::kOrdinal64);
  const auto span_builder = SpanBuilder<AtomicSpanSequence>(this, element);
  const auto token_builder = TokenBuilder(this, element.start_token, false);
}

void SpanSequenceTreeVisitor::OnOrdinaledLayoutMember(
    const std::unique_ptr<RawOrdinaledLayoutMember>& element) {
  const auto visiting = Visiting(this, VisitorKind::kOrdinaledLayoutMember);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }

  {
    const auto builder = StatementBuilder<DivisibleSpanSequence>(
        this, *element, SpanSequence::Position::kNewlineIndented);

    // We want to keep the ordinal atomic with the member name, so we need a separate scope for
    // these two nodes, as they are meant to be their own AtomicSpanSequence, but no raw AST node or
    // visitor exists for grouping them.
    {
      const auto ordinal_name_builder = SpanBuilder<AtomicSpanSequence>(this, element->start_token);
      OnOrdinal64(*element->ordinal);
      building_.top().back()->SetTrailingSpace(true);
      OnIdentifier(element->identifier);
    }

    OnTypeConstructor(element->type_ctor);
    SetSpacesBetweenChildren(building_.top(), true);
    ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
  }
}

void SpanSequenceTreeVisitor::OnParameterList(const std::unique_ptr<RawParameterList>& element) {
  const auto visiting = Visiting(this, VisitorKind::kParameterList);

  const auto builder = SpanBuilder<AtomicSpanSequence>(this, *element);
  if (element->type_ctor) {
    auto opening_paren = IngestUpTo(element->type_ctor->start_token);
    if (opening_paren.has_value())
      building_.top().push_back(std::move(opening_paren.value()));
  }

  TreeVisitor::OnParameterList(element);
}

void SpanSequenceTreeVisitor::OnProtocolCompose(
    const std::unique_ptr<RawProtocolCompose>& element) {
  const auto visiting = Visiting(this, VisitorKind::kProtocolCompose);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }

  const auto builder = StatementBuilder<AtomicSpanSequence>(
      this, *element, SpanSequence::Position::kNewlineIndented);
  TreeVisitor::OnProtocolCompose(element);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

void SpanSequenceTreeVisitor::OnProtocolDeclaration(
    const std::unique_ptr<RawProtocolDeclaration>& element) {
  const auto visiting = Visiting(this, VisitorKind::kProtocolDeclaration);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }

  const auto first_modifier_or_identifier_token =
      element->modifiers != nullptr ? element->modifiers->modifiers.front()->start_token
                                    : element->identifier->start_token;

  // Special case: an empty protocol definition should always be atomic.
  if (element->methods.empty() && element->composed_protocols.empty()) {
    const auto builder = StatementBuilder<AtomicSpanSequence>(
        this, first_modifier_or_identifier_token, SpanSequence::Position::kNewlineUnindented);
    if (element->modifiers != nullptr) {
      OnModifierList(element->modifiers);
    }
    auto postscript = IngestUpTo(element->end_token);
    if (postscript.has_value())
      building_.top().push_back(std::move(postscript.value()));
    ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
    return;
  }

  Token first_child_start_token;
  if (!element->composed_protocols.empty() && !element->methods.empty()) {
    // If the protocol has both methods and compositions, compare the addresses of the first
    // character of the first element of each to determine which is the first child start token.
    if (element->composed_protocols[0]->start_token.ptr() <
        element->methods[0]->start_token.ptr()) {
      first_child_start_token = element->composed_protocols[0]->start_token;
    } else {
      first_child_start_token = element->methods[0]->start_token;
    }
  } else if (element->composed_protocols.empty()) {
    // No compositions - the first token of the first method element is the first child start token.
    first_child_start_token = element->methods[0]->start_token;
  } else {
    // No methods - the first token of the first compose element is the first child start token.
    first_child_start_token = element->composed_protocols[0]->start_token;
  }

  // The following logic is a bit complicated, arrived at by trial and error.
  // We have to use StatementBuilder<MultilineSpanSequence> to start the
  // declaration on a new line. If there are modifiers, we have to use
  // SpanBuilder<AtomicSpanSequence> to put them on the same line as "protocol".
  // If there are no modifiers, we must *not* use a SpanBuilder: if we do, there
  // will be no space between "protocol" and the protocol name.
  const auto multiline_start =
      element->modifiers != nullptr ? first_modifier_or_identifier_token : first_child_start_token;
  const auto builder = StatementBuilder<MultilineSpanSequence>(
      this, multiline_start, SpanSequence::Position::kNewlineUnindented);
  if (element->modifiers != nullptr) {
    const auto first_line_builder =
        SpanBuilder<AtomicSpanSequence>(this, first_modifier_or_identifier_token);
    OnModifierList(element->modifiers);
    auto postscript = IngestUpTo(first_child_start_token);
    if (postscript.has_value())
      building_.top().push_back(std::move(postscript.value()));
  }

  // We want to purposefully ignore this identifier, as it has already been captured by the prelude
  // to the StatementBuilder we created above.  By running this method now, we mark the Identifier
  // as seen, so that the call to DeclarationOrderTreeVisitor::OnProtocolDeclaration won't print the
  // identifier a second time when it visits it.
  OnIdentifier(element->identifier, true);
  DeclarationOrderTreeVisitor::OnProtocolDeclaration(element);

  const auto closing_bracket_builder = SpanBuilder<AtomicSpanSequence>(
      this, element->end_token, SpanSequence::Position::kNewlineUnindented);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

void SpanSequenceTreeVisitor::OnProtocolMethod(const std::unique_ptr<RawProtocolMethod>& element) {
  const auto visiting = Visiting(this, VisitorKind::kProtocolMethod);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }
  const auto builder = StatementBuilder<AtomicSpanSequence>(
      this, element->start_token, SpanSequence::Position::kNewlineIndented);
  if (element->modifiers != nullptr) {
    OnModifierList(element->modifiers);
  }

  if (element->maybe_request != nullptr) {
    const auto visiting_request = Visiting(this, VisitorKind::kProtocolRequest);
    // This is not an event - make sure to process the identifier into an AtomicSpanSequence with
    // the first parameter list, with no space between them.
    const auto name_builder = SpanBuilder<AtomicSpanSequence>(
        this, element->identifier->start_token, element->maybe_request->end_token);
    OnIdentifier(element->identifier);
    OnParameterList(element->maybe_request);
  }

  if (element->maybe_response != nullptr) {
    const auto visiting_response = Visiting(this, VisitorKind::kProtocolResponse);
    if (element->maybe_request == nullptr) {
      // This is an event - make sure to process the identifier into an AtomicSpanSequence with the
      // the second parameter list, with no space between them.
      const auto name_builder = SpanBuilder<AtomicSpanSequence>(
          this, element->identifier->start_token, element->maybe_response->end_token);
      OnIdentifier(element->identifier);
      OnParameterList(element->maybe_response);
    } else {
      // This is a method with both a request and a response.  Reaching this point means that the
      // last character we've seen is the closing `)` of the request parameter list, so make sure to
      // add a space after that character before processing the `->` and the response parameter
      // list.
      building_.top().back()->SetTrailingSpace(true);
      OnParameterList(element->maybe_response);
    }
  }

  if (element->maybe_error_ctor != nullptr) {
    building_.top().back()->SetTrailingSpace(true);
    OnTypeConstructor(element->maybe_error_ctor);
  }
  SetSpacesBetweenChildren(building_.top(), true);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

void SpanSequenceTreeVisitor::OnResourceDeclaration(
    const std::unique_ptr<RawResourceDeclaration>& element) {
  const auto visiting = Visiting(this, VisitorKind::kResourceDeclaration);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }

  const auto builder = StatementBuilder<MultilineSpanSequence>(
      this, element->start_token, SpanSequence::Position::kNewlineUnindented);

  // Build the opening "resource_definition ..." line.
  {
    const auto first_line_builder =
        SpanBuilder<AtomicSpanSequence>(this, element->identifier->start_token);
    OnIdentifier(element->identifier);
    if (element->maybe_type_ctor != nullptr) {
      const auto subtype_builder =
          SpanBuilder<AtomicSpanSequence>(this, element->maybe_type_ctor->start_token);
      auto postscript = IngestUpToAndIncludingTokenKind(Token::Kind::kLeftCurly);
      if (postscript.has_value())
        building_.top().push_back(std::move(postscript.value()));

      // By default, `:` tokens do not have a space following the token.  However, in the case of
      // sub-typed resource definitions like `handle : uint32 {...`, we need to add this space in.
      // We can do this by adding spaces between every child of the first element of the
      // SpanSequence currently being built.
      SetSpacesBetweenChildren(building_.top(), true);
    } else {
      auto postscript = IngestUpToAndIncludingTokenKind(Token::kLeftCurly);
      if (postscript.has_value())
        building_.top().push_back(std::move(postscript.value()));
    }
    SetSpacesBetweenChildren(building_.top(), true);
  }

  // Build the indented "property { ... }" portion.
  {
    const auto properties_builder = SpanBuilder<MultilineSpanSequence>(
        this, element->properties.front()->start_token, SpanSequence::Position::kNewlineIndented);
    TreeVisitor::OnResourceDeclaration(element);

    const auto closing_bracket_builder = SpanBuilder<AtomicSpanSequence>(
        this, element->properties.back()->end_token, SpanSequence::Position::kNewlineUnindented);
    auto closing_bracket = IngestUpToAndIncludingSemicolon();
    if (closing_bracket.has_value())
      building_.top().push_back(std::move(closing_bracket.value()));
  }

  const auto closing_bracket_builder = SpanBuilder<AtomicSpanSequence>(
      this, element->end_token, SpanSequence::Position::kNewlineUnindented);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

void SpanSequenceTreeVisitor::OnResourceProperty(
    const std::unique_ptr<RawResourceProperty>& element) {
  const auto visiting = Visiting(this, VisitorKind::kResourceProperty);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }

  const auto builder = StatementBuilder<AtomicSpanSequence>(
      this, *element, SpanSequence::Position::kNewlineIndented);

  TreeVisitor::OnResourceProperty(element);
  SetSpacesBetweenChildren(building_.top(), true);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

void SpanSequenceTreeVisitor::OnServiceDeclaration(
    const std::unique_ptr<RawServiceDeclaration>& element) {
  const auto visiting = Visiting(this, VisitorKind::kServiceDeclaration);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }

  // Special case: an empty service definition should always be atomic.
  if (element->members.empty()) {
    const auto builder = StatementBuilder<AtomicSpanSequence>(
        this, element->identifier->start_token, element->end_token,
        SpanSequence::Position::kNewlineUnindented);
    ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
    return;
  }

  const auto builder = StatementBuilder<MultilineSpanSequence>(
      this, element->members.front()->start_token, SpanSequence::Position::kNewlineUnindented);

  // We want to purposefully ignore this identifier, as it has already been captured by the prelude
  // to the StatementBuilder we created above.  By running this method now, we mark the Identifier
  // as seen, so that the call to TreeVisitor::OnServiceDeclaration won't print the identifier a
  // second time when it visits it.
  OnIdentifier(element->identifier, true);
  TreeVisitor::OnServiceDeclaration(element);

  const auto closing_bracket_builder = SpanBuilder<AtomicSpanSequence>(
      this, element->end_token, SpanSequence::Position::kNewlineUnindented);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

void SpanSequenceTreeVisitor::OnServiceMember(const std::unique_ptr<RawServiceMember>& element) {
  const auto visiting = Visiting(this, VisitorKind::kServiceMember);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }

  const auto builder = StatementBuilder<AtomicSpanSequence>(
      this, *element, SpanSequence::Position::kNewlineIndented);

  TreeVisitor::OnServiceMember(element);
  SetSpacesBetweenChildren(building_.top(), true);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

void SpanSequenceTreeVisitor::OnStructLayoutMember(
    const std::unique_ptr<RawStructLayoutMember>& element) {
  const auto visiting = Visiting(this, VisitorKind::kStructLayoutMember);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }

  const auto builder = StatementBuilder<DivisibleSpanSequence>(
      this, *element, SpanSequence::Position::kNewlineIndented);
  TreeVisitor::OnStructLayoutMember(element);
  SetSpacesBetweenChildren(building_.top(), true);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

void SpanSequenceTreeVisitor::OnTypeConstructor(
    const std::unique_ptr<RawTypeConstructor>& element) {
  // Special case: make sure not to visit the subtype on a bits/enum declaration twice, since it is
  // already being processed as part of the prelude to the layout.
  if (IsInsideOf(VisitorKind::kValueLayout) || (IsInsideOf(VisitorKind::kResourceDeclaration) &&
                                                !IsInsideOf(VisitorKind::kResourceProperty))) {
    return;
  }

  const auto visiting = Visiting(this, VisitorKind::kTypeConstructor);
  if (element->layout_ref->kind == RawLayoutReference::Kind::kInline) {
    // Check if we have attributes - if we do, we'll need to process them before creating the
    // TypeConstructor-containing AtomicSpanSequence, because inline layout attributes should be
    // stacked when wrapped.
    auto as_inline_layout_ref = static_cast<RawInlineLayoutReference*>(element->layout_ref.get());
    if (as_inline_layout_ref->attributes != nullptr) {
      OnAttributeList(as_inline_layout_ref->attributes);
      const auto builder = SpanBuilder<AtomicSpanSequence>(this, *element);
      TreeVisitor::OnTypeConstructor(element);
      return;
    }
  }

  const auto builder = SpanBuilder<AtomicSpanSequence>(this, *element);
  TreeVisitor::OnTypeConstructor(element);
}

void SpanSequenceTreeVisitor::OnTypeDeclaration(
    const std::unique_ptr<RawTypeDeclaration>& element) {
  const auto visiting = Visiting(this, VisitorKind::kTypeDeclaration);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }

  const auto builder = StatementBuilder<DivisibleSpanSequence>(
      this, *element, SpanSequence::Position::kNewlineUnindented);
  TreeVisitor::OnTypeDeclaration(element);
  SetSpacesBetweenChildren(building_.top(), true);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

void SpanSequenceTreeVisitor::OnUsing(const std::unique_ptr<RawUsing>& element) {
  const auto visiting = Visiting(this, VisitorKind::kUsing);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }

  const auto builder = StatementBuilder<DivisibleSpanSequence>(
      this, *element, SpanSequence::Position::kNewlineUnindented);
  TreeVisitor::OnUsing(element);
  SetSpacesBetweenChildren(building_.top(), true);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

void SpanSequenceTreeVisitor::OnValueLayoutMember(
    const std::unique_ptr<RawValueLayoutMember>& element) {
  const auto visiting = Visiting(this, VisitorKind::kValueLayoutMember);
  if (element->attributes != nullptr) {
    OnAttributeList(element->attributes);
  }

  const auto builder = StatementBuilder<DivisibleSpanSequence>(
      this, *element, SpanSequence::Position::kNewlineIndented);
  TreeVisitor::OnValueLayoutMember(element);
  SetSpacesBetweenChildren(building_.top(), true);
  ClearBlankLinesAfterAttributeList(element->attributes, building_.top());
}

MultilineSpanSequence SpanSequenceTreeVisitor::Result() {
  ZX_ASSERT_MSG(!building_.empty(), "Result() must be called exactly once after OnFile()");
  auto result = MultilineSpanSequence(std::move(building_.top()));
  result.Close();
  building_.pop();
  return result;
}

}  // namespace fidlc
