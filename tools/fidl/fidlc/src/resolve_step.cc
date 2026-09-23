// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/resolve_step.h"

#include <zircon/assert.h>

#include <string_view>
#include <utility>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/flat_ast.h"

namespace fidlc {

void ResolveStep::RunImpl() {
  // In a single pass:
  // (1) parse all references into keys/contextuals;
  // (2) insert reference edges into the graph.
  library()->ForEachElement([&](Element* element) {
    VisitElement(element, Context(Context::Mode::kParseAndInsert, element));
  });

  // Add edges from protocols to result unions to stop result unions from being
  // shared between decomposed methods. This lets us mutate the result union (to
  // remove the framework error) when compiling methods in the CompileStep. The
  // edge has to come from the protocol, not the method, because in a protocol
  // like `open(added=2) protocol Foo { Bar() -> () error uint32; }`, Bar ends
  // up getting split at 2 even though 2 is not in Bar's set of points.
  for (auto& protocol : library()->declarations.protocols) {
    for (auto& method : protocol->methods) {
      if (method.maybe_result_union)
        graph_[protocol.get()].neighbors.insert(method.maybe_result_union);
    }
  }

  // Add all elements of this library to the graph, with membership edges.
  for (auto& entry : library()->declarations.all) {
    Decl* decl = entry.second;
    // Note: It's important to insert decl here so that (1) we properly
    // initialize its points in the next loop, and (2) we can always recursively
    // look up a neighbor in the graph, even if it has out-degree zero.
    graph_.try_emplace(decl);
    decl->ForEachEdge(
        [&](Element* parent, Element* child) { graph_[child].neighbors.insert(parent); });
  }

  // Initialize point sets for each element in the graph.
  for (auto& [element, info] : graph_) {
    // There shouldn't be any library elements in the graph because they are
    // special (they don't get split, so their availabilities stop at
    // kInherited). We don't add membership edges to them, and we specifically
    // avoid adding reference edges to them in ResolveStep::ParseReference.
    ZX_ASSERT(element->kind != Element::Kind::kLibrary);
    // Each element starts with between 2 and 5 points. All have (1) `added` and
    // (2) `removed`. Some have (3) `deprecated`. Some are added back for legacy
    // support, so they have (4) LEGACY and (5) +inf. Elements from other
    // libraries (that exist due to reference edges) only ever have 2 points
    // because those libraries are already compiled, hence post-decomposition.
    info.points = element->availability.points();
  }

  // Run the temporal decomposition algorithm.
  std::vector<const Element*> worklist;
  worklist.reserve(graph_.size());
  for (auto& [element, info] : graph_) {
    worklist.push_back(element);
  }
  while (!worklist.empty()) {
    const Element* element = worklist.back();
    worklist.pop_back();
    auto& [element_points, neighbors] = graph_.at(element);
    for (auto& neighbor : neighbors) {
      auto& neighbor_points = graph_.at(neighbor).points;
      auto min = *neighbor_points.begin();
      auto max = *neighbor_points.rbegin();
      bool pushed_neighbor = false;
      for (auto p : element_points) {
        if (p > min && p < max) {
          auto [iter, inserted] = neighbor_points.insert(p);
          if (inserted && !pushed_neighbor) {
            worklist.push_back(neighbor);
            pushed_neighbor = true;
          }
        }
      }
    }
  }

  // Split declarations based on the final point sets.
  Library::Declarations decomposed_declarations;
  for (auto [name, decl] : library()->declarations.all) {
    auto decl_present = decl->availability.set();
    auto& points = graph_.at(decl).points;
    ZX_ASSERT_MSG(points.size() >= 2, "every decl must have at least 2 points");
    // Note: Even if there are only two points, we still "split" the decl into
    // one piece. There is no need to make it a special case.
    auto prev = *points.begin();
    for (auto it = std::next(points.begin()); it != points.end(); ++it) {
      auto range = VersionRange(prev, *it);
      if (auto overlap = VersionSet::Intersect(VersionSet(range), decl_present)) {
        ZX_ASSERT_MSG(overlap == VersionSet(range),
                      "decomposed range must wholly inside or outside the availability");
        decomposed_declarations.Insert(decl->Split(range));
      } else {
        auto [a, b] = range.pair();
        auto [x, maybe_y] = decl_present.ranges();
        auto removed = x.pair().second;
        ZX_ASSERT_MSG(a >= removed && b <= Version::kLegacy,
                      "skipped range must lie within [removed, LEGACY)");
      }
      prev = *it;
    }
  }
  library()->declarations = std::move(decomposed_declarations);

  // Resolve all references and validate them.
  library()->ForEachElement([&](Element* element) {
    VisitElement(element, Context(Context::Mode::kResolveAndValidate, element));
  });
}

void ResolveStep::VisitElement(Element* element, Context context) {
  for (auto& attribute : element->attributes->attributes) {
    // Skip attributes that are compiled early, e.g. @available.
    if (all_libraries()->RetrieveAttributeSchema(attribute.get()).IsCompileEarly()) {
      continue;
    }
    for (auto& arg : attribute->args) {
      VisitConstant(arg->value.get(), context);
    }
  }
  switch (element->kind) {
    case Element::Kind::kAlias: {
      auto alias_decl = static_cast<Alias*>(element);
      VisitTypeConstructor(alias_decl->partial_type_ctor.get(), context);
      break;
    }
    case Element::Kind::kNewType: {
      auto new_type = static_cast<NewType*>(element);
      VisitTypeConstructor(new_type->type_ctor.get(), context);
      break;
    }
    case Element::Kind::kConst: {
      auto const_decl = static_cast<Const*>(element);
      VisitTypeConstructor(const_decl->type_ctor.get(), context);
      VisitConstant(const_decl->value.get(), context);
      break;
    }
    case Element::Kind::kBits: {
      auto bits_decl = static_cast<Bits*>(element);
      VisitTypeConstructor(bits_decl->subtype_ctor.get(), context);
      break;
    }
    case Element::Kind::kBitsMember: {
      auto bits_member = static_cast<Bits::Member*>(element);
      VisitConstant(bits_member->value.get(), context);
      break;
    }
    case Element::Kind::kEnum: {
      auto enum_decl = static_cast<Enum*>(element);
      VisitTypeConstructor(enum_decl->subtype_ctor.get(), context);
      break;
    }
    case Element::Kind::kEnumMember: {
      auto enum_member = static_cast<Enum::Member*>(element);
      VisitConstant(enum_member->value.get(), context);
      break;
    }
    case Element::Kind::kStructMember: {
      auto struct_member = static_cast<Struct::Member*>(element);
      VisitTypeConstructor(struct_member->type_ctor.get(), context);
      if (auto& constant = struct_member->maybe_default_value) {
        VisitConstant(constant.get(), context);
      }
      break;
    }
    case Element::Kind::kTableMember: {
      auto table_member = static_cast<Table::Member*>(element);
      VisitTypeConstructor(table_member->type_ctor.get(), context);
      break;
    }
    case Element::Kind::kUnionMember: {
      auto union_member = static_cast<Union::Member*>(element);
      VisitTypeConstructor(union_member->type_ctor.get(), context);
      break;
    }
    case Element::Kind::kOverlayMember: {
      auto overlay_member = static_cast<Overlay::Member*>(element);
      VisitTypeConstructor(overlay_member->type_ctor.get(), context);
      break;
    }
    case Element::Kind::kProtocolCompose: {
      auto composed_protocol = static_cast<Protocol::ComposedProtocol*>(element);
      VisitReference(composed_protocol->reference, context);
      break;
    }
    case Element::Kind::kProtocolMethod: {
      auto method = static_cast<Protocol::Method*>(element);
      if (auto& type_ctor = method->maybe_request) {
        VisitTypeConstructor(type_ctor.get(), context);
      }
      if (auto& type_ctor = method->maybe_response) {
        VisitTypeConstructor(type_ctor.get(), context);
      }
      break;
    }
    case Element::Kind::kServiceMember: {
      auto service_member = static_cast<Service::Member*>(element);
      VisitTypeConstructor(service_member->type_ctor.get(), context);
      break;
    }
    case Element::Kind::kResource: {
      auto resource_decl = static_cast<Resource*>(element);
      VisitTypeConstructor(resource_decl->subtype_ctor.get(), context);
      break;
    }
    case Element::Kind::kResourceProperty: {
      auto resource_property = static_cast<Resource::Property*>(element);
      VisitTypeConstructor(resource_property->type_ctor.get(), context);
      break;
    }
    case Element::Kind::kBuiltin:
    case Element::Kind::kLibrary:
    case Element::Kind::kModifier:
    case Element::Kind::kProtocol:
    case Element::Kind::kService:
    case Element::Kind::kStruct:
    case Element::Kind::kTable:
    case Element::Kind::kUnion:
    case Element::Kind::kOverlay:
      break;
  }
}

void ResolveStep::VisitTypeConstructor(TypeConstructor* type_ctor, Context context) {
  VisitReference(type_ctor->layout, context);
  for (auto& param : type_ctor->parameters->items) {
    switch (param->kind) {
      case LayoutParameter::kLiteral:
        break;
      case LayoutParameter::kType: {
        auto type_param = static_cast<TypeLayoutParameter*>(param.get());
        VisitTypeConstructor(type_param->type_ctor.get(), context);
        break;
      }
      case LayoutParameter::kIdentifier: {
        auto identifier_param = static_cast<IdentifierLayoutParameter*>(param.get());
        VisitReference(identifier_param->reference, context);
        // After resolving an IdentifierLayoutParameter, we can determine
        // whether it's a type constructor or a constant.
        if (identifier_param->reference.state() == Reference::State::kResolved) {
          identifier_param->Disambiguate();
        }
        break;
      }
    }
  }
  auto constraint_context = ConstraintContext(type_ctor, context);
  for (auto& constraint : type_ctor->constraints->items) {
    VisitConstant(constraint.get(), constraint_context);
  }
}

void ResolveStep::VisitConstant(Constant* constant, Context context) {
  switch (constant->kind) {
    case Constant::Kind::kLiteral:
      break;
    case Constant::Kind::kIdentifier: {
      auto identifier_constant = static_cast<IdentifierConstant*>(constant);
      VisitReference(identifier_constant->reference, context);
      break;
    }
    case Constant::Kind::kBinaryOperator: {
      auto binop_constant = static_cast<BinaryOperatorConstant*>(constant);
      VisitConstant(binop_constant->left_operand.get(), context);
      VisitConstant(binop_constant->right_operand.get(), context);
      break;
    }
  }
}

ResolveStep::Context ResolveStep::ConstraintContext(const TypeConstructor* type_ctor,
                                                    Context context) {
  switch (context.mode) {
    case Context::Mode::kParseAndInsert: {
      // Assume all constraints might be contextual.
      Context augmented(Context::Mode::kParseAndInsert, context.enclosing);
      augmented.allow_contextual = true;
      return augmented;
    }
    case Context::Mode::kResolveAndValidate:
      // Handled below.
      break;
  }
  if (type_ctor->layout.state() != Reference::State::kResolved) {
    return context;
  }
  auto target = type_ctor->layout.resolved().element();
  if (target->kind != Element::Kind::kResource) {
    return context;
  }
  auto subtype_property = static_cast<Resource*>(target)->LookupProperty("subtype");
  if (!subtype_property) {
    return context;
  }
  auto& subtype_layout = subtype_property->type_ctor->layout;
  // If the resource_definition is in the same library, we might not have
  // resolved it yet depending on the element traversal order.
  ResolveReference(subtype_layout, Context(Context::Mode::kResolveAndValidate, subtype_property));
  if (subtype_layout.state() == Reference::State::kFailed) {
    return context;
  }
  auto subtype_target = subtype_layout.resolved().element();
  if (subtype_target->kind != Element::Kind::kEnum) {
    return context;
  }
  Context augmented(Context::Mode::kResolveAndValidate, context.enclosing);
  augmented.maybe_resource_subtype = static_cast<Enum*>(subtype_target);
  return augmented;
}

// Helper for looking up names as libraries, decls, or members. The Try* methods
// do not report an error, while the Must* methods do.
class ResolveStep::Lookup final {
 public:
  Lookup(ResolveStep* step, const Reference& ref) : step_(step), ref_(ref) {}

