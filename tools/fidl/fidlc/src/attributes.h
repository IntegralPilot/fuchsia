// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_ATTRIBUTES_H_
#define TOOLS_FIDL_FIDLC_SRC_ATTRIBUTES_H_

#include <lib/fit/function.h>

#include <memory>
#include <optional>

#include "tools/fidl/fidlc/src/source_span.h"
#include "tools/fidl/fidlc/src/values.h"

namespace fidlc {

struct AttributeArg final {
  AttributeArg(std::optional<SourceSpan> name, std::unique_ptr<Constant> value, SourceSpan span)
      : name(name), value(std::move(value)), span(span) {}

  std::unique_ptr<AttributeArg> Clone() const;

  // Span of just the argument name, e.g. "bar". This is initially null for
  // arguments like `@foo("abc")`, but will be set during compilation.
  std::optional<SourceSpan> name;
  std::unique_ptr<Constant> value;
  // Span of the entire argument, e.g. `bar="abc"`, or `"abc"` if unnamed.
  const SourceSpan span;

  // Default name to use for arguments like `@foo("abc")` when there is no
  // schema for `@foo` we can use to infer the name.
  static constexpr std::string_view kDefaultAnonymousName = "value";
};

struct Attribute final {
  Attribute(SourceSpan name, std::vector<std::unique_ptr<AttributeArg>> args, SourceSpan span)
      : name(name), args(std::move(args)), span(span) {}

  const AttributeArg* GetArg(std::string_view arg_name) const;

  // Returns the lone argument if there is exactly 1 and it is not named. For
  // example it returns non-null for `@foo("x")` but not for `@foo(bar="x")`.
  AttributeArg* GetStandaloneAnonymousArg() const;

  std::unique_ptr<Attribute> Clone() const;

  // Span of just the attribute name not including the "@", e.g. "foo".
  const SourceSpan name;
  const std::vector<std::unique_ptr<AttributeArg>> args;
  // Span of the entire attribute, e.g. `@foo(bar="abc")`.
  const SourceSpan span;
  // Set to true by Library::CompileAttribute.
  bool compiled = false;

  // We parse `///` doc comments as nameless RawAttribute with `provenance` set
  // to kDocComment. When consuming into an Attribute, we use this name.
  static constexpr std::string_view kDocCommentName = "doc";
  // We parse modifier availabilities like `flexible(added=5)` as nameless
  // RawAttribute with `provenance` set to kModifierAvailability. When consuming
  // into an Attribute, we use this name.
  static constexpr std::string_view kModifierAvailabilityName = "available";
};

// In the flat AST, "no attributes" is represented by an AttributeList
// containing an empty vector. (In the raw AST, null is used instead.)
struct AttributeList final {
  AttributeList() = default;
  explicit AttributeList(std::vector<std::unique_ptr<Attribute>> attributes)
      : attributes(std::move(attributes)) {}

  bool Empty() const { return attributes.empty(); }
  const Attribute* Get(std::string_view attribute_name) const;
  Attribute* Get(std::string_view attribute_name);
  std::unique_ptr<AttributeList> Clone() const;

  std::vector<std::unique_ptr<Attribute>> attributes;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_ATTRIBUTES_H_
