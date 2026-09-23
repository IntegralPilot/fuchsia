// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/abi.h>
#include <lib/ld/module.h>
#include <lib/ld/tls.h>

#include <bit>

#include "ensure-test-thread-pointer.h"
#include "test-start.h"
#include "tls-ld-dep.h"

extern "C" int64_t TestStart() {
  if constexpr (HAVE_TLSDESC != WANT_TLSDESC) {
    // This wouldn't be testing what it's supposed to test.
    return 77;
  } else if (EnsureTestThreadPointer()) {
    if (ld::abi::_ld_abi.static_tls_modules.size() != 1) {
      return 1;
    }

    if (ld::abi::_ld_abi.static_tls_offsets.size() != 1) {
      return 2;
    }

    const ptrdiff_t tp_offset =
        std::bit_cast<ptrdiff_t>(ld::abi::_ld_abi.static_tls_offsets.front());

    int* data = get_tls_ld_dep_data();
    char* bss1 = get_tls_ld_dep_bss1();

    if (ld::TpRelativeToOffset(data) != tp_offset) {
      return 3;
    }

    if (ld::TpRelativeToOffset(bss1) != tp_offset + kTlsLdDepBss1Offset) {
      return 4;
    }
  }

  for (const auto& module : ld::AbiLoadedModules(ld::abi::_ld_abi)) {
    if (!module.soname.empty() && (module.symbols.flags() & elfldltl::ElfDynFlags::kStaticTls)) {
      return 5;
    }
  }

  return 17;
}
