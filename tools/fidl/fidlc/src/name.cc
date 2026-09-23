// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/name.h"

#include <zircon/assert.h>

#include "tools/fidl/fidlc/src/utils.h"

namespace fidlc {

// static
std::string NamingContext::BuildFlattenedName(SourceSpan name, Kind kind,
                                              const std::shared_ptr<NamingContext>& parent) {
  switch (kind) {
    case Kind::kDecl:
      return std::string(name.data());
    case Kind::kLayoutMember:
      return ToUpperCamelCase(std::string(name.data()));
    case Kind::kMethodRequest: {
      std::string result = ToUpperCamelCase(std::string(parent->name_.data()));
      result.append(ToUpperCamelCase(std::string(name.data())));
      result.append("Request");
      return result;
    }
    case Kind::kMethodResponse: {
      std::string result = ToUpperCamelCase(std::string(parent->name_.data()));
      result.append(ToUpperCamelCase(std::string(name.data())));
      result.append("Response");
      return result;
    }
  }
}

std::shared_ptr<NamingContext> NamingContext::Create(const Name& decl_name) {
  ZX_ASSERT_MSG(decl_name.span().has_value(),
                "cannot have a naming context from a name without a span");
  return Create(decl_name.span().value());
}

std::vector<std::string> NamingContext::Context() const {
  std::vector<std::string> names;
  const auto* current = this;
  while (current) {
    // Internally, we don't store a separate context item to represent whether a
    // layout is the request or response, since this bit of information is
    // embedded in the Kind. When collapsing the stack of contexts into a list
    // of strings, we need to flatten this case out to avoid losing this data.
    switch (current->kind_) {
      case Kind::kMethodRequest:
        names.push_back("Request");
        break;
      case Kind::kMethodResponse:
        names.push_back("Response");
        break;
      case Kind::kDecl:
      case Kind::kLayoutMember:
        break;
    }

    names.emplace_back(current->name_.data());
    current = current->parent_.get();
  }
  std::reverse(names.begin(), names.end());
  return names;
}

Name NamingContext::ToName(Library* library, SourceSpan declaration_span) {
  if (parent_ == nullptr)
    return Name::CreateSourced(library, name_);
  return Name::CreateAnonymous(library, declaration_span, shared_from_this(),
                               Name::Provenance::kAnonymousLayout);
}

std::optional<SourceSpan> Name::span() const {
  return std::visit(
      overloaded{
          [](const Sourced& sourced) -> std::optional<SourceSpan> { return sourced.span; },
          [](const Anonymous& anonymous) -> std::optional<SourceSpan> { return anonymous.span; },
          [](const Intrinsic& intrinsic) -> std::optional<SourceSpan> { return std::nullopt; }},
      value_);
}

std::string_view Name::decl_name() const {
  return std::visit(
      overloaded{[](const Sourced& sourced) -> std::string_view { return sourced.span.data(); },
                 [](const Anonymous& anonymous) -> std::string_view {
                   return anonymous.context->flattened_name();
                 },
                 [](const Intrinsic& intrinsic) -> std::string_view { return intrinsic.name; }},
      value_);
}

std::string Name::full_name() const {
  auto name = std::string(decl_name());
  if (member_name_.has_value()) {
    constexpr std::string_view kSeparator = ".";
    name.reserve(name.size() + kSeparator.size() + member_name_.value().size());
    name.append(kSeparator);
    name.append(member_name_.value());
  }
  return name;
}

}  // namespace fidlc
