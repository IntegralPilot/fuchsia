// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dl-impl-tests.h"

#include <ranges>

namespace dl::testing {

thread_local DlImplTestsTls DlImplTestsTls::cleanup_at_thread_exit_;

// The thread_local blocks_ tracks any array previously installed by an earlier
// call on this same thread within this same test.  Its .data() should always
// match what's in _dl_tlsdesc_runtime_dynamic_blocks.  Its only real purpose
// is just to remember the old size so EnlargeDynamicTlsArray can be used.
void DlImplTestsTls::Prepare(const RuntimeDynamicLinker& linker) {
  std::span<const dl::TlsModule> dynamic_tls_modules = linker.dynamic_tls_modules();
  size_t dynamic_tls_size = dynamic_tls_modules.size();

  UnsizedDynamicTlsArray used_blocks = ExchangeRuntimeDynamicBlocks({});
  ASSERT_EQ(used_blocks.release(), cleanup_at_thread_exit_.blocks_.data());

  size_t prev_tls_size = cleanup_at_thread_exit_.blocks_.size();
  ASSERT_GE(dynamic_tls_size, prev_tls_size);

  // Only enlarge the dynamic TLS array if new TLS modules were loaded.
  if (dynamic_tls_size > prev_tls_size) {
    fbl::AllocChecker ac;
    SizedDynamicTlsArray new_blocks =
        EnlargeDynamicTlsArray(ac, cleanup_at_thread_exit_.blocks_, dynamic_tls_size);
    ASSERT_TRUE(ac.check());

    // Initialize any new TLS blocks for TLS modules that have been loaded since
    // the last time __dl_tlsdesc_runtime_dynamic_blocks was prepared for TLS access.
    for (const auto& [dest, module] :
         std::views::drop(std::views::zip(new_blocks, dynamic_tls_modules), prev_tls_size)) {
      fbl::AllocChecker block_ac;
      dest = DynamicTlsPtr::New(block_ac, module);
      ASSERT_TRUE(block_ac.check());
    }
    cleanup_at_thread_exit_.blocks_ = std::move(new_blocks);
  }

  _dl_tlsdesc_runtime_dynamic_blocks = cleanup_at_thread_exit_.blocks_.data();
}

// At teardown for in-test extra threads, or at the end of the test on the main
// thread, the array is freed.
void DlImplTestsTls::Cleanup() {
  UnsizedDynamicTlsArray used_blocks = ExchangeRuntimeDynamicBlocks({});
  ASSERT_EQ(cleanup_at_thread_exit_.blocks_.data(), used_blocks.get());
  // It's deleted when used_blocks goes out of scope, so don't double-delete[].
  std::ignore = cleanup_at_thread_exit_.blocks_.release();
}

}  // namespace dl::testing
