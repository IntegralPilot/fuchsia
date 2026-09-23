// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dladdr-impl.h"

#include <memory>

#include "../ld/ld-abi.h"
#include "../weak.h"
#include "dlfcn-abi.h"

namespace LIBC_NAMESPACE_DECL {
namespace {

using Module = ld::abi::Abi<>::Module;
using Sym = elfldltl::Elf<>::Sym;

const Module* FindModule(uintptr_t vaddr) {
  const auto modules = ld::AbiLoadedModules(_ld_abi);
  auto module = ld::FindModuleByVaddr(modules, vaddr);
  return module == modules.end() ? nullptr : std::addressof(*module);
}

}  // namespace

DladdrResult DladdrImpl(const void* __restrict addr, Dl_info* __restrict info) {
  const uintptr_t vaddr = reinterpret_cast<uintptr_t>(addr);

  // libdl takes over here if it's present.
  const Module* module = Weak<_dlfcn_module_by_vaddr>::Fallback<FindModule>(vaddr);
  if (!module) {
    return {};
  }

  // Always store the module information.
  const uintptr_t base = module->link_map.addr;
  *info = {
      .dli_fname = module->link_map.name.get(),
      .dli_fbase = reinterpret_cast<void*>(base),
  };

  const Sym* sym = module->symbols.LookupVaddr(vaddr - base);
  if (sym) {
    const uintptr_t symaddr = sym->value + base;
    info->dli_saddr = reinterpret_cast<void*>(symaddr);
    info->dli_sname = module->symbols.string(sym->name);
  }

  return {.module = module, .sym = sym};
}

}  // namespace LIBC_NAMESPACE_DECL
