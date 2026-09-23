// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_LOAD_MODULE_H_
#define LIB_LD_LOAD_MODULE_H_

#include <lib/elfldltl/resolve.h>
#include <lib/elfldltl/tls-layout.h>

#include <cassert>
#include <compare>
#include <functional>

#include "decoded-module.h"
#include "tls.h"

namespace ld {

constexpr elfldltl::ResolverPolicy kResolverPolicy = elfldltl::ResolverPolicy::kStrongOverWeak;

// Forward declaration for a helper class defined below.
// See the name_ref() and soname_ref() methods in ld::LoadModule, below.
template <class LoadModule>
class LoadModuleRef;

// The ld::LoadModule template class provides a base class for a
// information about a given ELF file's use in a particular dynamic linking
// scenario.  This includes things like its name and runtime load bias.
//
// The template parameter can either be a ld::DecodedModule<...> instantiation
// or subclass of one, or it can be some pointer-like type that can be
// dereferenced with `*` to get to one.  That can be a plain pointer, a pointer
// to `const`, a smart pointer type of some kind, or even something like
// `std::optional`.
//
// This provides proxy methods for read-only access to the object derived from
// ld::DecodedModule.  The ld::LoadModule object itself holds little more than
// the module name.  A mutable decoded module object can be modified via the
// `decoded()` accessor.
template <typename DecodedStorage>
class LoadModule {
 private:
  static_assert(!std::is_const_v<DecodedStorage>);

  // Determine whether the template parameter is derived from an instantiation
  // of ld::DecodedModule<...>.  If not, it must be some pointer-like type.
  static constexpr bool kDecodedDirect = []() -> bool {
    if constexpr (std::is_class_v<DecodedStorage>) {
      return std::is_base_of_v<DecodedModuleBase, DecodedStorage>;
    }
    return false;
  }();

  // This is a static method so it can easily be called from both const and
  // non-const overloads of decoded(), below, where Storage will be const
  // DecodedStorage or DecodedStorage respectively.  It's also used to deduce
  // the underlying Decoded type, whether direct or pointed-to.
  template <class Storage>
  static constexpr auto& GetDecodedRef(Storage& storage) {
    if constexpr (kDecodedDirect) {
      return storage;
    } else {
      return *storage;
    }
  }

 public:
  // In the simple case, this will be the same as the template parameter.  But
  // if the template parameter is pointer-like, this is the pointed-to type,
  // which might be const.
  using Decoded = std::remove_reference_t<decltype(GetDecodedRef(std::declval<DecodedStorage&>()))>;

  // For convenience alias all the interesting types from Decoded.
  using Elf = Decoded::Elf;
  using Addr = Decoded::Addr;
  using size_type = Decoded::size_type;
  using Phdr = Decoded::Phdr;
  using Sym = Decoded::Sym;
  using Module = Decoded::Module;
  using TlsModule = Decoded::TlsModule;
  using LoadInfo = Decoded::LoadInfo;
  using RelocationInfo = Decoded::RelocationInfo;
  using Soname = Decoded::Soname;
  using LookupResult = Decoded::LookupResult;

  static constexpr bool kMutableDecoded = !std::is_const_v<Decoded>;

  // This is returned by the name_ref() and soname_ref() methods.
  using Ref = LoadModuleRef<LoadModule>;

  constexpr LoadModule() = default;

  constexpr LoadModule(const LoadModule&) = delete;

  constexpr LoadModule(LoadModule&&) noexcept = default;

  // The LoadModule is initially constructed just with a name, which is what
  // will eventually appear in Module::link_map::name.  This is the name by
  // which the module is initially found (in the filesystem or whatever).
  // When the object has a DT_SONAME (Module::soname), this is usually the
  // same; but nothing guarantees that.
  constexpr explicit LoadModule(std::string_view name) : name_(name) {}
  constexpr explicit LoadModule(const Soname& name) : name_(name) {}

  constexpr LoadModule& operator=(LoadModule&&) noexcept = default;

  constexpr const Soname& name() const { return name_; }

  constexpr void set_name(const Soname& name) {
    name_ = name;
    SetAbiName();
  }
  constexpr void set_name(std::string_view name) {
    name_ = name;
    SetAbiName();
  }

  // If the template parameter is pointer-like, then HasDecoded() is initially
  // false and this must be used to install a pointer.
  constexpr void set_decoded(DecodedStorage decoded) {
    decoded_ = std::move(decoded);
    assert(HasDecoded());
  }

  // For convenient container searches, equality comparison against a (hashed)
  // name checks both name fields.  An unloaded module only has a load name.
  // A loaded module may also have a SONAME.
  constexpr bool operator==(const Soname& name) const {
    return name == name_ || (HasModule() && name == this->module().soname);
  }

  // This returns an object that can be used like a LoadModule* pointing at
  // this, but is suitable for use in a container like std::unordered_set or
  // fbl::HashTable keyed by name().
  Ref name_ref() const { return {this, &LoadModule::name}; }

