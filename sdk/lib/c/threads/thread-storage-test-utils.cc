// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "thread-storage-test-utils.h"

#include <lib/ld/tls.h>
#include <zircon/assert.h>

#include <algorithm>

#include "stack-abi.h"
#include "threads_impl.h"

namespace LIBC_NAMESPACE_DECL {

// This is defined in the non-test code to get the real layout from the dynamic
// linking state and such.  In test code, it's set to a synthetic layout.
elfldltl::TlsLayout<> ThreadStorage::GetTlsLayout() {
  return LibcThreadTestScopedTlsGlobals::gTlsLayout;
}

// This is defined in the non-test code to fill the real layout with all the
// actual PT_TLS segments.  In test code, it's a callback set by the test.
void ThreadStorage::InitializeTls(std::span<std::byte> thread_block, size_t tp_offset) {
  if (LibcThreadTestScopedTlsGlobals::gInitializeTls) {
    LibcThreadTestScopedTlsGlobals::gInitializeTls(thread_block, tp_offset);
  }
}

void LibcThreadTestStorage::Check(fit::function<void(bool, std::string_view)> expect_true,
                                  Thread* result) {
  const ptrdiff_t kTlsBias =
      static_cast<ptrdiff_t>(elfldltl::TlsTraits<>::kTlsLocalExecOffset) -
      (elfldltl::TlsTraits<>::kTlsNegative ? static_cast<ptrdiff_t>(gTlsLayout.size_bytes()) : 0);
  std::span<std::byte> tls_portion =
      found_thread_block_.subspan(found_tp_offset_ + kTlsBias, gTlsLayout.size_bytes());
  expect_true(std::all_of(tls_portion.begin(), tls_portion.end(),
                          [](std::byte b) { return b == std::byte{0}; }),
              "TLS portion of the thread is not all zeroes.");

  expect_true(
      ld::TpRelative(-found_tp_offset_, pthread_to_tp(result)) == found_thread_block_.data(),
      "Expected the tp offset to point to the start of the thread block");

  expect_true(found_thread_block_.size() % PageRoundedSize::Page().get() == 0,
              "Thread block size is not a multiple of page size");
  expect_true(found_thread_block_.size() >= gTlsLayout.size_bytes(),
              "Insufficient block size to hold the full TLS layout");
}

}  // namespace LIBC_NAMESPACE_DECL
