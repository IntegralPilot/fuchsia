// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/diagnostic_types.h"

#include <zircon/assert.h>

#include <ostream>

#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/names.h"
#include "tools/fidl/fidlc/src/replacement_step.h"
#include "tools/fidl/fidlc/src/source_span.h"

namespace fidlc {

namespace internal {

std::string Display(char c) { return std::string(1, c); }

std::string Display(const std::string& s) { return s; }

std::string Display(std::string_view s) { return std::string(s); }

std::string Display(const std::set<std::string_view>& s) {
  std::stringstream ss;
  for (auto it = s.begin(); it != s.end(); it++) {
    if (it != s.cbegin()) {
      ss << ", ";
    }
    ss << *it;
  }
  return ss.str();
}

std::string Display(SourceSpan s) { return s.position_str(); }

std::string Display(Token::KindAndSubkind t) { return std::string(Token::Name(t)); }

std::string Display(Openness o) {
  switch (o) {
    case Openness::kOpen:
      return "open";
    case Openness::kAjar:
      return "ajar";
    case Openness::kClosed:
      return "closed";
  }
}

std::string Display(Protocol::Method::Kind i) {
  switch (i) {
    case Protocol::Method::Kind::kOneWay:
      return "one-way method";
    case Protocol::Method::Kind::kTwoWay:
      return "two-way method";
    case Protocol::Method::Kind::kEvent:
      return "event";
  }
}

std::string Display(const Attribute* a) { return std::string(a->name.data()); }

std::string Display(const AttributeArg* a) {
  return a->name.has_value() ? std::string(a->name.value().data()) : "";
}

std::string Display(const Constant* c) { return NameConstant(c); }

std::string Display(Element::Kind k) {
  switch (k) {
    case Element::Kind::kBits:
      return "bits";
    case Element::Kind::kBitsMember:
      return "bits member";
    case Element::Kind::kBuiltin:
      return "builtin";
    case Element::Kind::kConst:
      return "const";
    case Element::Kind::kEnum:
      return "enum";
    case Element::Kind::kEnumMember:
      return "enum member";
    case Element::Kind::kLibrary:
      return "library";
    case Element::Kind::kModifier:
      return "modifier";
    case Element::Kind::kNewType:
      return "new-type";
    case Element::Kind::kProtocol:
      return "protocol";
    case Element::Kind::kProtocolCompose:
      return "protocol composition";
    case Element::Kind::kProtocolMethod:
      return "protocol method";
    case Element::Kind::kResource:
      return "resource";
    case Element::Kind::kResourceProperty:
      return "resource property";
    case Element::Kind::kService:
      return "service";
    case Element::Kind::kServiceMember:
      return "service member";
    case Element::Kind::kStruct:
      return "struct";
    case Element::Kind::kStructMember:
      return "struct member";
    case Element::Kind::kTable:
      return "table";
    case Element::Kind::kTableMember:
      return "table member";
    case Element::Kind::kAlias:
      return "alias";
    case Element::Kind::kUnion:
      return "union";
    case Element::Kind::kUnionMember:
      return "union member";
    case Element::Kind::kOverlay:
      return "overlay";
    case Element::Kind::kOverlayMember:
      return "overlay member";
  }
}

std::string Display(Decl::Kind k) { return Display(Decl::ElementKind(k)); }

std::string Display(const Element* e) {
  std::stringstream ss;
  ss << Display(e->kind) << " '" << e->GetName() << "'";
  return ss.str();
}

// Displays decls with arrows, e.g "struct 'Foo' -> union 'Bar' -> struct 'Foo'".
std::string Display(const std::vector<const Decl*>& d) {
  std::stringstream ss;
  for (auto it = d.begin(); it != d.end(); ++it) {
    if (it != d.begin()) {
      ss << " -> ";
    }
    ss << Display(*it);
  }
  return ss.str();
}

std::string Display(const Type* t) { return NameType(t); }

std::string Display(const Name& n) { return n.full_name(); }

std::string Display(const Platform& p) { return p.name(); }

std::string Display(Version v) { return v.ToString(); }

std::string Display(VersionRange r) {
  auto [a, b] = r.pair();
  std::stringstream ss;
  // Ranges with these values should never show up in error messages.
  ZX_ASSERT(a != Version::kNegInf && a != Version::kPosInf);
  ZX_ASSERT(b != Version::kNegInf);
  if (b == Version::kPosInf) {
    ss << "from version " << Display(a) << " onward";
  } else if (auto b_inclusive = b.Predecessor(); a == b_inclusive) {
    ss << "at version " << Display(a);
  } else {
    ZX_ASSERT(b != Version::kLegacy);
    ss << "from version " << Display(a) << " to " << Display(b_inclusive);
  }
  return ss.str();
}

std::string Display(const VersionSet& s) {
  auto& [x, maybe_y] = s.ranges();
  if (!maybe_y) {
    return Display(x);
  }
  ZX_ASSERT_MSG(x.pair().second != Version::kPosInf,
                "first range must have finite end if there are two");
  return Display(x) + " and " + Display(maybe_y.value());
}

std::string Display(AbiKind k) {
  switch (k) {
    case AbiKind::kOffset:
      return "offset";
    case AbiKind::kOrdinal:
      return "ordinal";
    case AbiKind::kValue:
      return "value";
    case AbiKind::kSelector:
      return "selector";
  }
}

std::string Display(AbiValue v) {
  return std::visit([](auto&& value) { return Display(value); }, v);
}

std::string Display(const std::vector<std::pair<VersionRange, SourceSpan>>& v) {
  std::stringstream ss;
  for (size_t i = 0; i < v.size(); ++i) {
    auto& [range, span] = v[i];
    ss << Display(range) << " (" << Display(span) << ")";
    if (i != v.size() - 1)
      ss << ", ";
    if (i == v.size() - 2)
      ss << "and ";
  }
  return ss.str();
}

}  // namespace internal

std::string DiagnosticDef::FormatId() const {
  char id_str[8];
  std::snprintf(id_str, 8, "fi-%04d", id);
  return id_str;
}

std::string Diagnostic::Format() const {
  std::ostringstream out;
  out << msg;
  if (def.opts.documented) {
    out << " [https://fuchsia.dev/error/" << def.FormatId() << ']';
  }
  return out.str();
}

}  // namespace fidlc