  // This returns an object that can be used like a LoadModule* pointing at
  // this, but is suitable for use in a container like std::unordered_set or
  // fbl::HashTable keyed by soname().
  Ref soname_ref() const { return {this, &LoadModule::soname}; }

  // This returns the offset from the thread pointer to this module's static
  // TLS block if it has one.  The value is assigned by AssignStaticTls, below.
  // This method should not be called unless AssignStaticTls has been called.
  constexpr size_type static_tls_bias() const { return static_tls_bias_; }

  // Use ths TlsLayout object to assign a static TLS offset for this module's
  // PT_TLS segment, if it has one.  SetTls() has already been called if it
  // will be.  This returns false if there was no PT_TLS segment.  If it
  // returns true, then the .tls_module() and .static_tls_bias() should go into
  // the Abi array fields at the the index TLS module ID - 1.  If Decoded is
  // const, then the module ID is not available directly in this class and a
  // subclass tls_module_id() definition must be used to fetch it.
  template <elfldltl::ElfMachine Machine = elfldltl::ElfMachine::kNative, size_type RedZone = 0>
  constexpr bool AssignStaticTls(elfldltl::TlsLayout<Elf>& tls_layout) {
    if (!HasModule()) [[unlikely]] {
      return false;
    }

    if (module().tls_modid == 0) {
      return false;
    }

    // These correspond to the p_memsz and p_align of the PT_TLS.
    const size_type memsz = tls_module().tls_size();
    const size_type align = tls_module().tls_alignment;

    // Save the offset for use in resolving IE relocations.
    static_tls_bias_ = tls_layout.template Assign<Machine, RedZone>(memsz, align);

    return true;
  }

  // This returns true if it's safe to call the decoded() method.  This is
  // always true if the template parameter is a DecodedModule type itself.
  // If it's instead some pointer-like type, then this can return false
  // before the pointer has been installed.
  // TODO(https://fxbug.dev/326524302): There is not yet any way to install it!
  constexpr bool HasDecoded() const {
    if constexpr (kDecodedDirect) {
      return true;
    } else {
      return static_cast<bool>(decoded_);
    }
  }

  // Access the underlying DecodedModule object.  This must not be called if
  // HasDecoded() returns false.  There are both const and mutable overloads
  // for this.  But note that Decoded might be a const type itself when the
  // template parameter is a pointer-like type, in which case decoded() always
  // returns a const reference even when called on a mutable LoadModule.
  constexpr Decoded& decoded() { return GetDecodedRef(decoded_); }
  constexpr const Decoded& decoded() const { return GetDecodedRef(decoded_); }

  // When Decoded is mutable and uses AbiModuleInline::kYes, EmplaceModule(..)
  // just constructs Module{...}).
  template <typename... Args, bool CanEmplace = Decoded::kModuleInline && kMutableDecoded,
            typename = std::enable_if_t<CanEmplace>>
  constexpr void EmplaceModule(uint32_t modid, Args&&... args) {
    decoded().EmplaceModule(modid, std::forward<Args>(args)...);
    SetAbiName();
  }

  // When Decoded is mutable and uses AbiModuleInline::kNo, then calling
  // NewModule(a..., c...)  does new (a...) Module{c...}.  The last argument in
  // a... must be a fbl::AllocChecker that indicates whether `new` succeeded.
  template <typename... Args, bool CanNew = !Decoded::kModuleInline && kMutableDecoded,
            typename = std::enable_if_t<CanNew>>
  constexpr void NewModule(uint32_t modid, Args&&... args) {
    decoded().NewModule(modid, std::forward<Args>(args)...);
    SetAbiName();
  }

  // The following methods are simple proxies for the same method on decoded().
  // Note only const overloads are provided here, for reading information from
  // the DecodedModule to use in loading and dynamic linking.  For filling in
  // the information from the file in the first place, use decoded() for the
  // mutable reference.

  constexpr bool HasModule() const { return HasDecoded() && decoded().HasModule(); }

  constexpr const Module& module() const { return decoded().module(); }

  constexpr const Soname& soname() const { return decoded().soname(); }

  constexpr const LoadInfo& load_info() const { return decoded().load_info(); }

  template <bool RI = Decoded::kRelocInfo, typename = std::enable_if_t<RI>>
  constexpr const RelocationInfo& reloc_info() const {
    return decoded().reloc_info();
  }

  constexpr const TlsModule& tls_module() const { return decoded().tls_module(); }

  // The following methods satisfy the Module template API for use with
  // elfldltl::ResolverDefinition (see <lib/elfldltl/resolve.h>).

  constexpr const auto& symbol_info() const { return module().symbols; }

