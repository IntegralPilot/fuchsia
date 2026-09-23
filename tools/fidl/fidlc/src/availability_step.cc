// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/availability_step.h"

#include <zircon/assert.h>

#include "tools/fidl/fidlc/src/compile_step.h"
#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/reporter.h"
#include "tools/fidl/fidlc/src/versioning_types.h"

namespace fidlc {

void AvailabilityStep::RunImpl() {
  PopulateLexicalParents();
  library()->ForEachElement([&](Element* element) { CompileAvailability(element); });
  ValidateAvailabilities();
}

void AvailabilityStep::PopulateLexicalParents() {
  // First, map modifiers and members to their parents.
  for (const auto& entry : library()->declarations.all) {
    Decl* decl = entry.second;
    decl->ForEachEdge(
        [&](Element* parent, Element* child) { lexical_parents_.emplace(child, parent); });
  }

  // Second, map anonymous layouts to the struct/table/union member or method
  // whose type constructor they occur in. We do this with a helpful function
  // that recursively visits all anonymous types in `type_ctor`.
  std::function<void(Element*, const TypeConstructor*)> link_anonymous =
      [&](Element* member, const TypeConstructor* type_ctor) -> void {
    if (type_ctor->layout.IsSynthetic()) {
      auto anon_layout = type_ctor->layout.raw_synthetic().target.element();
      lexical_parents_.emplace(anon_layout, member);
    }
    for (const auto& param : type_ctor->parameters->items) {
      if (auto param_type_ctor = param->AsTypeCtor()) {
        link_anonymous(member, param_type_ctor);
      }
    }
  };

  for (auto& decl : library()->declarations.structs) {
    for (auto& member : decl->members) {
      link_anonymous(&member, member.type_ctor.get());
    }
  }
  for (auto& decl : library()->declarations.tables) {
    for (auto& member : decl->members) {
      link_anonymous(&member, member.type_ctor.get());
    }
  }
  for (auto& decl : library()->declarations.unions) {
    for (auto& member : decl->members) {
      link_anonymous(&member, member.type_ctor.get());
    }
  }
  for (auto& decl : library()->declarations.overlays) {
    for (auto& member : decl->members) {
      link_anonymous(&member, member.type_ctor.get());
    }
  }
  for (auto& protocol : library()->declarations.protocols) {
    for (auto& method : protocol->methods) {
      if (auto& request = method.maybe_request) {
        link_anonymous(&method, request.get());
      }
      if (auto& response = method.maybe_response) {
        link_anonymous(&method, response.get());
      }
    }
  }
  for (auto& decl : library()->declarations.resources) {
    for (auto& property : decl->properties) {
      link_anonymous(&property, property.type_ctor.get());
    }
  }
}

void AvailabilityStep::CompileAvailability(Element* element) {
  if (element->availability.state() != Availability::State::kUnset) {
    // Already compiled.
    return;
  }

  // Inheritance relies on the parent being compiled first.
  if (auto parent = LexicalParent(element)) {
    CompileAvailability(parent);
  }

  // If this is an anonymous layout, don't attempt to compile the attribute
  // since it can result in misleading errors. Instead, rely on
  // VerifyAttributesStep to report an error about the attribute placement.
  if (!element->IsAnonymousLayout()) {
    if (auto* attribute = element->attributes->Get("available")) {
      CompileAvailabilityFromAttribute(element, attribute);
      return;
    }
  }

  // There is no attribute, so simulate an empty one -- unless this is the
  // library declaration, in which case we default to @available(added=HEAD).
  std::optional<Version> default_added;
  if (element->kind == Element::Kind::kLibrary) {
    ZX_ASSERT(element == library());
    library()->platform = Platform::Unversioned();
    default_added = Version::kHead;
  }
  bool valid = element->availability.Init({.added = default_added});
  ZX_ASSERT_MSG(valid, "initializing default availability should succeed");
  if (auto source = AvailabilityToInheritFrom(element)) {
    auto result = element->availability.Inherit(source.value());
    ZX_ASSERT_MSG(result.Ok(), "inheriting into default availability should succeed");
  }
}

static bool CanBeRenamed(Element::Kind kind) {
  switch (kind) {
    case Element::Kind::kAlias:
    case Element::Kind::kBits:
    case Element::Kind::kBuiltin:
    case Element::Kind::kConst:
    case Element::Kind::kEnum:
    case Element::Kind::kLibrary:
    case Element::Kind::kModifier:
    case Element::Kind::kNewType:
    case Element::Kind::kOverlay:
    case Element::Kind::kProtocol:
    case Element::Kind::kProtocolCompose:
    case Element::Kind::kResource:
    case Element::Kind::kService:
    case Element::Kind::kStruct:
    case Element::Kind::kTable:
    case Element::Kind::kUnion:
      return false;
    case Element::Kind::kBitsMember:
    case Element::Kind::kEnumMember:
    case Element::Kind::kOverlayMember:
    case Element::Kind::kProtocolMethod:
    case Element::Kind::kResourceProperty:
    case Element::Kind::kServiceMember:
    case Element::Kind::kStructMember:
    case Element::Kind::kTableMember:
    case Element::Kind::kUnionMember:
      return true;
  }
}

void AvailabilityStep::CompileAvailabilityFromAttribute(Element* element, Attribute* attribute) {
  CompileStep::CompileAttributeEarly(compiler(), attribute);

  const bool is_library = element->kind == Element::Kind::kLibrary;
  ZX_ASSERT(is_library == (element == library()));

  const auto platform = attribute->GetArg("platform");
  const auto added = attribute->GetArg("added");
  const auto deprecated = attribute->GetArg("deprecated");
  const auto removed = attribute->GetArg("removed");
  const auto replaced = attribute->GetArg("replaced");
  const auto renamed = attribute->GetArg("renamed");
  const auto note = attribute->GetArg("note");

  // These errors do not block further analysis.
  if (!is_library && attribute->args.empty()) {
    reporter()->Fail(ErrAvailableMissingArguments, attribute->span);
  }
  if (note && !deprecated && !removed && !replaced) {
    reporter()->Fail(ErrNoteWithoutDeprecationOrRemoval, attribute->span);
  }

  // These errors block further analysis because we don't know what's intended,
  // and proceeding further will lead to confusing error messages.
  // We use & to report as many errors as possible (&& would short circuit).
  bool ok = true;
  if (is_library) {
    if (!added) {
      ok &= reporter()->Fail(ErrLibraryAvailabilityMissingAdded, attribute->span);
    }
    if (replaced) {
      ok &= reporter()->Fail(ErrLibraryReplaced, replaced->span);
    }
  } else {
    if (platform) {
      ok &= reporter()->Fail(ErrPlatformNotOnLibrary, platform->span);
    }
    if (!library()->attributes->Get("available")) {
      ok &= reporter()->Fail(ErrMissingLibraryAvailability, attribute->span, library()->name);
    }
  }
  if (removed && replaced) {
    ok &= reporter()->Fail(ErrRemovedAndReplaced, attribute->span);
  }
  if (renamed) {
    if (!CanBeRenamed(element->kind)) {
      ok &= reporter()->Fail(ErrCannotBeRenamed, renamed->span, element->kind);
    }
    if (!replaced && !removed) {
      ok &= reporter()->Fail(ErrRenamedWithoutReplacedOrRemoved, renamed->span);
    }
    if (renamed->value->IsResolved()) {
      auto new_name = renamed->value->Value().AsString().value();
      if (new_name == element->GetName()) {
        ok &= reporter()->Fail(ErrRenamedToSameName, renamed->span, new_name);
      }
    }
  }
  if (element->kind == Element::Kind::kModifier) {
    for (auto& arg : attribute->args) {
      if (arg.get() != added && arg.get() != removed) {
        ok &= reporter()->Fail(ErrInvalidModifierAvailableArgument, arg->span, arg.get());
      }
    }
  }
  if (!ok) {
    element->availability.Fail();
    return;
  }

  const auto removed_or_replaced = removed ? removed : replaced;
  const auto init_args = Availability::InitArgs{
      .added = GetVersion(added),
      .deprecated = GetVersion(deprecated),
      .removed = GetVersion(removed_or_replaced),
      .replaced = replaced != nullptr,
  };
  if (is_library) {
    const auto library_platform = GetPlatform(platform).value_or(GetDefaultPlatform());
    library()->platform = library_platform;
    if (library_platform.is_unversioned()) {
      reporter()->Fail(ErrReservedPlatform, attribute->span, library_platform);
    } else if (!version_selection()->Contains(library_platform)) {
      reporter()->Fail(ErrPlatformVersionNotSelected, attribute->span, library(), library_platform);
    }
    if (!init_args.added) {
      // Return early to avoid letting the -inf from Availability::Unbounded()
      // propagate any further, since .Inherit() asserts added != -inf.
      element->availability.Fail();
      return;
    }
  }
  if (!element->availability.Init(init_args)) {
    std::string msg;
    if (added) {
      msg.append("added");
    }
    if (deprecated) {
      msg.append(msg.empty() ? "deprecated" : " <= deprecated");
    }
    if (removed) {
      msg.append(" < removed");
    } else if (replaced) {
      msg.append(" < replaced");
    }
    reporter()->Fail(ErrInvalidAvailabilityOrder, attribute->span, msg);
    // Return early to avoid confusing error messages about inheritance
    // conflicts for an availability that isn't even self-consistent.
    return;
  }

  // Reports an error for arg given its inheritance status.
  auto report = [&](const AttributeArg* arg, Availability::InheritResult::Status status) {
    const char* when;
    const AttributeArg* inherited_arg;
    switch (status) {
      case Availability::InheritResult::Status::kOk:
        return;
      case Availability::InheritResult::Status::kBeforeParentAdded:
        when = "before";
        inherited_arg = AncestorArgument(element, {"added"});
        break;
      case Availability::InheritResult::Status::kAfterParentDeprecated:
        when = "after";
        inherited_arg = AncestorArgument(element, {"deprecated"});
        break;
      case Availability::InheritResult::Status::kAfterParentRemoved:
        when = "after";
        inherited_arg = AncestorArgument(element, {"removed", "replaced"});
        break;
    }
    auto child_what = arg->name.value().data();
    auto parent_what = inherited_arg->name.value().data();
    reporter()->Fail(ErrAvailabilityConflictsWithParent, arg->span, arg, arg->value->span.data(),
                     inherited_arg, inherited_arg->value->span.data(), inherited_arg->span,
                     child_what, when, parent_what);
  };

  if (auto source = AvailabilityToInheritFrom(element)) {
    const auto result = element->availability.Inherit(source.value());
    report(added, result.added);
    report(deprecated, result.deprecated);
    report(removed_or_replaced, result.removed);
  }

  if (element->availability.state() != Availability::State::kInherited)
    return;
  // Modifiers are different from other elements because we don't combine them
  // from all selected versions. We just use the latest modifiers.
  if (element->kind == Element::Kind::kModifier)
    return;
  if (auto& platform = library()->platform.value();
      !platform.is_unversioned() && version_selection()->Contains(platform)) {
    auto& target_set = version_selection()->LookupSet(platform);
    if (target_set.size() > 1 && element->availability.state() == Availability::State::kInherited &&
        removed) {
      auto set = element->availability.set();
      for (auto target_version : target_set) {
        if (set.Contains(target_version)) {
          element->availability.SetLegacy();
          break;
        }
      }
    }
  }
}

Platform AvailabilityStep::GetDefaultPlatform() {
  auto platform = Platform::Parse(std::string(FirstComponent(library()->name)));
  ZX_ASSERT_MSG(platform, "library component should be valid platform");
  return platform.value();
}

std::optional<Platform> AvailabilityStep::GetPlatform(const AttributeArg* maybe_arg) {
  if (!(maybe_arg && maybe_arg->value->IsResolved())) {
    return std::nullopt;
  }
  auto str = maybe_arg->value->Value().AsString().value();
  auto platform = Platform::Parse(std::string(str));
  if (!platform) {
    reporter()->Fail(ErrInvalidPlatform, maybe_arg->value->span, str);
    return std::nullopt;
  }
  return platform;
}

std::optional<Version> AvailabilityStep::GetVersion(const AttributeArg* maybe_arg) {
  if (!(maybe_arg && maybe_arg->value->IsResolved())) {
    return std::nullopt;
  }
  // CompileAttributeEarly resolves version arguments to uint32.
  auto value = maybe_arg->value->Value().AsNumeric<uint32_t>().value();
  auto version = Version::From(value);
  // Do not allow referencing the LEGACY version directly. It may only be
  // specified on the command line, or in FIDL libraries via the `legacy`
  // argument to @available.
  if (!version || version == Version::kLegacy) {
    auto span = maybe_arg->value->span;
    reporter()->Fail(ErrInvalidVersion, span, span.data());
    return std::nullopt;
  }
  return version;
}

std::optional<Availability> AvailabilityStep::AvailabilityToInheritFrom(const Element* element) {
  const Element* parent = LexicalParent(element);
  if (!parent) {
    ZX_ASSERT_MSG(element == library(), "if it has no parent, it must be the library");
    return Availability::Unbounded();
  }
  if (parent->availability.state() == Availability::State::kInherited) {
    // The typical case: inherit from the parent.
    return parent->availability;
  }
  // The parent failed to compile, so don't try to inherit.
  return std::nullopt;
}

const AttributeArg* AvailabilityStep::AncestorArgument(
    const Element* element, const std::vector<std::string_view>& arg_names) {
  while ((element = LexicalParent(element))) {
    if (auto attribute = element->attributes->Get("available")) {
      for (auto name : arg_names) {
        if (auto arg = attribute->GetArg(name)) {
          return arg;
        }
      }
    }
  }
  ZX_PANIC("no ancestor exists for this arg");
}

Element* AvailabilityStep::LexicalParent(const Element* element) {
  ZX_ASSERT(element);
  if (element == library()) {
    return nullptr;
  }
  if (auto it = lexical_parents_.find(element); it != lexical_parents_.end()) {
    return it->second;
  }
  // If it's not in lexical_parents_, it must be a top-level declaration.
  return library();
}

namespace {

struct CmpAvailability {
  bool operator()(const Element* lhs, const Element* rhs) const {
    return lhs->availability.set() < rhs->availability.set();
  }
};

// Helper that checks for canonical name collisions on overlapping elements.
class NameValidator {
 public:
  NameValidator(Reporter* reporter, const Platform& platform)
      : reporter_(reporter), platform_(platform) {}

