// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/module.h>

#include <algorithm>

#include "ld-abi.h"
#include "libc.h"
#include "src/__support/common.h"

namespace LIBC_NAMESPACE_DECL {
namespace {

void ModulesFini() {
  // TODO(https://fxbug.dev/338239201): Mitigate dlopen/dlclose calls either
  // racing in other threads or directly in fini functions.

  constexpr auto module_fini = [](const auto& module) {
    module.fini.CallFini(module.link_map.addr);
  };

  // Initializers ran in reverse load order in StartupCtors(), so finalizers
  // are in load order here: the executable's run first.
  std::ranges::for_each(ld::AbiLoadedModules(_ld_abi), module_fini);
}

}  // namespace
}  // namespace LIBC_NAMESPACE_DECL

// This is the `extern "C"` name also defined by musl's integrated dynamic
// linker.  A name in LIBC_NAMESPACE will be used directly when that's gone.
void __libc_exit_fini() { LIBC_NAMESPACE::ModulesFini(); }