  const Library* TryLibrary(std::string_view name) {
    auto root_library = step_->all_libraries()->root_library();
    if (name == root_library->name) {
      return root_library;
    }
    auto filename = ref_.span().source_file().filename();
    return step_->library()->dependencies.LookupAndMarkUsed(filename, name);
  }

  std::optional<Reference::Key> TryDecl(const Library* library, std::string_view name) {
    auto [begin, end] = library->declarations.all.equal_range(name);
    if (begin == end) {
      return std::nullopt;
    }
    // TryDecl is only used from within ParseSourcedReference, which should not
    // resolve Internal declarations names; only synthetic references can
    // resolve internal names.
    // Internal declarations should only exist in the root library, and should
    // never have conflicting names, so any match should have only one element.
    // We therefore return nullopt if any of the declarations found is an
    // internal one.
    for (auto it = begin; it != end; ++it) {
      if (it->second->kind == Decl::Kind::kBuiltin &&
          static_cast<Builtin*>(it->second)->IsInternal()) {
        return std::nullopt;
      }
    }
    return Reference::Key(library, name);
  }

  std::optional<Reference::Key> MustDecl(const Library* library, std::string_view name) {
    if (auto key = TryDecl(library, name)) {
      return key;
    }
    reporter()->Fail(ErrNameNotFound, ref_.span(), name, library);
    return std::nullopt;
  }

