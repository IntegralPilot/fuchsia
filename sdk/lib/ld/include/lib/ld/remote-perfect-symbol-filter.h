// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_REMOTE_PERFECT_SYMBOL_FILTER_H_
#define LIB_LD_REMOTE_PERFECT_SYMBOL_FILTER_H_

#include <lib/elfldltl/layout.h>
#include <lib/elfldltl/perfect-symbol-table.h>

#include "remote-decoded-module.h"
#include "remote-load-module.h"

namespace ld {

// This instantiates an elfldltl::PerfectSymbolFilter with the same template
// arguments, and fills it by doing lookups in the module.  If the optional
// flag is true, then symbols in the underlying elfldltl::PerfectSymbolSet not
// found in the module will just not be returned by the filter; otherwise,
// diag.UndefinedSymbol will be called for each one.  The return value is
// nullptr when the diagnostics object returned false.
//
// Note that the returned filter will hold onto the ld::RemoteDecodedModule
// reference just in case, but it should ordinarily be the same one underlying
// the ld::RemoteLoadModule object passed to the filter.
//
// **NOTE:** As with elfldltl::PerfectSymbolSet et al, this should only be
// instantiated where it will have internal linkage, e.g. using a Names
// reference that's inside an anonymous namespace.
template <const auto& Names, class Elf = elfldltl::Elf<>, auto... SetArgs, class Diagnostics>
  requires elfldltl::SymbolNameArray<decltype(Names)>
inline ld::RemoteLoadModule<Elf>::SymbolFilter RemotePerfectSymbolFilter(
    Diagnostics& diag, typename RemoteDecodedModule<Elf>::Ptr module, bool undef_ok = false) {
  struct Filter : elfldltl::PerfectSymbolFilter<Names, Elf, SetArgs...> {
    // Hold a reference to ensure the pointers in the map remain valid.
    typename RemoteDecodedModule<Elf>::Ptr module_keepalive;
  };
  Filter filter;
  if (!filter.Init(diag, *module, undef_ok)) {
    return {};
  }
  filter.module_keepalive = std::move(module);
  return std::move(filter);
}

// This is the type of functions that the perfect_symbol_filter() GN template
// will define.  Each one just wraps a call to ld::RemotePerfectSymbolFilter
// instantiated with an internal-linkage elfldltl::SymbolNameArray initialized
// via elfldltl::PerfectSymbolTable.
template <class Diagnostics, class Elf = elfldltl::Elf<>>
using RemotePerfectSymbolFilterMaker = ld::RemoteLoadModule<Elf>::SymbolFilter(
    Diagnostics& diag, typename RemoteDecodedModule<Elf>::Ptr module);

}  // namespace ld

#endif  // LIB_LD_REMOTE_PERFECT_SYMBOL_FILTER_H_
