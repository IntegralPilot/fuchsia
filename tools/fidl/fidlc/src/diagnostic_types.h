// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_DIAGNOSTIC_TYPES_H_
#define TOOLS_FIDL_FIDLC_SRC_DIAGNOSTIC_TYPES_H_

#include <lib/stdcompat/type_traits.h>
#include <zircon/assert.h>

#include <sstream>
#include <string_view>

#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/properties.h"
#include "tools/fidl/fidlc/src/source_span.h"
#include "tools/fidl/fidlc/src/token.h"
#include "tools/fidl/fidlc/src/versioning_types.h"

namespace fidlc {

using ErrorId = uint32_t;

namespace internal {

std::string Display(char c);
std::string Display(const std::string& s);
std::string Display(std::string_view s);
std::string Display(const std::set<std::string_view>& s);
std::string Display(SourceSpan s);
std::string Display(Token::KindAndSubkind t);
std::string Display(Openness o);
std::string Display(Protocol::Method::Kind k);
std::string Display(const Attribute* a);
std::string Display(const AttributeArg* a);
std::string Display(const Constant* c);
std::string Display(Element::Kind k);
std::string Display(Decl::Kind k);
std::string Display(const Element* e);
std::string Display(const std::vector<const Decl*>& d);
std::string Display(const Type* t);
std::string Display(const Name& n);
std::string Display(const Platform& p);
std::string Display(Version v);
std::string Display(VersionRange r);
std::string Display(const VersionSet& s);
std::string Display(AbiKind k);
std::string Display(AbiValue v);
std::string Display(const std::vector<std::pair<VersionRange, SourceSpan>>& v);
template <typename T, typename = decltype(std::to_string(std::declval<T>()))>
std::string Display(T val) {
  return std::to_string(val);
}

// TODO(https://fxbug.dev/42064981): Use std::format when we're on C++20.
template <typename... Args>
std::string FormatDiagnostic(std::string_view msg, const Args&... args) {
  std::string displayed_args[] = {Display(args)...};
  std::stringstream s;
  size_t offset = 0;
  for (size_t i = 0; i < msg.size() - 2; i++) {
    if (msg[i] == '{' && msg[i + 1] >= '0' && msg[i + 1] <= '9' && msg[i + 2] == '}') {
      size_t index = msg[i + 1] - '0';
      s << msg.substr(offset, i - offset) << displayed_args[index];
      offset = i + 3;
    }
  }
  s << msg.substr(offset);
  return s.str();
}

constexpr size_t CountFormatArgs(std::string_view msg) {
  size_t count = 0;
  for (size_t i = 0; i < msg.size() - 2; i++) {
    if (msg[i] == '{' && msg[i + 1] >= '0' && msg[i + 1] <= '9' && msg[i + 2] == '}') {
      size_t index = msg[i + 1] - '0';
      count = std::max(count, index + 1);
    }
  }
  return count;
}

// No-op non-constexpr function used to produce an error.
inline void IncorrectNumberOfFormatArgs() {}

template <typename... Args>
constexpr void CheckFormatArgs(std::string_view msg) {
  static_assert(
      (std::is_same_v<Args, std::remove_const_t<std::remove_reference_t<Args>>> && ...),
      "remove redundant `const` or `&`; DiagnosticDef args are always passed by const reference");
  static_assert(((!std::is_pointer_v<Args> || std::is_const_v<std::remove_pointer_t<Args>>) && ...),
                "use a const pointer; DiagnosticDef args should not be mutable pointers");
  static_assert(((!std::is_same_v<Args, std::string>) && ...),
                "use std::string_view, not std::string");

  // We can't static_assert below because the compiler doesn't know msg is
  // always constexpr. Instead, we generate a "must be initialized by a constant
  // expression" error by calling a non-constexpr function. The error only
  // happens if the condition is true. Otherwise, the call gets eliminated.
  if (sizeof...(Args) != internal::CountFormatArgs(msg)) {
    IncorrectNumberOfFormatArgs();
  }
}

}  // namespace internal

// A tag that indicates whether a diagnostic definition is an error or warning.
// In the future this could be extended to include hints, suggestions, etc.
enum class DiagnosticKind : uint8_t {
  kError,
  kWarning,
  kRetired,
};

// Extra fields set on some diagnostic definitions.
struct DiagnosticOptions {
  // If true, the error message will link to the diagnostic on fuchsia.dev.
  bool documented = true;
};

struct DiagnosticDef {
  constexpr DiagnosticDef(ErrorId id, DiagnosticKind kind, std::string_view msg,
                          DiagnosticOptions opts)
      : id(id), kind(kind), msg(msg), opts(opts) {}
  DiagnosticDef(const DiagnosticDef&) = delete;

  // Returns a string of the form "fi-NNNN".
  std::string FormatId() const;

  ErrorId id;
  DiagnosticKind kind;
  std::string_view msg;
  DiagnosticOptions opts;
};

// The definition of an error. All instances of ErrorDef are in diagnostics.h.
// Template args define format parameters in the error message.
template <ErrorId Id, typename... Args>
struct ErrorDef final : DiagnosticDef {
  constexpr explicit ErrorDef(std::string_view msg, DiagnosticOptions opts = {})
      : DiagnosticDef(Id, DiagnosticKind::kError, msg, opts) {
    internal::CheckFormatArgs<Args...>(msg);
  }
};

// The definition of a warning. All instances of WarningDef are in
// diagnostics.h. Template args define format parameters in the warning message.
template <ErrorId Id, typename... Args>
struct WarningDef final : DiagnosticDef {
  constexpr explicit WarningDef(std::string_view msg, DiagnosticOptions opts = {})
      : DiagnosticDef(Id, DiagnosticKind::kWarning, msg, opts) {
    internal::CheckFormatArgs<Args...>(msg);
  }
};

// The definition of an obsolete error. These are never displayed to the user -
// they are merely used to retire error numerals from circulation.
template <ErrorId Id>
struct RetiredDef final : DiagnosticDef {
  constexpr explicit RetiredDef()
      : DiagnosticDef(Id, DiagnosticKind::kRetired, "retired diagnostic", {}) {}
};

// A Diagnostic is the result of instantiating a DiagnosticDef with arguments.
// It stores a formatted std::string where "{}" markers have been replaced by
// arguments. It also stores a SourceSpan indicating where the problem occurred.
struct Diagnostic {
  template <typename... Args>
  Diagnostic(const DiagnosticDef& def, SourceSpan span, const Args&... args)
      : def(def), span(span), msg(internal::FormatDiagnostic(def.msg, args...)) {}
  Diagnostic(const Diagnostic&) = delete;

  // The factory functions below could be constructors, and std::make_unique
  // would work fine. However, template error messages are better with static
  // functions because it doesn't have to try every constructor.

  template <ErrorId Id, typename... Args>
  static std::unique_ptr<Diagnostic> MakeError(const ErrorDef<Id, Args...>& def, SourceSpan span,
                                               const cpp20::type_identity_t<Args>&... args) {
    return std::make_unique<Diagnostic>(def, span, args...);
  }

  template <ErrorId Id, typename... Args>
  static std::unique_ptr<Diagnostic> MakeWarning(const WarningDef<Id, Args...>& def,
                                                 SourceSpan span,
                                                 const cpp20::type_identity_t<Args>&... args) {
    return std::make_unique<Diagnostic>(def, span, args...);
  }

  // Formats the error message to a string.
  std::string Format() const;

  const DiagnosticDef& def;
  const SourceSpan span;
  const std::string msg;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_DIAGNOSTIC_TYPES_H_
