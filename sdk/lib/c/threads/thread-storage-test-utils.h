// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_THREADS_THREAD_STORAGE_TEST_UTILS_H_
#define LIB_C_THREADS_THREAD_STORAGE_TEST_UTILS_H_

#include <lib/elfldltl/tls-layout.h>
#include <lib/fit/function.h>
#include <zircon/types.h>

#include <span>
#include <string_view>

#include "thread-storage.h"

namespace LIBC_NAMESPACE_DECL {

// This provides a simple way of setting gTlsLayout and gInitializeTls within a test to
// be used by ThreadStorage::GetTlsLayout and ThreadStorage::InitializeTls in test code.
class LibcThreadTestScopedTlsGlobals {
 public:
  using TlsLayout = elfldltl::TlsLayout<>;
  using InitTlsFn = fit::function<void(std::span<std::byte>, size_t)>;

  // This is a large enough size (far larger than a test would need), but harmlessly small in even
  // the 38-bit address space for riscv64-fuchsia. Tests can allocate a VMAR of this size to place
  // test VMOs here.
  static constexpr size_t kTestVmarSizeBytes = 1 << 30;

  LibcThreadTestScopedTlsGlobals(TlsLayout tls_layout, InitTlsFn initialize_tls) {
    gTlsLayout = tls_layout;
    gInitializeTls = std::move(initialize_tls);
  }

  ~LibcThreadTestScopedTlsGlobals() {
    gTlsLayout = {};
    gInitializeTls = {};
  }

  static inline TlsLayout gTlsLayout;
  static inline InitTlsFn gInitializeTls;
};

// This is a helper subclass for just recording the thread_block and tp_offset
// from a ThreadStorage::Allocate and checking the resulting storage. Users can
// also supply their own `expect_true` function for printing assertion messages.
//
// Example usage:
//
//   LibcThreadTestStorage thread_storage(layout);
//   auto result = thread_storage.Allocate(handles, stack_size, guard_size);
//   thread_storage.Check([](bool check, std::string_view message){
//     ZX_ASSERT_MSG(check, "%s", message.data());
//   });
//
class LibcThreadTestStorage : public LibcThreadTestScopedTlsGlobals {
 public:
  LibcThreadTestStorage(TlsLayout tls_layout)
      : LibcThreadTestScopedTlsGlobals(tls_layout,
                                       [this](std::span<std::byte> thread_block, size_t tp_offset) {
                                         // Just record the resulting thread block calculations.
                                         found_thread_block_ = thread_block;
                                         found_tp_offset_ = tp_offset;
                                       }) {}

  zx::result<Thread*> Allocate(thrd_zx_create_handles_t allocate_from,
                               PageRoundedSize stack_size = PageRoundedSize::Page(),
                               PageRoundedSize guard_size = PageRoundedSize::Page(),
                               std::string_view vmo_name = "thread-storage-test") {
    return storage_.Allocate(allocate_from, vmo_name, stack_size, guard_size);
  }

  void Check(fit::function<void(bool, std::string_view)> expect_true, Thread* result);

 private:
  ThreadStorage storage_;
  zx::result<Thread*> result_;
  size_t found_tp_offset_;
  std::span<std::byte> found_thread_block_;
};

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_THREADS_THREAD_STORAGE_TEST_UTILS_H_
