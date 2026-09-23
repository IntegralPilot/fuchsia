// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/flat_ast.h"

#include <zircon/assert.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/reporter.h"

namespace fidlc {

bool Element::IsDecl() const {
  switch (kind) {
    case Kind::kAlias:
    case Kind::kBits:
    case Kind::kBuiltin:
    case Kind::kConst:
    case Kind::kEnum:
    case Kind::kNewType:
    case Kind::kOverlay:
    case Kind::kProtocol:
    case Kind::kResource:
    case Kind::kService:
    case Kind::kStruct:
    case Kind::kTable:
    case Kind::kUnion:
      return true;
    case Kind::kBitsMember:
    case Kind::kEnumMember:
    case Kind::kLibrary:
    case Kind::kModifier:
    case Kind::kOverlayMember:
    case Kind::kProtocolCompose:
    case Kind::kProtocolMethod:
    case Kind::kResourceProperty:
    case Kind::kServiceMember:
    case Kind::kStructMember:
    case Kind::kTableMember:
    case Kind::kUnionMember:
      return false;
  }
}

Decl* Element::AsDecl() {
  ZX_ASSERT(IsDecl());
  return static_cast<Decl*>(this);
}

ModifierList* Element::GetModifiers() {
  switch (kind) {
    case Kind::kBits:
      return static_cast<Bits*>(this)->modifiers.get();
    case Kind::kEnum:
      return static_cast<Enum*>(this)->modifiers.get();
    case Kind::kOverlay:
      return static_cast<Overlay*>(this)->modifiers.get();
    case Kind::kProtocol:
      return static_cast<Protocol*>(this)->modifiers.get();
    case Kind::kStruct:
      return static_cast<Struct*>(this)->modifiers.get();
    case Kind::kTable:
      return static_cast<Table*>(this)->modifiers.get();
    case Kind::kUnion:
      return static_cast<Union*>(this)->modifiers.get();
    case Kind::kProtocolMethod:
      return static_cast<Protocol::Method*>(this)->modifiers.get();
    case Kind::kLibrary:
    case Kind::kModifier:
    case Kind::kAlias:
    case Kind::kBuiltin:
    case Kind::kConst:
    case Kind::kNewType:
    case Kind::kResource:
    case Kind::kService:
    case Kind::kBitsMember:
    case Kind::kEnumMember:
    case Kind::kOverlayMember:
    case Kind::kProtocolCompose:
    case Kind::kResourceProperty:
    case Kind::kServiceMember:
    case Kind::kStructMember:
    case Kind::kTableMember:
    case Kind::kUnionMember:
      return nullptr;
  }
}

void Element::ForEachModifier(const fit::function<void(Modifier*)>& fn) {
  if (auto modifiers = GetModifiers()) {
    for (auto& modifier : modifiers->modifiers)
      fn(modifier.get());
  }
}

bool Element::IsAnonymousLayout() const {
  switch (kind) {
    case Element::Kind::kBits:
    case Element::Kind::kEnum:
    case Element::Kind::kStruct:
    case Element::Kind::kTable:
    case Element::Kind::kUnion:
    case Element::Kind::kOverlay:
      return static_cast<const Decl*>(this)->name.as_anonymous() != nullptr;
    default:
      return false;
  }
}

std::string_view Element::GetName() const {
  switch (kind) {
    case Kind::kLibrary:
      return static_cast<const Library*>(this)->name;
    case Kind::kModifier:
      return static_cast<const Modifier*>(this)->name.data();
    case Kind::kBits:
    case Kind::kBuiltin:
    case Kind::kConst:
    case Kind::kEnum:
    case Kind::kProtocol:
    case Kind::kResource:
    case Kind::kService:
    case Kind::kStruct:
    case Kind::kTable:
    case Kind::kAlias:
    case Kind::kUnion:
    case Kind::kNewType:
    case Kind::kOverlay:
      return static_cast<const Decl*>(this)->name.decl_name();
    case Kind::kBitsMember:
      return static_cast<const Bits::Member*>(this)->name.data();
    case Kind::kEnumMember:
      return static_cast<const Enum::Member*>(this)->name.data();
    case Kind::kProtocolCompose:
      return static_cast<const Protocol::ComposedProtocol*>(this)->reference.span().data();
    case Kind::kProtocolMethod:
      return static_cast<const Protocol::Method*>(this)->name.data();
    case Kind::kResourceProperty:
      return static_cast<const Resource::Property*>(this)->name.data();
    case Kind::kServiceMember:
      return static_cast<const Service::Member*>(this)->name.data();
    case Kind::kStructMember:
      return static_cast<const Struct::Member*>(this)->name.data();
    case Kind::kTableMember:
      return static_cast<const Table::Member*>(this)->name.data();
    case Kind::kUnionMember:
      return static_cast<const Union::Member*>(this)->name.data();
    case Kind::kOverlayMember:
      return static_cast<const Overlay::Member*>(this)->name.data();
  }
}

SourceSpan Element::GetNameSource() const {
  switch (kind) {
    case Kind::kLibrary:
      ZX_PANIC("should not call GetNameSource() on a library element");
    case Kind::kModifier:
      return static_cast<const Modifier*>(this)->name;
    case Kind::kBits:
    case Kind::kBuiltin:
    case Kind::kConst:
    case Kind::kEnum:
    case Kind::kProtocol:
    case Kind::kResource:
    case Kind::kService:
    case Kind::kStruct:
    case Kind::kTable:
    case Kind::kAlias:
    case Kind::kUnion:
    case Kind::kNewType:
    case Kind::kOverlay:
      return static_cast<const Decl*>(this)->name.span().value();
    case Kind::kBitsMember:
      return static_cast<const Bits::Member*>(this)->name;
    case Kind::kEnumMember:
      return static_cast<const Enum::Member*>(this)->name;
    case Kind::kProtocolCompose:
      return static_cast<const Protocol::ComposedProtocol*>(this)->reference.span();
    case Kind::kProtocolMethod:
      return static_cast<const Protocol::Method*>(this)->name;
    case Kind::kResourceProperty:
      return static_cast<const Resource::Property*>(this)->name;
    case Kind::kServiceMember:
      return static_cast<const Service::Member*>(this)->name;
    case Kind::kStructMember:
      return static_cast<const Struct::Member*>(this)->name;
    case Kind::kTableMember:
      return static_cast<const Table::Member*>(this)->name;
    case Kind::kUnionMember:
      return static_cast<const Union::Member*>(this)->name;
    case Kind::kOverlayMember:
      return static_cast<const Overlay::Member*>(this)->name;
  }
}

std::optional<AbiKind> Element::abi_kind() const {
  switch (kind) {
    case Element::Kind::kBitsMember:
    case Element::Kind::kEnumMember:
      return AbiKind::kValue;
    case Element::Kind::kStructMember:
      return AbiKind::kOffset;
    case Element::Kind::kTableMember:
    case Element::Kind::kUnionMember:
    case Element::Kind::kOverlayMember:
      return AbiKind::kOrdinal;
    case Element::Kind::kProtocolMethod:
      return AbiKind::kSelector;
    default:
      return std::nullopt;
  }
}

std::optional<AbiValue> Element::abi_value() const {
  switch (kind) {
    case Element::Kind::kBitsMember:
      return static_cast<const Bits::Member*>(this)->value->Value().AsUnsigned().value();
    case Element::Kind::kEnumMember: {
      auto& value = static_cast<const Enum::Member*>(this)->value->Value();
      if (auto number = value.AsUnsigned())
        return number.value();
      return value.AsSigned().value();
    }
    case Element::Kind::kProtocolMethod:
      return std::string_view(static_cast<const Protocol::Method*>(this)->selector);
    case Element::Kind::kStructMember:
      return static_cast<uint64_t>(static_cast<const Struct::Member*>(this)->field_shape.offset);
    case Element::Kind::kTableMember:
      return static_cast<const Table::Member*>(this)->ordinal->value;
    case Element::Kind::kUnionMember:
      return static_cast<const Union::Member*>(this)->ordinal->value;
    case Element::Kind::kOverlayMember:
      return static_cast<const Overlay::Member*>(this)->ordinal->value;
    default:
      return std::nullopt;
  }
}

bool Builtin::IsInternal() const {
  switch (id) {
    case Identity::kBool:
    case Identity::kInt8:
    case Identity::kInt16:
    case Identity::kInt32:
    case Identity::kInt64:
    case Identity::kUint8:
    case Identity::kZxUchar:
    case Identity::kUint16:
    case Identity::kUint32:
    case Identity::kUint64:
    case Identity::kZxUsize64:
    case Identity::kZxUintptr64:
    case Identity::kFloat32:
    case Identity::kFloat64:
    case Identity::kString:
    case Identity::kBox:
    case Identity::kArray:
    case Identity::kStringArray:
    case Identity::kVector:
    case Identity::kZxExperimentalPointer:
    case Identity::kClientEnd:
    case Identity::kServerEnd:
    case Identity::kByte:
    case Identity::kOptional:
    case Identity::kMax:
    case Identity::kNext:
    case Identity::kHead:
      return false;
    case Identity::kFrameworkErr:
      return true;
  }
}

Resource::Property* Resource::LookupProperty(std::string_view name) {
  for (Property& property : properties) {
    if (property.name.data() == name.data()) {
      return &property;
    }
  }
  return nullptr;
}

Dependencies::RegisterResult Dependencies::Register(
    const SourceSpan& span, std::string_view filename, Library* dep_library,
    const std::unique_ptr<RawIdentifier>& maybe_alias) {
  refs_.push_back(std::make_unique<LibraryRef>(span, dep_library));
  LibraryRef* ref = refs_.back().get();

  auto name = maybe_alias ? maybe_alias->span().data() : dep_library->name;
  auto iter = by_filename_.find(filename);
  if (iter == by_filename_.end()) {
    iter = by_filename_.emplace(filename, std::make_unique<PerFile>()).first;
  }
  PerFile& per_file = *iter->second;
  if (!per_file.libraries.insert(dep_library).second) {
    return RegisterResult::kDuplicate;
  }
  if (!per_file.refs.emplace(name, ref).second) {
    return RegisterResult::kCollision;
  }
  dependencies_aggregate_.insert(dep_library);
  return RegisterResult::kSuccess;
}

bool Dependencies::Contains(std::string_view filename, std::string_view library_name) {
  const auto iter = by_filename_.find(filename);
  if (iter == by_filename_.end()) {
    return false;
  }
  const PerFile& per_file = *iter->second;
  return per_file.refs.find(library_name) != per_file.refs.end();
}

Library* Dependencies::LookupAndMarkUsed(std::string_view filename,
                                         std::string_view library_name) const {
  auto iter1 = by_filename_.find(filename);
  if (iter1 == by_filename_.end()) {
    return nullptr;
  }

  auto iter2 = iter1->second->refs.find(library_name);
  if (iter2 == iter1->second->refs.end()) {
    return nullptr;
  }

  auto ref = iter2->second;
  ref->used = true;
  return ref->library;
}

void Dependencies::VerifyAllDependenciesWereUsed(const Library* for_library, Reporter* reporter) {
  for (const auto& [filename, per_file] : by_filename_) {
    for (const auto& [name, ref] : per_file->refs) {
      if (!ref->used) {
        reporter->Fail(ErrUnusedImport, ref->span, for_library, ref->library);
      }
    }
  }
}

// static
std::unique_ptr<Library> Library::CreateRootLibrary() {
  // TODO(https://fxbug.dev/42146818): Because this library doesn't get compiled, we have
  // to simulate what AvailabilityStep would do (set the platform, inherit the
  // availabilities). Perhaps we could make the root library less special and
  // compile it as well. That would require addressing circularity issues.
  auto library = std::make_unique<Library>();
  library->name = "fidl";
  library->platform = Platform::Unversioned();
  library->availability.Init({.added = Version::kHead});
  library->availability.Inherit(Availability::Unbounded());
  auto insert = [&](const char* name, Builtin::Identity id) {
    auto decl = std::make_unique<Builtin>(id, Name::CreateIntrinsic(library.get(), name));
    decl->availability.Init({});
    decl->availability.Inherit(library->availability);
    library->declarations.Insert(std::move(decl));
  };
  // An assertion in Declarations::Insert ensures that these insertions
  // stays in sync with the order of Builtin::Identity.
  insert("bool", Builtin::Identity::kBool);
  insert("int8", Builtin::Identity::kInt8);
  insert("int16", Builtin::Identity::kInt16);
  insert("int32", Builtin::Identity::kInt32);
  insert("int64", Builtin::Identity::kInt64);
  insert("uint8", Builtin::Identity::kUint8);
  insert("uchar", Builtin::Identity::kZxUchar);
  insert("uint16", Builtin::Identity::kUint16);
  insert("uint32", Builtin::Identity::kUint32);
  insert("uint64", Builtin::Identity::kUint64);
  insert("usize64", Builtin::Identity::kZxUsize64);
  insert("uintptr64", Builtin::Identity::kZxUintptr64);
  insert("float32", Builtin::Identity::kFloat32);
  insert("float64", Builtin::Identity::kFloat64);
  insert("string", Builtin::Identity::kString);
  insert("box", Builtin::Identity::kBox);
  insert("array", Builtin::Identity::kArray);
  insert("string_array", Builtin::Identity::kStringArray);
  insert("vector", Builtin::Identity::kVector);
  insert("experimental_pointer", Builtin::Identity::kZxExperimentalPointer);
  insert("client_end", Builtin::Identity::kClientEnd);
  insert("server_end", Builtin::Identity::kServerEnd);
  insert("byte", Builtin::Identity::kByte);
  insert("FrameworkErr", Builtin::Identity::kFrameworkErr);
  insert("optional", Builtin::Identity::kOptional);
  insert("MAX", Builtin::Identity::kMax);
  insert("NEXT", Builtin::Identity::kNext);
  insert("HEAD", Builtin::Identity::kHead);

  // Simulate narrowing availabilities to maintain the invariant that they
  // always reach kNarrowed (except for the availability of `library`).
  library->ForEachElement([](Element* element) {
    element->availability.Narrow(VersionRange(Version::kHead, Version::kPosInf));
  });

  return library;
}

void Library::ForEachElement(const fit::function<void(Element*)>& fn) {
  fn(this);
  for (auto& [name, decl] : declarations.all) {
    fn(decl);
    decl->ForEachEdge([&](Element* parent, Element* child) { fn(child); });
  }
}

void Decl::ForEachMember(const fit::function<void(Element*)>& fn) {
  switch (kind) {
    case Decl::Kind::kBuiltin:
    case Decl::Kind::kConst:
    case Decl::Kind::kAlias:
    case Decl::Kind::kNewType:
      break;
    case Decl::Kind::kBits:
      for (auto& member : static_cast<Bits*>(this)->members) {
        fn(&member);
      }
      break;
    case Decl::Kind::kEnum:
      for (auto& member : static_cast<Enum*>(this)->members) {
        fn(&member);
      }
      break;
    case Decl::Kind::kProtocol:
      for (auto& composed_protocol : static_cast<Protocol*>(this)->composed_protocols) {
        fn(&composed_protocol);
      }
      for (auto& method : static_cast<Protocol*>(this)->methods) {
        fn(&method);
      }
      break;
    case Decl::Kind::kResource:
      for (auto& member : static_cast<Resource*>(this)->properties) {
        fn(&member);
      }
      break;
    case Decl::Kind::kService:
      for (auto& member : static_cast<Service*>(this)->members) {
        fn(&member);
      }
      break;
    case Decl::Kind::kStruct:
      for (auto& member : static_cast<Struct*>(this)->members) {
        fn(&member);
      }
      break;
    case Decl::Kind::kTable:
      for (auto& member : static_cast<Table*>(this)->members) {
        fn(&member);
      }
      break;
    case Decl::Kind::kUnion:
      for (auto& member : static_cast<Union*>(this)->members) {
        fn(&member);
      }
      break;
    case Decl::Kind::kOverlay:
      for (auto& member : static_cast<Overlay*>(this)->members) {
        fn(&member);
      }
      break;
  }  // switch
}

void Decl::ForEachEdge(const fit::function<void(Element* parent, Element* child)>& fn) {
  ForEachModifier([&](Modifier* modifier) { fn(this, modifier); });
  ForEachMember([&](Element* member) {
    fn(this, member);
    member->ForEachModifier([&](Modifier* modifier) { fn(member, modifier); });
  });
}

template <typename T>
static T* StoreDecl(std::unique_ptr<Decl> decl, std::multimap<std::string_view, Decl*>* all,
                    std::vector<std::unique_ptr<T>>* declarations) {
  auto ptr = static_cast<T*>(decl.release());
  all->emplace(ptr->name.decl_name(), ptr);
  declarations->emplace_back(ptr);
  return ptr;
}

Decl* Library::Declarations::Insert(std::unique_ptr<Decl> decl) {
  switch (decl->kind) {
    case Decl::Kind::kBuiltin: {
      auto index = static_cast<size_t>(static_cast<Builtin*>(decl.get())->id);
      ZX_ASSERT_MSG(index == builtins.size(), "inserted builtin out of order");
      return StoreDecl(std::move(decl), &all, &builtins);
    }
    case Decl::Kind::kBits:
      return StoreDecl(std::move(decl), &all, &bits);
    case Decl::Kind::kConst:
      return StoreDecl(std::move(decl), &all, &consts);
    case Decl::Kind::kEnum:
      return StoreDecl(std::move(decl), &all, &enums);
    case Decl::Kind::kProtocol:
      return StoreDecl(std::move(decl), &all, &protocols);
    case Decl::Kind::kResource:
      return StoreDecl(std::move(decl), &all, &resources);
    case Decl::Kind::kService:
      return StoreDecl(std::move(decl), &all, &services);
    case Decl::Kind::kStruct:
      return StoreDecl(std::move(decl), &all, &structs);
    case Decl::Kind::kTable:
      return StoreDecl(std::move(decl), &all, &tables);
    case Decl::Kind::kAlias:
      return StoreDecl(std::move(decl), &all, &aliases);
    case Decl::Kind::kUnion:
      return StoreDecl(std::move(decl), &all, &unions);
    case Decl::Kind::kOverlay:
      return StoreDecl(std::move(decl), &all, &overlays);
    case Decl::Kind::kNewType:
      return StoreDecl(std::move(decl), &all, &new_types);
  }
}

Builtin* Library::Declarations::LookupBuiltin(Builtin::Identity id) const {
  auto index = static_cast<size_t>(id);
  ZX_ASSERT_MSG(index < builtins.size(), "builtin id out of range");
  auto builtin = builtins[index].get();
  ZX_ASSERT_MSG(builtin->id == id, "builtin's id does not match index");
  return builtin;
}

std::unique_ptr<Modifier> Modifier::Clone() const {
  return std::make_unique<Modifier>(attributes->Clone(), name, value);
}

std::unique_ptr<TypeConstructor> TypeConstructor::Clone() const {
  return std::make_unique<TypeConstructor>(span, layout, parameters->Clone(), constraints->Clone());
}

std::unique_ptr<LayoutParameterList> LayoutParameterList::Clone() const {
  return std::make_unique<LayoutParameterList>(MapClone(items), span);
}

std::unique_ptr<TypeConstraints> TypeConstraints::Clone() const {
  return std::make_unique<TypeConstraints>(MapClone(items), span);
}

TypeConstructor* LiteralLayoutParameter::AsTypeCtor() const { return nullptr; }
TypeConstructor* TypeLayoutParameter::AsTypeCtor() const { return type_ctor.get(); }
TypeConstructor* IdentifierLayoutParameter::AsTypeCtor() const { return as_type_ctor.get(); }

Constant* LiteralLayoutParameter::AsConstant() const { return literal.get(); }
Constant* TypeLayoutParameter::AsConstant() const { return nullptr; }
Constant* IdentifierLayoutParameter::AsConstant() const { return as_constant.get(); }

std::unique_ptr<LayoutParameter> LiteralLayoutParameter::Clone() const {
  return std::make_unique<LiteralLayoutParameter>(literal->CloneLiteralConstant(), span);
}

std::unique_ptr<LayoutParameter> TypeLayoutParameter::Clone() const {
  return std::make_unique<TypeLayoutParameter>(type_ctor->Clone(), span);
}

std::unique_ptr<LayoutParameter> IdentifierLayoutParameter::Clone() const {
  ZX_ASSERT_MSG(!(as_constant || as_type_ctor), "Clone() is not allowed after Disambiguate()");
  return std::make_unique<IdentifierLayoutParameter>(reference, span);
}

void IdentifierLayoutParameter::Disambiguate() {
  switch (reference.resolved().element()->kind) {
    case Element::Kind::kConst:
    case Element::Kind::kBitsMember:
    case Element::Kind::kEnumMember: {
      as_constant = std::make_unique<IdentifierConstant>(reference, span);
      break;
    }
    default: {
      as_type_ctor = std::make_unique<TypeConstructor>(span, reference,
                                                       std::make_unique<LayoutParameterList>(),
                                                       std::make_unique<TypeConstraints>());
      break;
    }
  }
}

std::unique_ptr<Decl> Decl::Split(VersionRange range) const {
  auto decl = SplitImpl(range);
  decl->availability = availability;
  decl->availability.Narrow(range);
  return decl;
}

std::unique_ptr<ModifierList> ModifierList::Split(VersionRange range) const {
  std::vector<std::unique_ptr<Modifier>> result;
  for (auto& modifier : modifiers) {
    if (VersionSet::Intersect(VersionSet(range), modifier->availability.set())) {
      result.push_back(modifier->Clone());
      result.back()->availability = modifier->availability;
      result.back()->availability.Narrow(range);
    }
  }
  return std::make_unique<ModifierList>(std::move(result));
}

// For a decl member type T that has a Clone() method, takes a vector<T> and
// returns a vector of copies filtered to only include those that intersect with
// the given range, and narrows their availabilities to that range.
template <typename T>
static std::vector<T> FilterMembers(const std::vector<T>& all, VersionRange range) {
  std::vector<T> result;
  for (auto& child : all) {
    if (VersionSet::Intersect(VersionSet(range), child.availability.set())) {
      // Methods are a special case because we need to filter their modifiers.
      if constexpr (std::is_same_v<T, Protocol::Method>) {
        result.push_back(child.Clone(range));
      } else {
        result.push_back(child.Clone());
      }
      result.back().availability = child.availability;
      result.back().availability.Narrow(range);
    }
  }
  return result;
}

// Like FilterMembers, but for members with ordinals (table and union members).
// In addition to filtering, sorts the result by ordinal.
template <typename T>
static std::vector<T> FilterOrdinaledMembers(const std::vector<T>& all, VersionRange range) {
  std::vector<T> result = FilterMembers(all, range);
  std::sort(result.begin(), result.end(),
            [](const T& lhs, const T& rhs) { return lhs.ordinal->value < rhs.ordinal->value; });
  return result;
}

std::unique_ptr<Decl> Builtin::SplitImpl(VersionRange range) const {
  ZX_PANIC("splitting builtins not allowed");
}

std::unique_ptr<Decl> Const::SplitImpl(VersionRange range) const {
  return std::make_unique<Const>(attributes->Clone(), name, type_ctor->Clone(), value->Clone());
}

std::unique_ptr<Decl> Enum::SplitImpl(VersionRange range) const {
  return std::make_unique<Enum>(attributes->Clone(), modifiers->Split(range), name,
                                subtype_ctor->Clone(), FilterMembers(members, range));
}

std::unique_ptr<Decl> Bits::SplitImpl(VersionRange range) const {
  return std::make_unique<Bits>(attributes->Clone(), modifiers->Split(range), name,
                                subtype_ctor->Clone(), FilterMembers(members, range));
}

std::unique_ptr<Decl> Service::SplitImpl(VersionRange range) const {
  return std::make_unique<Service>(attributes->Clone(), name, FilterMembers(members, range));
}

std::unique_ptr<Decl> Struct::SplitImpl(VersionRange range) const {
  return std::make_unique<Struct>(attributes->Clone(), modifiers->Split(range), name,
                                  FilterMembers(members, range));
}

std::unique_ptr<Decl> Table::SplitImpl(VersionRange range) const {
  return std::make_unique<Table>(attributes->Clone(), modifiers->Split(range), name,
                                 FilterOrdinaledMembers(members, range));
}

std::unique_ptr<Decl> Union::SplitImpl(VersionRange range) const {
  return std::make_unique<Union>(attributes->Clone(), modifiers->Split(range), name,
                                 FilterOrdinaledMembers(members, range));
}

std::unique_ptr<Decl> Overlay::SplitImpl(VersionRange range) const {
  return std::make_unique<Overlay>(attributes->Clone(), modifiers->Split(range), name,
                                   FilterOrdinaledMembers(members, range));
}

std::unique_ptr<Decl> Protocol::SplitImpl(VersionRange range) const {
  return std::make_unique<Protocol>(attributes->Clone(), modifiers->Split(range), name,
                                    FilterMembers(composed_protocols, range),
                                    FilterMembers(methods, range));
}

std::unique_ptr<Decl> Resource::SplitImpl(VersionRange range) const {
  return std::make_unique<Resource>(attributes->Clone(), name, subtype_ctor->Clone(),
                                    FilterMembers(properties, range));
}

std::unique_ptr<Decl> Alias::SplitImpl(VersionRange range) const {
  return std::make_unique<Alias>(attributes->Clone(), name, partial_type_ctor->Clone());
}

std::unique_ptr<Decl> NewType::SplitImpl(VersionRange range) const {
  return std::make_unique<NewType>(attributes->Clone(), name, type_ctor->Clone());
}

Enum::Member Enum::Member::Clone() const {
  return Member(name, value->Clone(), attributes->Clone());
}

Bits::Member Bits::Member::Clone() const {
  return Member(name, value->Clone(), attributes->Clone());
}

Service::Member Service::Member::Clone() const {
  return Member(type_ctor->Clone(), name, attributes->Clone());
}

Struct::Member Struct::Member::Clone() const {
  return Member(type_ctor->Clone(), name,
                maybe_default_value ? maybe_default_value->Clone() : nullptr, attributes->Clone());
}

Table::Member Table::Member::Clone() const {
  return Member(ordinal, type_ctor->Clone(), name, attributes->Clone());
}

Union::Member Union::Member::Clone() const {
  return Member(ordinal, type_ctor->Clone(), name, attributes->Clone());
}

Overlay::Member Overlay::Member::Clone() const {
  return Member(ordinal, type_ctor->Clone(), name, attributes->Clone());
}

Protocol::Method Protocol::Method::Clone(VersionRange range) const {
  // We don't copy maybe_result_union because it is an unowned AST node pointer.
  // The CompileStep will make it point to the new union in the decomposed AST.
  return Method(attributes->Clone(), modifiers->Split(range), kind, name,
                maybe_request ? maybe_request->Clone() : nullptr,
                maybe_response ? maybe_response->Clone() : nullptr, /*maybe_result_union=*/nullptr,
                has_error);
}

Protocol::ComposedProtocol Protocol::ComposedProtocol::Clone() const {
  return ComposedProtocol(attributes->Clone(), reference);
}

Resource::Property Resource::Property::Clone() const {
  return Property(type_ctor->Clone(), name, attributes->Clone());
}

}  // namespace fidlc
