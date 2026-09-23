// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "runtime-dynamic-linker.h"

#include <lib/fit/defer.h>

#include <algorithm>

#include <fbl/array.h>

#include "tlsdesc-runtime-dynamic.h"

namespace dl {

LinkingSessionBase::LinkingSessionBase(RuntimeDynamicLinker& linker) : linker_{linker} {}

ModuleList& LinkingSessionBase::loaded_modules() { return linker_.modules(); }

size_type LinkingSessionBase::max_static_tls_modid() const {
  return linker_.max_static_tls_modid();
}

size_type LinkingSessionBase::max_tls_modid() const {
  return linker_.max_static_tls_modid() + linker_.dynamic_tls_modules().size();
}

fit::result<Error> RuntimeDynamicLinker::AddNewTlsModules(Vector<TlsModule> tls_modules) {
  Diagnostics diag;
  if (!dynamic_tls_modules_.reserve(diag, "dynamic TLS vector",
                                    dynamic_tls_modules_.size() + tls_modules.size()))
      [[unlikely]] {
    return diag.take_error();
  }
  for (auto& tls_module : tls_modules) {
    dynamic_tls_modules_.push_back(std::move(tls_module));
  }
  return diag.ok();
}

void RuntimeDynamicLinker::AddNewModules(ModuleList modules) {
  loaded_ += modules.size();
  modules_.splice(modules_.end(), modules);
}

RuntimeModule* RuntimeDynamicLinker::FindModule(Soname name) {
  auto it = std::ranges::find_if(modules_, name.equal_to());
  if (it == modules_.end()) {
    return nullptr;
  }
  // TODO(https://fxbug.dev/328135195): increase reference count.
  RuntimeModule& found = *it;
  return &found;
}

void* RuntimeDynamicLinker::TlsBlock(const RuntimeModule& module) const {
  assert(module.tls_module_id() > 0);
  return module.tls_module_id() <= max_static_tls_modid()
             ? ld::TlsInitialExecData(abi_, module.tls_module_id())
             : dl::DynamicTlsData(
                   abi_, module.tls_module_id(),
                   dynamic_tls_modules_[abi_.dynamic_tls_index(module.tls_module_id())]);
}

fit::result<Error, void*> RuntimeDynamicLinker::LookupSymbol(const RuntimeModule& root,
                                                             const char* ref) {
  Diagnostics diag;
  // The root module's name is included in symbol not found errors.
  ld::ScopedModuleDiagnostics root_diag{diag, root.name().str()};

  elfldltl::SymbolName name{ref};
  // TODO(https://fxbug.dev/370087572): properly handle weak symbols.
  for (const RuntimeModule& module : root.module_tree()) {
    if (const auto* sym = name.Lookup(module.symbol_info())) {
      bool is_tls = sym->type() == elfldltl::ElfSymType::kTls;
      uintptr_t bias = (is_tls ? reinterpret_cast<uintptr_t>(TlsBlock(module))
                               : static_cast<size_type>(module.load_bias()));
      return diag.ok(reinterpret_cast<void*>(sym->value + bias));
    }
  }
  diag.UndefinedSymbol(ref);
  return diag.take_error();
}

void RuntimeDynamicLinker::MakeGlobal(ModuleTree module_tree) {
  // This iterates through the `module_tree`, promoting any modules that are
  // not already global.  When a module is promoted, it is looked up in the
  // dynamic linker's `modules_` list and moved to the back of that
  // doubly-linked list.  Note, that this loop does not change the ordering of
  // the `module_tree`.
  for (RuntimeModule& loaded_module : module_tree) {
    // If the loaded module is already global, then its load order does not
    // change in modules_.
    if (loaded_module.is_global()) {
      continue;
    }
    loaded_module.set_global();
    // Move the promoted module to the back of the dynamic linker's modules_
    // list.
    modules_.push_back(modules_.erase(loaded_module));
  }
}

void RuntimeDynamicLinker::PopulateStartupModules(fbl::AllocChecker& func_ac,
                                                  const ld::abi::Abi<>& abi) {
  // Arm the function-level AllocChecker with the result of the function.
  auto set_result = [&func_ac](bool v) { func_ac.arm(sizeof(RuntimeModule), v); };

  ModuleList startup_modules;
  for (const AbiModule& abi_module : ld::AbiLoadedModules(abi)) {
    fbl::AllocChecker ac;
    std::unique_ptr<RuntimeModule> module =
        RuntimeModule::Create(ac, Soname{abi_module.link_map.name.get()});
    if (!ac.check()) [[unlikely]] {
      set_result(false);
      return;
    }
    module->SetStartupModule(abi_module, abi);
    startup_modules.push_back(std::move(module));
  }

  // In a second pass, set the direct dependencies for each startup module.
  // This loop will get each DT_NEEDED offset from the ABI module's PT_DYNAMIC
  // (provided by `link_map.ld`) and look the offset up in the ABI module's
  // symbol table to get the name of the dependency. The module for the dep
  // (which should also be in the `startup_modules` list) is then added to the
  // depending module's `direct_deps` list.
  for (RuntimeModule& module : startup_modules) {
    const AbiModule& abi_module = module.module();
    for (const Elf::Dyn* dyn_entry = abi_module.link_map.ld.get();
         dyn_entry->tag != elfldltl::ElfDynTag::kNull; ++dyn_entry) {
      // Skip any entries that are not a DT_NEEDED.
      if (dyn_entry->tag != elfldltl::ElfDynTag::kNeeded) {
        continue;
      }
      const char* dep_name = abi_module.symbols.string(dyn_entry->val);
      auto found = std::ranges::find_if(startup_modules, Soname{dep_name}.equal_to());
      // The ABI should include all the dependencies of every startup module in
      // its .loaded_modules list.
      assert(found != startup_modules.end());

      fbl::AllocChecker ac;
      module.direct_deps().push_back(&*found, &ac);
      if (!ac.check()) [[unlikely]] {
        set_result(false);
        return;
      }
    }
  }

  AddNewModules(std::move(startup_modules));

  set_result(true);
}

// TODO(https://fxbug.dev/342484765): Have this function return fit::result<Error::OutOfMemory>.
std::unique_ptr<RuntimeDynamicLinker> RuntimeDynamicLinker::Create(const ld::abi::Abi<>& abi,
                                                                   fbl::AllocChecker& ac) {
  assert(abi.loaded_modules);
  assert(abi.static_tls_modules.size() == abi.static_tls_offsets.size());

  // Arm the caller's AllocChecker with the return value of this function.
  auto result = [&ac](std::unique_ptr<RuntimeDynamicLinker> v) {
    ac.arm(sizeof(RuntimeDynamicLinker), v);
    return v;
  };

  fbl::AllocChecker linker_ac;
  std::unique_ptr<RuntimeDynamicLinker> dynamic_linker{new (linker_ac) RuntimeDynamicLinker(abi)};
  if (linker_ac.check()) [[likely]] {
    fbl::AllocChecker populate_ac;
    dynamic_linker->PopulateStartupModules(populate_ac, abi);
    if (!populate_ac.check()) [[unlikely]] {
      return result(nullptr);
    }
  }

  return result(std::move(dynamic_linker));
}

// TODO(https://fxbug.dev/382516279): This needs to handle synchronization
// between locking the modules_ list and running the user callback outside
// of any locks.
int RuntimeDynamicLinker::IteratePhdrInfo(DlIteratePhdrCallback* callback, void* data) const {
  for (const RuntimeModule& module : modules_) {
    void* tls = module.tls_module_id() == 0 ? nullptr : TlsBlock(module);
    dl_phdr_info phdr_info = module.MakeDlPhdrInfo(tls, dl_phdr_info_counts());
    // A non-zero return value ends the iteration.
    if (int result = callback(&phdr_info, sizeof(phdr_info), data); result != 0) {
      return result;
    }
  }
  return 0;
}

fit::result<Error, int> RuntimeDynamicLinker::DlInfo(void* handle, int request, void* info) {
  const RuntimeModule& module = RuntimeModule::FromPtr(handle);
  switch (request) {
    case kLinkMap:
      *static_cast<const link_map**>(info) = module.user_link_map();
      return fit::ok(0);

    case kTlsModid:
      *static_cast<size_t*>(info) = module.tls_module_id();
      return fit::ok(0);

    case kTlsData:
      *static_cast<void**>(info) = module.tls_module_id() == 0 ? nullptr : TlsBlock(module);
      return fit::ok(0);

    case kPhdrs:
      *static_cast<ElfW(Phdr)**>(info) = const_cast<ElfW(Phdr)*>(
          reinterpret_cast<const ElfW(Phdr)*>(module.module().phdrs.data()));
      return fit::ok(static_cast<int>(module.module().phdrs.size()));

    default:
      return fit::error{Error{"unsupported dlinfo request"}};
  };
}

}  // namespace dl
