// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/replacement_step.h"

#include "tools/fidl/fidlc/src/versioning_types.h"

namespace fidlc {

namespace {

Version Start(const Element* element) { return element->availability.range().pair().first; }
Version End(const Element* element) { return element->availability.range().pair().second; }

std::optional<std::string_view> GetRenamed(const Element* element) {
  if (auto available = element->attributes->Get("available")) {
    if (auto renamed = available->GetArg("renamed")) {
      return renamed->value->Value().AsString();
    }
  }
  return std::nullopt;
}

// A wrapper around Element used to special case composed methods.
class Member {
 public:
  explicit Member(const Element* element) : element_(element) {}
  explicit Member(Protocol::MethodWithInfo info) : element_(info.method) {
    if (info.composed) {
      if (info.composed->availability.ending() == Availability::Ending::kSplit &&
          End(info.composed) == End(info.method)) {
        source_ = info.method;
      } else {
        source_ = info.composed;
      }
    }
  }

  const Element* element() { return element_; }
  Availability::Ending ending() { return source_->availability.ending(); }
  const Attribute* available() { return source_->attributes->Get("available"); }

 private:
  const Element* element_;
  // The @available attribute of source_ determines how element_ is removed or
  // replaced. This is either the same as element_, or a `compose` member.
  const Element* source_ = element_;
};

void ForEachMember(Decl* decl, const fit::function<void(Member)> callback) {
  if (decl->kind == Decl::Kind::kProtocol) {
    for (auto& info : static_cast<Protocol*>(decl)->all_methods)
      callback(Member(info));
  } else {
    decl->ForEachMember([&](Element* element) { callback(Member(element)); });
  }
}

}  // namespace

void ReplacementStep::RunImpl() {
  CheckDecls();
  CheckMembers();
}

void ReplacementStep::CheckDecls() {
  // Goal: Compare decls with the same name whose start/end version coincide.
  using Key = std::pair<std::string_view, Version>;
  auto& declarations = library()->declarations.all;
  // Step 1: Populate maps for removed and replaced decls.
  std::map<Key, const Decl*> removed, replaced;
  for (auto& [name, decl] : declarations) {
    if (decl->availability.ending() == Availability::Ending::kRemoved) {
      removed.try_emplace({name, End(decl)}, decl);
    } else if (decl->availability.ending() == Availability::Ending::kReplaced) {
      replaced.try_emplace({name, End(decl)}, decl);
    }
  }
  // Step 2: Do a second pass to match up replacement decls.
  for (auto& [name, decl] : declarations) {
    Key key = {name, Start(decl)};
    if (auto it = removed.find(key); it != removed.end()) {
      const Decl* old = it->second;
      auto span = old->attributes->Get("available")->GetArg("removed")->span;
      reporter()->Fail(ErrInvalidRemoved, span, old, key.second, decl->GetNameSource());
    }
    replaced.erase(key);
  }
  // Step 3: Report errors for replaced decls where Step 2 found no replacement.
  for (auto& [key, decl] : replaced) {
    auto span = decl->attributes->Get("available")->GetArg("replaced")->span;
    reporter()->Fail(ErrInvalidReplaced, span, decl, key.second);
  }
}

void ReplacementStep::CheckMembers() {
  // Goal: Compare members with the same name whose start/end version coincide.
  // Since removed/replaced members cause their decl to be split by ResolveStep,
  // we compare members from the two halves of each split.
  auto& declarations = library()->declarations.all;
  for (auto it = declarations.begin(); it != declarations.end(); ++it) {
    Decl* old_decl = it->second;
    if (old_decl->availability.ending() != Availability::Ending::kSplit)
      continue;
    // The new decl comes next because std::multimap preserves insertion order.
    auto next = std::next(it);
    ZX_ASSERT(next != declarations.end());
    Decl* new_decl = next->second;
    ZX_ASSERT(old_decl->GetName() == new_decl->GetName());
    ZX_ASSERT(old_decl->kind == new_decl->kind);
    Version version = End(old_decl);
    ZX_ASSERT(Start(new_decl) == version);
    // Step 1: Populate maps for removed and replaced members.
    std::map<std::string_view, Member> name_removed;
    std::map<AbiValue, Member> value_removed;
    std::map<std::pair<std::string_view, AbiValue>, Member> name_and_value_replaced;
    std::map<std::string_view, Member> name_replacements;
    ForEachMember(old_decl, [&](Member member) {
      auto end_name = GetRenamed(member.element()).value_or(member.element()->GetName());
      if (member.ending() == Availability::Ending::kRemoved) {
        name_removed.try_emplace(end_name, member);
        if (auto value = member.element()->abi_value())
          value_removed.try_emplace(value.value(), member);
      } else if (member.ending() == Availability::Ending::kReplaced) {
        auto value = member.element()->abi_value().value_or(0);
        name_and_value_replaced.try_emplace({end_name, value}, member);
      }
    });
    // Step 2: Do a second pass to match up replacement members.
    ForEachMember(new_decl, [&](Member member) {
      auto name = member.element()->GetName();
      auto value = member.element()->abi_value().value_or(0);
      if (auto it = name_removed.find(name); it != name_removed.end()) {
        Member old = it->second;
        if (auto renamed = GetRenamed(old.element())) {
          reporter()->Fail(ErrInvalidRemovedAndRenamed, old.available()->span, old.element(),
                           version, renamed.value(), member.element()->GetNameSource());
        } else {
          reporter()->Fail(ErrInvalidRemoved, old.available()->span, old.element(), version,
                           member.element()->GetNameSource());
        }
      } else if (auto it = value_removed.find(value); it != value_removed.end()) {
        Member old = it->second;
        reporter()->Fail(ErrInvalidRemovedAbi, old.available()->span, old.element(), version,
                         member.element()->abi_kind().value(), value,
                         member.element()->GetNameSource(), name);
      }
      if (!name_and_value_replaced.erase({name, value}))
        name_replacements.try_emplace(name, member);
    });
    // Step 3: Report errors for replaced members where Step 2 found no replacement.
    for (auto& [name_and_value, member] : name_and_value_replaced) {
      if (auto it = name_replacements.find(name_and_value.first); it != name_replacements.end()) {
        auto old = it->second;
        reporter()->Fail(ErrInvalidReplacedAbi, member.available()->span, member.element(), version,
                         member.element()->abi_kind().value(),
                         member.element()->abi_value().value(), old.element()->abi_value().value(),
                         old.element()->GetNameSource());
      } else if (auto renamed = GetRenamed(member.element())) {
        reporter()->Fail(ErrInvalidReplacedAndRenamed, member.available()->span, member.element(),
                         version, renamed.value());
      } else {
        reporter()->Fail(ErrInvalidReplaced, member.available()->span, member.element(), version);
      }
    }
  }
}

}  // namespace fidlc