  Element* TryMember(Decl* parent, std::string_view name) {
    switch (parent->kind) {
      case Decl::Kind::kBits:
        for (auto& member : static_cast<Bits*>(parent)->members) {
          if (member.name.data() == name) {
            return &member;
          }
        }
        return nullptr;
      case Decl::Kind::kEnum:
        for (auto& member : static_cast<Enum*>(parent)->members) {
          if (member.name.data() == name) {
            return &member;
          }
        }
        return nullptr;
      default:
        return nullptr;
    }
  }

  Element* MustMember(Decl* parent, std::string_view name) {
    switch (parent->kind) {
      case Decl::Kind::kBits:
      case Decl::Kind::kEnum:
        if (auto member = TryMember(parent, name)) {
          return member;
        }
        break;
      default:
        reporter()->Fail(ErrCannotReferToMember, ref_.span(), parent);
        return nullptr;
    }
    reporter()->Fail(ErrMemberNotFound, ref_.span(), parent, name);
    return nullptr;
  }

 private:
  Reporter* reporter() { return step_->reporter(); }

  ResolveStep* step_;
  const Reference& ref_;
};

void ResolveStep::VisitReference(Reference& ref, Context context) {
  switch (context.mode) {
    case Context::Mode::kParseAndInsert:
      ParseReference(ref, context);
      InsertReferenceEdges(ref, context);
      break;
    case Context::Mode::kResolveAndValidate: {
      ResolveReference(ref, context);
      ValidateReference(ref, context);
      break;
    }
  }
}

void ResolveStep::ParseReference(Reference& ref, Context context) {
  auto initial_state = ref.state();
  auto checkpoint = reporter()->Checkpoint();
  switch (initial_state) {
    case Reference::State::kRawSynthetic:
      ParseSyntheticReference(ref, context);
      break;
    case Reference::State::kRawSourced:
      ParseSourcedReference(ref, context);
      break;
    default:
      ZX_PANIC("unexpected reference state");
  }
  if (ref.state() == initial_state) {
    ZX_ASSERT_MSG(checkpoint.NumNewErrors() > 0, "should have reported an error");
    ref.MarkFailed();
    return;
  }
  // If the enclosing element is the library, we must be visiting an identifier
  // constant in one of its attributes, e.g. `@foo(CONSTANT) library example;`.
  // This isn't allowed because it's unclear what it would mean in a versioned
  // library where CONSTANT takes on different values at different versions.
  if (context.enclosing->kind == Element::Kind::kLibrary) {
    reporter()->Fail(ErrReferenceInLibraryAttribute, ref.span());
    ref.MarkFailed();
    return;
  }
}

void ResolveStep::ParseSyntheticReference(Reference& ref, Context context) {
  // Note that we can't use target.name() here because it returns a Name by
  // value, which would go out of scope.
  auto& name = ref.raw_synthetic().target.element()->AsDecl()->name;
  ref.SetKey(Reference::Key(name.library(), name.decl_name()));
}

void ResolveStep::ParseSourcedReference(Reference& ref, Context context) {
  // This implements the identifier resolution algorithm from
  // https://fuchsia.dev/fuchsia-src/reference/fidl/language/language#resolution-algorithm
  const auto& components = ref.raw_sourced().components;
  Lookup lookup(this, ref);
  switch (components.size()) {
    case 1: {
      if (auto key = lookup.TryDecl(library(), components[0])) {
        ref.SetKey(key.value());
      } else if (auto key = lookup.TryDecl(all_libraries()->root_library(), components[0])) {
        ref.SetKey(key.value());
      } else if (context.allow_contextual) {
        ref.MarkContextual();
      } else {
        reporter()->Fail(ErrNameNotFound, ref.span(), components[0], library());
      }
      break;
    }
    case 2: {
      if (auto key = lookup.TryDecl(library(), components[0])) {
        ref.SetKey(key.value().Member(components[1]));
      } else if (auto dep_library = lookup.TryLibrary(components[0])) {
        if (auto key = lookup.MustDecl(dep_library, components[1])) {
          ref.SetKey(key.value());
        }
      } else {
        reporter()->Fail(ErrNameNotFound, ref.span(), components[0], library());
      }
      break;
    }
    default: {
      std::string long_library_name;
      size_t prev_size;
      for (auto it = components.begin(); it != components.end() - 1; ++it) {
        prev_size = long_library_name.size();
        if (it != components.begin())
          long_library_name.push_back('.');
        long_library_name.append(*it);
      }
      auto short_library_name = std::string_view(long_library_name.data(), prev_size);
      if (auto dep_library = lookup.TryLibrary(long_library_name)) {
        if (auto key = lookup.MustDecl(dep_library, components.back())) {
          ref.SetKey(key.value());
        }
      } else if (auto dep_library = lookup.TryLibrary(short_library_name)) {
        if (auto key = lookup.MustDecl(dep_library, components[components.size() - 2])) {
          ref.SetKey(key.value().Member(components.back()));
        }
      } else {
        reporter()->Fail(ErrUnknownDependentLibrary, ref.span(), long_library_name,
                         short_library_name);
      }
      break;
    }
  }
}

void ResolveStep::InsertReferenceEdges(const Reference& ref, Context context) {
  // Don't insert edges for a contextual reference, if parsing failed, or if the
  // reference is already resolved (see comment in ResolveStep::ParseReference
  // for details about when this happens).
  if (ref.state() == Reference::State::kContextual || ref.state() == Reference::State::kFailed ||
      ref.state() == Reference::State::kResolved) {
    return;
  }
  auto key = ref.key();
  // Only insert edges if the target is in the same platform.
  if (key.library->platform.value() != library()->platform.value()) {
    return;
  }
  // Note: key.library may is not necessarily library(), thus
  // key.library->declarations could be pre-decomposition or post-decomposition.
  // Although no branching is needed here, this is important to keep in mind.
  auto [begin, end] = key.library->declarations.all.equal_range(key.decl_name);
  for (auto it = begin; it != end; ++it) {
    Element* target = it->second;
    Element* enclosing = context.enclosing;
    // Don't insert a self-loop.
    if (target == enclosing) {
      continue;
    }
    // Only insert an edge if we have a chance of resolving to this target
    // post-decomposition (as opposed to one of the other same-named targets).
    if (VersionSet::Intersect(target->availability.set(), enclosing->availability.set())) {
      graph_[target].neighbors.insert(enclosing);
    }
  }
}

void ResolveStep::ResolveReference(Reference& ref, Context context) {
  auto initial_state = ref.state();
  auto checkpoint = reporter()->Checkpoint();
  switch (initial_state) {
    case Reference::State::kFailed:
    case Reference::State::kResolved:
      // Nothing to do, either failed parsing or already attempted resolving.
      return;
    case Reference::State::kContextual:
      ResolveContextualReference(ref, context);
      break;
    case Reference::State::kKey:
      ResolveKeyReference(ref, context);
      break;
    default:
      ZX_PANIC("unexpected reference state");
  }
  if (ref.state() == initial_state) {
    ZX_ASSERT_MSG(checkpoint.NumNewErrors() > 0, "should have reported an error");
    ref.MarkFailed();
  }
}

void ResolveStep::ResolveContextualReference(Reference& ref, Context context) {
  auto name = ref.contextual().name;
  auto subtype_enum = context.maybe_resource_subtype;
  if (!subtype_enum) {
    reporter()->Fail(ErrNameNotFound, ref.span(), name, library());
    return;
  }
  Lookup lookup(this, ref);
  auto member = lookup.TryMember(subtype_enum, name);
  if (!member) {
    reporter()->Fail(ErrNameNotFound, ref.span(), name, library());
    return;
  }
  ref.ResolveTo(Reference::Target(member, subtype_enum));
}

void ResolveStep::ResolveKeyReference(Reference& ref, Context context) {
  auto decl = LookupDeclByKey(ref, context);
  if (!decl) {
    return;
  }
  if (!ref.key().member_name) {
    ref.ResolveTo(Reference::Target(decl));
    return;
  }
  Lookup lookup(this, ref);
  auto member = lookup.MustMember(decl, ref.key().member_name.value());
  if (!member) {
    return;
  }
  ref.ResolveTo(Reference::Target(member, decl));
}

static std::vector<std::pair<VersionRange, SourceSpan>> BuildCandidatesInfo(
    std::multimap<std::string_view, Decl*>::const_iterator begin,
    std::multimap<std::string_view, Decl*>::const_iterator end) {
  std::vector<std::pair<VersionRange, SourceSpan>> info;
  for (auto it = begin; it != end; ++it) {
    auto decl = it->second;
    auto span = decl->GetNameSource();
    auto range = decl->availability.range().pair();
    if (range.first == Version::kLegacy)
      continue;
    if (!info.empty() && info.back().second == span &&
        info.back().first.pair().second == range.first) {
      info.back().first.pair().second = range.second;
    } else {
      info.emplace_back(VersionRange(range.first, range.second), span);
    }
  }
  return info;
}

Decl* ResolveStep::LookupDeclByKey(const Reference& ref, Context context) {
  auto key = ref.key();
  auto [begin, end] = key.library->declarations.all.equal_range(key.decl_name);
  ZX_ASSERT_MSG(begin != end, "key must exist");
  auto platform = key.library->platform.value();
  auto source_range = context.enclosing->availability.range();
  // Case #1: source and target libraries are versioned in the same platform.
  if (library()->platform == platform) {
    for (auto it = begin; it != end; ++it) {
      auto decl = it->second;
      if (auto overlap = VersionRange::Intersect(source_range, decl->availability.range())) {
        ZX_ASSERT_MSG(overlap.value() == source_range, "referencee must outlive referencer");
        return decl;
      }
    }
    reporter()->Fail(ErrNameNotFoundInVersionRange, ref.span(), key.decl_name, key.library,
                     source_range, BuildCandidatesInfo(begin, end));
    return nullptr;
  }
  // Case #2: source and target libraries are versioned in different platforms.
  auto version = version_selection()->Lookup(platform);
  for (auto it = begin; it != end; ++it) {
    auto decl = it->second;
    if (decl->availability.range().Contains(version)) {
      return decl;
    }
  }
  // TODO(https://fxbug.dev/42146818): Provide a nicer error message in the case where
  // a decl with that name does exist, but in a different version range.
  reporter()->Fail(ErrNameNotFoundInVersionRange, ref.span(), key.decl_name, key.library,
                   VersionRange(version, version.Successor()), BuildCandidatesInfo(begin, end));
  return nullptr;
}

void ResolveStep::ValidateReference(const Reference& ref, Context context) {
  if (ref.state() == Reference::State::kFailed) {
    return;
  }
  if (!ref.IsSynthetic() && ref.resolved().name().as_anonymous()) {
    reporter()->Fail(ErrAnonymousNameReference, ref.span(), ref.resolved().name());
  }

  auto source = context.enclosing;
  auto target = ref.resolved().element();
  auto source_deprecated = source->availability.is_deprecated();
  auto target_deprecated = target->availability.is_deprecated();

  // TODO(https://fxbug.dev/42052719): The check below is a stopgap solution to allow
  // @deprecated elements to reference @available(deprecated=...) elements. We
  // should solve this in a more principled way by layering the latter on the
  // former. For example, that would also ensure that the following works:
  //
  //     @deprecated
  //     type Foo = struct { member DeprecatedType; };
  //     @available(deprecated=1)
  //     alias DeprecatedType = bool;
  //
  // Whereas with the current stopgap you'd have to also add @deprecated on the
  // member itself.
  if (source->attributes->Get("deprecated")) {
    source_deprecated = true;
  }

  if (source_deprecated || !target_deprecated) {
    return;
  }

  auto& source_platform = library()->platform.value();
  auto& target_platform = ref.resolved().library()->platform.value();

  if (source_platform == target_platform) {
    reporter()->Fail(ErrInvalidReferenceToDeprecated, ref.span(), target,
                     source->availability.range(), source_platform, source);
  } else {
    reporter()->Fail(ErrInvalidReferenceToDeprecatedOtherPlatform, ref.span(), target,
                     target->availability.range(), target_platform, source,
                     source->availability.range(), source_platform);
  }
}

}  // namespace fidlc
