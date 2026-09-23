// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_DLFCN_DL_RUNTIME_MODULE_H_
#define LIB_C_DLFCN_DL_RUNTIME_MODULE_H_

#include <lib/elfldltl/alloc-checker-container.h>
#include <lib/elfldltl/layout.h>
#include <lib/elfldltl/soname.h>
#include <lib/elfldltl/symbol.h>
#include <lib/ld/abi.h>
#include <lib/ld/dl-phdr-info.h>
#include <lib/ld/load.h>  // For ld::AbiModule
#include <lib/ld/tls.h>

#include <cstring>

#include <fbl/alloc_checker.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/vector.h>

#include "../dl_phdr_info.h"
#include "diagnostics.h"
#include "tls-desc-resolver.h"

namespace dl {

using AbiModule = ld::AbiModule<>;
using TlsModule = ld::abi::Abi<>::TlsModule;
using Elf = elfldltl::Elf<>;
using Soname = elfldltl::Soname<>;

// Forward declaration; defined below.
class RuntimeModule;

// A list of unique "permanent" RuntimeModule data structures used to represent
// a loaded file in the system image.
using ModuleList = fbl::DoublyLinkedList<std::unique_ptr<RuntimeModule>, fbl::DefaultObjectTag,
                                         fbl::SizeOrder::Constant>;

// Use an AllocCheckerContainer that supports fallible allocations; methods
// return a boolean value to signify allocation success or failure.
template <typename T>
using Vector = elfldltl::AllocCheckerContainer<fbl::Vector>::Container<T>;

// TODO(https://fxbug.dev/478041736): comment on how RuntimeModule relates to
// startup modules when the latter is supported.
// TODO(https://fxbug.dev/328135195): comment on the reference counting when
// that gets implemented.

// RuntimeModules are managed by the RuntimeDynamicLinker. A RuntimeModule is
// created for every unique ELF file object loaded either directly or indirectly
// as a dependency of another module. It holds the ld::abi::Abi<...>::Module
// data structure that describes the module in the passive ABI
// (see <sdk/lib/ld/module.h>).

// A RuntimeModule is created along with a LinkingSession::SessionModule (see
// linking-session.h) to represent the ELF file when it is first loaded by
// `dlopen`. Whereas a SessionModule is ephemeral and only lives as long as the
// the LinkingSession in a `dlopen` call, the RuntimeModule is a "permanent"
// data structure that is kept alive in the RuntimeDynamicLinker's `modules_`
// list until it is unloaded.

// While this is an internal API, a RuntimeModule* is the void* handle returned
// by the public <dlfcn.h> API.
class RuntimeModule : public fbl::DoublyLinkedListable<std::unique_ptr<RuntimeModule>> {
 public:
  using Addr = Elf::Addr;
  using SymbolInfo = elfldltl::SymbolInfo<Elf>;
  using size_type = Elf::size_type;

  // Not copyable, but movable.
  RuntimeModule(const RuntimeModule&) = delete;
  RuntimeModule(RuntimeModule&&) = default;

  // See unmap-[posix|zircon].cc for the dtor. On destruction, the module's load
  // image is unmapped per the semantics of the OS implementation.
  ~RuntimeModule();

  // The name of the runtime module is set to the filename passed to dlopen() to
  // create the runtime module. This is usually the same as the DT_SONAME of the
  // AbiModule, but that is not guaranteed. When performing an equality check,
  // match against both possible name values.
  constexpr bool operator==(const Soname& other) const {
    return other == name() || other == abi_module_.soname;
  }

  constexpr const Soname& name() const { return name_; }

  // Translate a void * pointer, as when passed to public-facing <dlfcn.h> APIs,
  // into a RuntimeModule reference.
  static constexpr const RuntimeModule& FromPtr(const void* ptr) {
    return *static_cast<const RuntimeModule*>(ptr);
  }
  static constexpr RuntimeModule& FromPtr(void* ptr) { return *static_cast<RuntimeModule*>(ptr); }

  // TODO(https://fxbug.dev/333920495): pass in the symbolizer_modid.
  [[nodiscard]] static std::unique_ptr<RuntimeModule> Create(fbl::AllocChecker& ac, Soname soname) {
    auto result = [&ac](std::unique_ptr<RuntimeModule> v) {
      ac.arm(sizeof(RuntimeModule), v);
      return v;
    };

    fbl::AllocChecker module_ac;
    std::unique_ptr<RuntimeModule> module{new (module_ac) RuntimeModule};
    if (!module_ac.check()) [[unlikely]] {
      return result(nullptr);
    }
    fbl::AllocChecker name_ac;

    size_t buf_sz = soname.size() + 1;
    char* buf = new (name_ac) char[buf_sz];
    if (!name_ac.check()) [[unlikely]] {
      return result(nullptr);
    }
    soname.copy(buf, buf_sz);
    module->name_ = Soname{buf};

    return result(std::move(module));
  }

  // This is called if the RuntimeModule is created from a startup module (i.e.
  // a module that was linked and loaded with the running program). This sets
  // the abi, TLS, and direct_deps information onto the RuntimeModule.
  void SetStartupModule(const AbiModule& abi_module, const ld::abi::Abi<>& abi) {
    abi_module_ = abi_module;
    no_delete_ = true;
    initialized_ = true;

    if (abi_module_.tls_modid > 0) {
      static_tls_bias_ = abi.static_tls_offsets[abi_module_.tls_modid - 1];
    }
  }

  constexpr AbiModule& module() { return abi_module_; }
  constexpr const AbiModule& module() const { return abi_module_; }