  // This is only provided when Decoded is mutable.  In that case, after
  // decoding and choosing load address, module() will be updated via
  // ld::SetModuleVaddrBounds.  In other cases, a derived class must implement
  // load_bias() itself.
  template <bool M = kMutableDecoded, typename = std::enable_if_t<M>>
  constexpr size_type load_bias() const {
    return module().link_map.addr;
  }

  constexpr bool uses_static_tls() const { return ld::ModuleUsesStaticTls(module()); }

  // This is only provided when Decoded is mutable.  In that case, SetTls
  // installs the actual TLS module ID and this will return it.  When Decoded
  // is const, then SetTls will be called with the placeholder TLS module ID 1
  // just so that it's nonzero to indicate the module has a PT_TLS at all.  The
  // derived class must define tls_module_id() itself to return the real TLS
  // module ID it assigns and stores elsewhere.
  template <bool M = kMutableDecoded, typename = std::enable_if_t<M>>
  constexpr size_type tls_module_id() const {
    return module().tls_modid;
  }

  // This can be overridden in a subclass for custom semantics.
  constexpr LookupResult Lookup(auto& diag, elfldltl::SymbolName& name) const {
    return decoded().Lookup(diag, name);
  }

 protected:
  constexpr auto& decoded_storage() { return decoded_; }
  constexpr auto& decoded_storage() const { return decoded_; }

  constexpr void SetAbiName() {
    if constexpr (kMutableDecoded) {
      if (HasDecoded()) {
        decoded().SetAbiName(name_);
      }
    }
  }

 private:
  DecodedStorage decoded_{};
  Soname name_;
  size_type static_tls_bias_ = 0;
};

// This object is returned by the name_ref() and soname_ref() methods of
// ld::LoadModule.  It's meant to be used in containers keyed by the module
// name.  Both name_ref() and soname_ref() of the same module should be
// inserted into the container so that it can be looked up by either name.
//
// The object can be used like LoadModule* or a similar smart pointer type.
// It also has methods for accessing the module's name (with hash), which is
// either LoadModule::name() or LoadModule::soname(); an operator== that takes
// a Soname (name with hash) object; and it supports both the std::hash and
// <fbl/intrusive_container_utils.h> APIs.  This makes it easy to use in
// containers like std::unordered_set or fbl::HashTable.
template <class LoadModule>
class LoadModuleRef {
 public:
  using Soname = LoadModule::Soname;

  constexpr LoadModuleRef() = default;

  constexpr LoadModuleRef(const LoadModuleRef&) = default;

  constexpr LoadModuleRef(LoadModule* module, const Soname& (LoadModule::*name)())
      : module_(module), name_(name) {}

  constexpr LoadModuleRef& operator=(const LoadModuleRef&) = default;

  constexpr LoadModule& operator*() const { return *module_; }

  constexpr LoadModule* operator->() const { return module_; }

  constexpr LoadModule* get() const { return module_; }

  constexpr const Soname& name() const { return module_->*name_(); }

  constexpr uint32_t hash() const { return name().hash(); }

  // This is the API contract used by fbl::HashTable.
  static const Soname& GetKey(const LoadModuleRef& ref) { return ref.name(); }
  static constexpr uint32_t GetHash(const LoadModuleRef& ref) { return ref.hash(); }

  constexpr bool operator==(const Soname& other_name) const { return name() == other_name; }

  constexpr bool operator!=(const Soname& other_name) const { return !(*this == other_name); }

  constexpr bool operator==(const LoadModuleRef& other) const {
    return module_ == other.module_ || (module_ && other.module_ && *this == other.name());
  }

  constexpr bool operator!=(const LoadModuleRef& other) const { return !(*this == other); }

  // While elfldltl::Soname is strictly ordered on the str() value, i.e. ASCII
  // string sorting, ld::LoadModuleRef is has a strong but arbitrary ordering.
  // That is, it's ordered first by hash value (which is precomputed), and
  // only ordered by string comparison among strings with the same hash value.
  //
  // This allows LoadModuleRef is to be used efficiently in either sorted or
  // hashed containers, but only for purposes of lookup, never for enumeration
  // where ordering should be useful.

  constexpr std::strong_ordering operator<=>(const LoadModuleRef& other) const {
    return Ordering<std::compare_three_way>(other);
  }

 private:
  template <class Compare>
  constexpr auto Ordering(const LoadModuleRef& other) const {
    return hash() == other.hash() ? Compare{}(name(), other.name())
                                  : Compare{}(hash(), other.hash());
  }

  LoadModule* module_ = nullptr;
  const Soname& (LoadModule::*name_)() = nullptr;
};

}  // namespace ld

// This is the API contract for standard C++ hash-based containers.
template <class LoadModule>
struct std::hash<ld::LoadModuleRef<LoadModule>> {
  constexpr uint32_t operator()(ld::LoadModuleRef<LoadModule> ref) const { return ref.hash(); }
};

#endif  // LIB_LD_LOAD_MODULE_H_