  void Insert(const Element* element) {
    // Skip elements whose availabilities we failed to compile.
    if (element->availability.state() != Availability::State::kInherited) {
      return;
    }
    auto set = element->availability.set();
    auto name = element->GetName();
    auto canonical_name = Canonicalize(name);
    auto& same_canonical_name = by_canonical_name_[canonical_name];

    // Note: This algorithm is worst-case O(n^2) in the number of elements
    // having the same name. It could be optimized to O(n*log(n)).
    for (auto other : same_canonical_name) {
      auto other_set = other->availability.set();
      auto overlap = VersionSet::Intersect(set, other_set);
      if (!overlap) {
        continue;
      }
      auto span = element->GetNameSource();
      auto other_name = other->GetName();
      auto other_span = other->GetNameSource();
      // Use a simplified error message when availabilities are the identical.
      if (set == other_set) {
        if (name == other_name) {
          reporter_->Fail(ErrNameCollision, span, element->kind, name, other->kind, other_span);
        } else {
          reporter_->Fail(ErrNameCollisionCanonical, span, element->kind, name, other->kind,
                          other_name, other_span, canonical_name);
        }
      } else {
        if (name == other_name) {
          reporter_->Fail(ErrNameOverlap, span, element->kind, name, other->kind, other_span,
                          overlap.value(), platform_);
        } else {
          reporter_->Fail(ErrNameOverlapCanonical, span, element->kind, name, other->kind,
                          other_name, other_span, canonical_name, overlap.value(), platform_);
        }
      }
      // Report at most one error per element to avoid noisy redundant errors.
      break;
    }
    same_canonical_name.insert(element);
  }

