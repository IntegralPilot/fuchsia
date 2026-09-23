// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/abi.h>
#include <lib/ld/module.h>
#include <lib/ld/tls.h>
#include <stdint.h>

#include "ensure-test-thread-pointer.h"
#include "test-start.h"
#include "tls-ie-dep.h"

[[gnu::weak, gnu::tls_model("initial-exec")]] extern constinit thread_local int tls_ie_weak;

extern "C" int64_t TestStart() {
  if (ld::abi::_ld_abi.static_tls_modules.size() != 1) {
    return 1;
  }

  if (EnsureTestThreadPointer()) {
    const ptrdiff_t dep_offset = ld::TlsInitialExecOffset(ld::abi::_ld_abi, 1);
    if (ld::TpRelativeToOffset(tls_ie_data()) != dep_offset) {
      return 2;
    }

    const ptrdiff_t dep_bss_offset = dep_offset + static_cast<ptrdiff_t>(sizeof(int));
    if (ld::TpRelativeToOffset(tls_ie_bss()) != dep_bss_offset) {
      return 3;
    }

    // The glibc behavior for a weak undefined TLS symbol in an IE reloc is
    // just to leave the GOT entry untouched.  That means an offset of zero so
    // you get the thread pointer!
    if (ld::TpRelativeToOffset(&tls_ie_weak) != 0) {
      return 4;
    }

    // TODO(https://fxbug.dev/479518504): There should ideally be a test here
    // of a weak undefined symbol referenced with an addend, but I haven't
    // figured out how to make the compiler generate a reloc with an addend.
    // The expected behavior (consistent with glibc, as above) depends on
    // whether the executable gets linked using DT_REL or DT_RELA, e.g. via `-z
    // rel` on normally-RELA machines.  When using DT_REL, the addend is in the
    // unrelocated GOT entry and so the result for undefined weak will be
    // thread pointer plus addend. However, when using DT_RELA it would be just
    // thread pointer as in the test above because the r_addend is in the reloc
    // that wasn't applied.
  }

  return 17;
}
