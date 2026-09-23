// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_TOKEN_H_
#define TOOLS_FIDL_FIDLC_SRC_TOKEN_H_

#include <stdint.h>

#include <string_view>

#include "tools/fidl/fidlc/src/source_span.h"

namespace fidlc {

// A Token represents a typed view into a source buffer. That is, it
// has a TokenKind, and it has a buffer representing the data
// corresponding to the token. No processing is done on the data:
// string or numeric literals aren't further parsed, identifiers
// uniqued, and so on.
class Token {
 public:
  enum Kind : uint8_t {
#define TOKEN(Name) k##Name,
#include "tools/fidl/fidlc/src/token_definitions.inc"
#undef TOKEN
  };

  // Each identifier token is assigned a subkind, which are defined below, or a sentinel value if
  // the token does not match one of the subkinds, to make it simple for the parser to either
  // consume a generic identifier or one that matches a specific subkind. This design makes it so
  // that there are no "reserved keywords" and any identifier can be defined by the user.
  enum Subkind : uint8_t {
    kNone = 0,
#define TOKEN_SUBKIND(Name, Spelling) k##Name,
#include "tools/fidl/fidlc/src/token_definitions.inc"
#undef TOKEN_SUBKIND
  };

  class KindAndSubkind {
   public:
    explicit constexpr KindAndSubkind(Kind kind) : kind_(kind), subkind_(Subkind::kNone) {}
    explicit constexpr KindAndSubkind(Subkind subkind)
        : kind_(Kind::kIdentifier), subkind_(subkind) {}
    explicit constexpr KindAndSubkind(Kind kind, Subkind subkind)
        : kind_(kind), subkind_(subkind) {}

    constexpr Kind kind() const { return kind_; }
    constexpr Subkind subkind() const { return subkind_; }
    constexpr uint16_t combined() const {
      return static_cast<uint16_t>(kind_) | static_cast<uint16_t>(subkind_ << 8);
    }

   private:
    Kind kind_;
    Subkind subkind_;
  };

  Token(SourceSpan span, uint16_t leading_newlines, Kind kind, Subkind subkind)
      : span_(span),
        leading_newlines_(leading_newlines),
        kind_and_subkind_(KindAndSubkind(kind, subkind)) {}

  Token() : Token(SourceSpan(), 0, Token::Kind::kNotAToken, Token::Subkind::kNone) {}

  static const char* Name(KindAndSubkind kind_and_subkind) {
    switch (kind_and_subkind.combined()) {
#define TOKEN(Name)          \
  case Token::Kind::k##Name: \
    return #Name;
#include "tools/fidl/fidlc/src/token_definitions.inc"
#undef TOKEN
#define TOKEN_SUBKIND(Name, Spelling)                                                       \
  case Token::KindAndSubkind(Token::Kind::kIdentifier, Token::Subkind::k##Name).combined(): \
    return #Spelling;
#include "tools/fidl/fidlc/src/token_definitions.inc"
#undef TOKEN_SUBKIND
      default:
        return "<unknown token>";
    }
  }

  const SourceSpan& span() const { return span_; }
  std::string_view data() const { return span_.data(); }
  const char* ptr() const { return span_.data().data(); }
  uint16_t leading_newlines() const { return leading_newlines_; }
  Kind kind() const { return kind_and_subkind_.kind(); }
  Subkind subkind() const { return kind_and_subkind_.subkind(); }
  KindAndSubkind kind_and_subkind() const { return kind_and_subkind_; }

  void set_leading_newlines(uint16_t leading_newlines) { leading_newlines_ = leading_newlines; }

 private:
  SourceSpan span_;
  uint16_t leading_newlines_;
  KindAndSubkind kind_and_subkind_;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_TOKEN_H_
