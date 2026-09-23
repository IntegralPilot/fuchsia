// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/abi.h>
#include <lib/ld/module.h>
#include <stdint.h>

#include <algorithm>

#include "test-start.h"

extern "C" int64_t a();

extern "C" int64_t TestStart() {
  // We expect to return 17 here.  a() returns 13.  We add 1 for every global
  // object in the module list.  This includes:
  //  1. the executable
  //  2. libld-dep-a.so
  //  3. ld.so.1
  //  4. On Fuchsia only for out-of-process tests only, libzircon.so
#if defined(__Fuchsia__) && !defined(IN_PROCESS_TEST)
  constexpr int kExtraDeps = 0;
#else
  constexpr int kExtraDeps = 1;
#endif

  // Use a() to ensure we get a DT_NEEDED on libld-dep-a.so even with
  // --as-needed.
  int64_t symbolic_deps = a();

  auto modules = ld::AbiLoadedModules(ld::abi::_ld_abi);
  auto visible = [](const auto& module) { return module.symbols_visible; };
  symbolic_deps += std::count_if(modules.begin(), modules.end(), visible);

  return symbolic_deps + kExtraDeps;
}