  size_t vaddr_size() const { return abi_module_.vaddr_end - abi_module_.vaddr_start; }

  // The following methods satisfy the Module template API for use with
  // elfldltl::ResolverDefinition (see <lib/elfldltl/resolve.h>).

  const SymbolInfo& symbol_info() const { return abi_module_.symbols; }

  constexpr const ld::abi::Abi<>::LinkMap& link_map() const { return abi_module_.link_map; }

  // The ld::abi::Abi<>::LinkMap embedded in the RuntimeModule's ABI module is
  // compatible with `struct link_map` (see <lib/elfldltl/svr4-abi.h>). This
  // will cast the module's LinkMap into the `struct link_map` type used by
  // users of the public-facing <dlfcn.h> API.
  const struct link_map* user_link_map() const {
    return reinterpret_cast<const struct link_map*>(&link_map());
  }

  constexpr Addr load_bias() const { return abi_module_.link_map.addr; }

  constexpr size_type tls_module_id() const { return abi_module_.tls_modid; }

  constexpr bool uses_static_tls() const { return ld::ModuleUsesStaticTls(abi_module_); }

  constexpr size_t static_tls_bias() const { return static_tls_bias_; }

  constexpr fit::result<bool, const typename Elf::Sym*> Lookup(  //
      Diagnostics& diag, elfldltl::SymbolName& name) const {
    return fit::ok(name.Lookup(symbol_info()));
  }

  // This is a list of module pointers to this module's DT_NEEDEDs, i.e. the
  // first level of dependencies in this module's module tree. If this list is
  // empty, the module does not have any dependencies.
  constexpr auto& direct_deps() { return direct_deps_; }

  // This is the breadth-first ordered list of module references, representing
  // this module's tree of modules.  A reference to this module (the root) is
  // always the first in this list.  The other modules in this list are
  // non-owning references to modules that were explicitly linked with this
  // module; global modules that may have been used for relocations, but are
  // not a DT_NEEDED of any dependency, are not included in this list.  This
  // list is set when dlopen() is called on this module.
  constexpr auto module_tree() const {  // Returns a constant view.
    // RuntimeModule::ReifyModuleTree should have been called before any
    // callers call this accessor.
    assert(!module_tree_.is_empty());
    return std::views::transform(module_tree_, Deref<const RuntimeModule>{});
  }
  constexpr auto module_tree() {  // Returns the mutable view.
    assert(!module_tree_.is_empty());
    return std::views::transform(module_tree_, Deref<RuntimeModule>{});
  }

  // Constructs this module's `module_tree` if it has not been set yet.
  bool ReifyModuleTree(Diagnostics& diag);

  // Whether this module is a global module: either the module was loaded at
  // startup or loaded by dlopen() with the RTLD_GLOBAL flag.
  constexpr bool is_global() const { return abi_module_.symbols_visible; }
  constexpr void set_global() { abi_module_.symbols_visible = true; }
  constexpr bool is_local() const { return !is_global(); }

  // Prevent this module from being unloaded. Once this is set (usually by
  // passing RTLD_NODELETE to dlopen), it cannot be unset. Setting this on a
  // module implies that none of its dependencies can be unloaded either: since
  // this module will live for the lifetime of the main program, so will its
  // dependencies.
  constexpr void set_no_delete() { no_delete_ = true; }

  // This is the list of TlsDesc objects (see tls-desc-resolver.h) that was set
  // during TLS relocation for this module. The RuntimeModule owns this list:
  // it will get destroyed with the module.
  constexpr TlsdescIndirectList& tls_desc_indirect_list() { return tls_desc_indirect_list_; }

  // Run the init functions for this module (as the root module) and the init
  // functions for all its dependencies.
  void InitializeModuleTree();

  // Run only this module's init functions. This is called if this module is
  // loaded as a dependency to another module.
  void Initialize();

  // Construct the `dl_phdr_info` for this module.
  dl_phdr_info MakeDlPhdrInfo(void* tls_data, ld::DlPhdrInfoCounts counts) const {
    return ld::MakeDlPhdrInfo(module(), tls_data, counts);
  }

 private:
  template <typename T>
  struct Deref {
    constexpr T& operator()(auto&& ptr) const { return *ptr; }
  };

  // A RuntimeModule can only be created with Module::Create...).
  RuntimeModule() = default;

  static void Unmap(uintptr_t vaddr, size_t len);

  // This name matches abi_module_.link_map.name, not abi_module_.soname.
  Soname name_;
  AbiModule abi_module_;

  Vector<RuntimeModule*> direct_deps_;
  Vector<RuntimeModule*> module_tree_;  // This is a superset of direct_deps_.

  TlsdescIndirectList tls_desc_indirect_list_;
  size_type static_tls_bias_ = 0;
  bool no_delete_ = false;
  bool initialized_ = false;
};

// This is the module tree view type returned by RuntimeModule::module_tree().
using ModuleTree = decltype(std::declval<RuntimeModule>().module_tree());
using ConstModuleTree = decltype(std::declval<const RuntimeModule>().module_tree());
static_assert(std::ranges::forward_range<ModuleTree>);
static_assert(std::ranges::forward_range<ConstModuleTree>);
static_assert(std::same_as<RuntimeModule&, std::ranges::range_reference_t<ModuleTree>>);
static_assert(std::same_as<const RuntimeModule&, std::ranges::range_reference_t<ConstModuleTree>>);

}  // namespace dl

#endif  // LIB_C_DLFCN_DL_RUNTIME_MODULE_H_