 private:
  Reporter* reporter_;
  const Platform& platform_;
  std::map<std::string, std::set<const Element*, CmpAvailability>> by_canonical_name_;
};

// Helper that checks for modifier conflicts on overlapping elements.
class ModifierValidator {
 public:
  explicit ModifierValidator(Reporter* reporter) : reporter_(reporter) {}

  void Insert(const Modifier* modifier) {
    // Skip elements whose availabilities we failed to compile.
    if (modifier->availability.state() != Availability::State::kInherited) {
      return;
    }
    auto set = modifier->availability.set();
    auto kind = modifier->value.index();
    auto& same_kind = by_kind_[kind];
    for (auto other : same_kind) {
      auto other_set = other->availability.set();
      auto overlap = VersionSet::Intersect(set, other_set);
      if (!overlap) {
        continue;
      }
      // We could emit more complicated error messages with the overlap range
      // like NameValidator does, but that's probably overkill for modifiers.
      if (modifier->value == other->value) {
        reporter_->Fail(ErrDuplicateModifier, modifier->name, modifier);
      } else {
        reporter_->Fail(ErrConflictingModifier, modifier->name, modifier, other);
      }
      // Report at most one error per modifier to avoid noisy redundant errors.
      break;
    }
    same_kind.insert(modifier);
  }

 private:
  Reporter* reporter_;
  std::map<size_t, std::set<const Modifier*, CmpAvailability>> by_kind_;
};

}  // namespace

void AvailabilityStep::ValidateAvailabilities() {
  auto& platform = library()->platform;
  if (!platform.has_value()) {
    // We failed to compile the library declaration's @available attribute.
    return;
  }
  NameValidator decl_validator(reporter(), *platform);
  for (auto& [name, decl] : library()->declarations.all) {
    decl_validator.Insert(decl);
    NameValidator member_validator(reporter(), *platform);
    decl->ForEachMember([&](Element* member) { member_validator.Insert(member); });
  }
  library()->ForEachElement([&](Element* element) {
    ModifierValidator modifier_validator(reporter());
    element->ForEachModifier(
        [&](const Modifier* modifier) { modifier_validator.Insert(modifier); });
  });
}

}  // namespace fidlc
