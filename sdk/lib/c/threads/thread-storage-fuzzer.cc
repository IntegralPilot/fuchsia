// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/tls-layout.h>
#include <lib/fit/defer.h>
#include <lib/zx/vmar.h>
#include <zircon/assert.h>
#include <zircon/process.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <bit>
#include <iostream>

#include <fuzzer/FuzzedDataProvider.h>

#include "thread-storage-test-utils.h"

extern "C" int LLVMFuzzerTestOneInput(const uint8_t* data, size_t size);

extern "C" int LLVMFuzzerTestOneInput(const uint8_t* data, size_t size) {
  using TlsTraits = elfldltl::TlsTraits<>;

  FuzzedDataProvider provider(data, size);

  // Place everything inside a constrained VMAR that's always destroyed at the
  // end of the test.
  zx::vmar test_vmar;
  uintptr_t base;
  zx_status_t status = zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
      LIBC_NAMESPACE::LibcThreadTestScopedTlsGlobals::kTestVmarSizeBytes, &test_vmar, &base);

  // This should never fail and isn't input-dependent. It's an unexpected catastrophic failure
  // if this does fail.
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to allocate test VMAR: %s", zx_status_get_string(status));
  auto auto_destroy_test_vmar = fit::defer([&test_vmar]() {
    zx_status_t status = test_vmar.destroy();
    ZX_ASSERT_MSG(status == ZX_OK, "Failed to destroy test VMAR: %s", zx_status_get_string(status));
  });

  // Zero is a valid size. It will probably eventually always be impossible for it to come
  // up because even static libc code will have some TLS use in the minimal amounts that must have
  // been linked in. But right now, it's actually possible. And ThreadStorage is designed to support
  // it in general, so it should be tested by the fuzzer. The ABI also requires that
  // if > 0 it be >= kTlsLocalExecOffset.
  const size_t initial_tls_size = provider.ConsumeIntegral<size_t>();
  const size_t tls_size =
      initial_tls_size > 0 ? std::min(TlsTraits::kTlsLocalExecOffset, initial_tls_size) : 0;

  // Zero is a valid value in the API. If there is a minimum size (PTHREAD_STACK_MIN),
  // then that should be enforced with graceful failure (EINVAL) as it is in
  // pthread_attr_setstacksize.
  //
  // The underlying allocation APIs operate on page-aligned sizes. To avoid wasting fuzzer time
  // on inputs that are not page-aligned, we generate only page-aligned sizes.
  const size_t page_size = zx_system_get_page_size();
  const size_t max_multiplier = SIZE_MAX / page_size;
  const size_t stack_mult = provider.ConsumeIntegralInRange<size_t>(0, max_multiplier);
  const size_t guard_mult = provider.ConsumeIntegralInRange<size_t>(0, max_multiplier);

  // It's an expected constraint at the ELF ABI level that PT_TLS p_align
  // cannot be greater than system page size, so set this as the max alignment.
  const uint8_t alignment_pow2 = provider.ConsumeIntegralInRange<uint8_t>(
      0, static_cast<uint8_t>(std::countr_zero(zx_system_get_page_size())));
  const size_t alignment = 1 << alignment_pow2;

  const elfldltl::TlsLayout<> layout(tls_size, alignment);

  const thrd_zx_create_handles_t handles = {
      .machine_stack_vmar = test_vmar.get(),
      .security_stack_vmar = test_vmar.get(),
      .thread_block_vmar = test_vmar.get(),
  };

  const LIBC_NAMESPACE::PageRoundedSize stack_size_rounded =
      LIBC_NAMESPACE::PageRoundedSize::From(stack_mult * page_size).value();
  const LIBC_NAMESPACE::PageRoundedSize guard_size_rounded =
      LIBC_NAMESPACE::PageRoundedSize::From(guard_mult * page_size).value();

  LIBC_NAMESPACE::LibcThreadTestStorage thread_storage(layout);
  auto result = thread_storage.Allocate(handles, stack_size_rounded, guard_size_rounded);

  switch (result.status_value()) {
    case ZX_ERR_INVALID_ARGS:
      ZX_ASSERT_MSG(
          stack_size_rounded.get() < PTHREAD_STACK_MIN,
          ": ZX_ERR_INVALID_ARGS should only be expected if the stack size (%zu) is less than the stack min (%d) "
          "[stack_size_rounded: 0x%zx, guard_size_rounded: 0x%zx, tls_size: %zu, alignment: %zu]",
          stack_size_rounded.get(), PTHREAD_STACK_MIN, stack_size_rounded.get(),
          guard_size_rounded.get(), tls_size, alignment);
      return -1;
    default:
      break;
  }

  if (result.is_error()) {
    // This should be the only error that's possible from the underlying
    // syscalls that can fail here. We can run into this is the stack or guard
    // sizes are too big, or we can't map each of the blocks into VMO.
    ZX_ASSERT_MSG(result.status_value() == ZX_ERR_NO_RESOURCES,
                  "Error from ThreadStorage::Allocate: %s", result.status_string());
    return -1;
  }
  thread_storage.Check(
      [](bool check, std::string_view message) { ZX_ASSERT_MSG(check, "%s", message.data()); },
      *result);

  return 0;
}
