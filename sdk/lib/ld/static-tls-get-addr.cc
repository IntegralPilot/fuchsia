// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/machine.h>
#include <lib/ld/abi.h>
#include <lib/ld/tls.h>
#include <zircon/compiler.h>

namespace ld::abi {

__EXPORT void* __tls_get_addr(const elfldltl::Elf<>::TlsGetAddrGot<>& got) {
  if (got.tls_modid == 0) [[unlikely]] {
    // Note that glibc doesn't handle this case properly at all, and is liable
    // to crash if it gets called.  It's clearly only intended for undefined
    // weak TLS symbols to be allowed if they aren't actually referenced in
    // live code paths at runtime.
    return nullptr;
  }
  return TpRelative(TlsInitialExecOffset(_ld_abi, got.tls_modid) +
                    static_cast<ptrdiff_t>(got.offset + elfldltl::TlsTraits<>::kTlsRelativeBias));
}

}  // namespace ld::abi
