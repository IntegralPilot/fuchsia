// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/abi.h>
#include <lib/ld/module.h>
#include <lib/ld/tls.h>
#include <stdint.h>

#include "ensure-test-thread-pointer.h"
#include "test-start.h"
#include "tls-dep.h"

extern "C" int64_t TestStart() {
  if constexpr (HAVE_TLSDESC != WANT_TLSDESC) {
    // This wouldn't be testing what it's supposed to test.
    return 77;
  } else if (EnsureTestThreadPointer()) {
    if (ld::abi::_ld_abi.static_tls_modules.size() != 1) {
      return 1;
    }

    // Any GD accesses here would get relaxed to IE at link time.  So call into
    // the GD-using dependency library.  Though it's in the IE set at runtime,
    // its accesses can't have been relaxed statically since that wasn't known.

    if (get_tls_dep_data() != &tls_dep_data) {
      return 2;
    }

    if (get_tls_dep_bss1() != &tls_dep_bss[1]) {
      return 3;
    }

    if (get_tls_dep_weak()) {
      return 4;
    }

    // TODO(https://fxbug.dev/479518504): There should ideally be a test here
    // of a weak undefined symbol referenced with an addend, but I haven't
    // figured out how to make the compiler generate a reloc with an addend.  }
  }

  for (const auto& module : ld::AbiLoadedModules(ld::abi::_ld_abi)) {
    if (!module.soname.empty() && (module.symbols.flags() & elfldltl::ElfDynFlags::kStaticTls)) {
      return 5;
    }
  }

  return 17;
}
