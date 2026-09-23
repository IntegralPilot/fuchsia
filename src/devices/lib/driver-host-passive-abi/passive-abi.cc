// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/soname.h>
#include <lib/ld/abi.h>
#include <lib/ld/module.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <string_view>

__BEGIN_CDECLS

__EXPORT uint64_t driver_host_find_symbol(uint64_t dl_passive_abi, const char* so_name,
                                          size_t so_name_len, const char* symbol_name,
                                          size_t symbol_name_len) {
  auto abi = reinterpret_cast<const ld::abi::Abi<>*>(dl_passive_abi);
  ZX_ASSERT(abi != nullptr);
  std::string_view so_name_view(so_name, so_name_len);

  elfldltl::SymbolName symbol_name_view{symbol_name, symbol_name_len};
  for (const auto& module_ : ld::AbiLoadedModules<>(*abi)) {
    if (module_.soname.str() != so_name_view) {
      continue;
    }
    auto* symbol = symbol_name_view.Lookup(module_.symbols);
    if (symbol) {
      auto load_bias = module_.link_map.addr;
      return load_bias + symbol->value;
    }
  }
  return 0;
}

__END_CDECLS
