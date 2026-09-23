// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tlsdesc-runtime-dynamic.h"

namespace dl {

fit::result<Error> PrepareTlsBlocksForThread(std::span<const TlsModule> dynamic_tls_modules,
                                             void* tp) {
  fbl::AllocChecker ac;
  SizedDynamicTlsArray blocks = MakeDynamicTlsArray(ac, dynamic_tls_modules.size());
  if (!ac.check()) [[unlikely]] {
    dl::Diagnostics diag;
    diag.OutOfMemory("dynamic TLS vector", dynamic_tls_modules.size() * sizeof(blocks[0]));
    return diag.take_error();
  }

  auto next = blocks.begin();
  for (const TlsModule& storage : dynamic_tls_modules) {
    *next++ = DynamicTlsPtr::New(ac, storage);
    if (!ac.check()) [[unlikely]] {
      dl::Diagnostics diag;
      diag.OutOfMemory("dynamic TLS block", storage.tls_size());
      return diag.take_error();
    }
  }
  assert(next == blocks.end());

  UnsizedDynamicTlsArray old_blocks = ExchangeRuntimeDynamicBlocks(std::move(blocks), tp);
  assert(!old_blocks);

  return fit::ok();
}

}  // namespace dl
