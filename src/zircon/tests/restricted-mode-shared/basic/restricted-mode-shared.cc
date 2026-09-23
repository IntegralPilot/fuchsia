// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <lib/zx/job.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>

#include <thread>

#include <zxtest/zxtest.h>

#include "helpers.h"
#include "write_to_stack_blob.h"

namespace {

void RunRestrictedMode(zx_vaddr_t cs_addr, zx_vaddr_t stack_addr,
                       std::optional<zx_excp_type_t> expected_exception, zx_status_t* result) {
  // Set up the restricted state.
  zx::vmo restricted;
  *result = zx_restricted_bind_state(0, restricted.reset_and_get_address(), nullptr);
  if (*result != ZX_OK) {
    return;
  }
  zx_restricted_state_t state{};
#if defined(__x86_64__)
  state.rsp = reinterpret_cast<uintptr_t>(stack_addr);
  state.ip = reinterpret_cast<uintptr_t>(cs_addr);
#elif defined(__aarch64__) || defined(__riscv)  // defined(__x86_64__)
  state.sp = reinterpret_cast<uintptr_t>(stack_addr);
  state.pc = reinterpret_cast<uintptr_t>(cs_addr);
#endif                                          // defined(__arch64__) || defined(__riscv)
  *result = restricted.write(&state, 0, sizeof(state));
  if (*result != ZX_OK) {
    return;
  }

  // Enter restricted mode.
  zx_restricted_reason_t exit_reason = 99;
  *result = restricted_enter_wrapper(0, &exit_reason);
  if (*result != ZX_OK) {
    return;
  }

  if (expected_exception) {
    // If we expected an exception, check that we got that exception and return.
    if (exit_reason != ZX_RESTRICTED_REASON_EXCEPTION) {
      *result = ZX_ERR_INTERNAL;
      return;
    }
    zx_restricted_exception_t exception_state{};
    *result = restricted.read(&exception_state, 0, sizeof(exception_state));
    if (*result != ZX_OK) {
      return;
    }
    if (exception_state.exception.header.type != expected_exception) {
      *result = ZX_ERR_INTERNAL;
    }
    return;
  } else if (exit_reason != ZX_RESTRICTED_REASON_SYSCALL) {
    // Otherwise, verify that we exited due to a syscall.
    *result = ZX_ERR_INTERNAL;
    return;
  }

  // Validate that the restricted mode routine wrote the right value to the stack without going
  // through zx_vmo_read.
  uint8_t expected[8] = {0xdd, 0xdd, 0xcc, 0xcc, 0xbb, 0xbb, 0xaa, 0xaa};
  uint8_t* stack_values = reinterpret_cast<uint8_t*>(stack_addr - 8);
  for (size_t i = 0; i < sizeof(expected); i++) {
    if (expected[i] != stack_values[i]) {
      *result = ZX_ERR_INTERNAL;
      return;
    }
  }
}

}  // namespace

TEST(RestrictedModeUnified, Basic) {
  // Create the shared process that will run restricted mode.
  static constexpr char kProcName[] = "restricted-shared-test";
  zx::process restricted_proc;
  zx::vmar restricted_vmar;
  ASSERT_OK(zx_process_create_shared(zx_process_self(), 0, kProcName, sizeof(kProcName),
                                     restricted_proc.reset_and_get_address(),
                                     restricted_vmar.reset_and_get_address()));

  // Set up the restricted mode stack.
  zx::vmo stack;
  const uint32_t stack_size = zx_system_get_page_size();
  zx::result<zx_vaddr_t> res = SetupStack(restricted_vmar.get(), stack_size, &stack);
  ASSERT_OK(res.status_value());
  zx_vaddr_t stack_addr = res.value();

  // Set up the restricted mode code segment.
  res = SetupCodeSegment(restricted_vmar.get(), write_to_stack_blob());
  ASSERT_OK(res.status_value());
  zx_vaddr_t cs_addr = res.value();

  // Spawn a thread inside the shared process that executes `RunRestrictedMode`. We want this
  // thread to have all of the functionality provided by the C runtime (safe stack, libraries,
  // etc.) but we don't want to have to do this ourselves. We accomplish this by setting the
  // current thread's process handle to the shared process' handle, and then spawning a thread
  // using C++ primitives.
  auto prev_proc = thrd_set_zx_process(restricted_proc.get());
  zx_status_t result = ZX_ERR_INTERNAL;
  std::thread restricted_thread(RunRestrictedMode, cs_addr, stack_addr, std::nullopt, &result);
  thrd_set_zx_process(prev_proc);

  // Wait for the restricted thread to complete.
  restricted_thread.join();
  ASSERT_OK(result);

  // Validate that the restricted mode routine wrote the right value to the stack using a
  // zx_vmo_read.
  uint8_t buffer[8];
  ASSERT_OK(stack.read(buffer, stack_size - 8, 8));
  uint8_t expected[8] = {0xdd, 0xdd, 0xcc, 0xcc, 0xbb, 0xbb, 0xaa, 0xaa};
  for (size_t i = 0; i < sizeof(buffer); i++) {
    EXPECT_EQ(expected[i], buffer[i]);
  }
}

TEST(RestrictedModeUnified, TestRestrictedAccessToSharedRegion) {
  // Create the shared process that will run restricted mode.
  static constexpr char kProcName[] = "restricted-shared-test";
  zx::process restricted_proc;
  zx::vmar restricted_vmar;
  ASSERT_OK(zx_process_create_shared(zx_process_self(), 0, kProcName, sizeof(kProcName),
                                     restricted_proc.reset_and_get_address(),
                                     restricted_vmar.reset_and_get_address()));

  // Set up the restricted mode stack. Note that this stack is in the shared region, so
  // we expect restricted mode to fail with a page fault exception.
  zx::vmo stack;
  const uint32_t stack_size = zx_system_get_page_size();
  zx::result<zx_vaddr_t> res = SetupStack(zx::vmar::root_self()->get(), stack_size, &stack);
  ASSERT_OK(res.status_value());
  zx_vaddr_t stack_addr = res.value();
  auto cleanup_stack = fit::defer([&stack_addr, &stack_size]() {
    zx::vmar::root_self()->unmap(stack_addr - stack_size, stack_size);
  });

  // Set up the restricted mode code segment.
  res = SetupCodeSegment(restricted_vmar.get(), write_to_stack_blob());
  ASSERT_OK(res.status_value());
  zx_vaddr_t cs_addr = res.value();

  // Spawn a thread inside the shared process that executes `RunRestrictedMode`. We want this
  // thread to have all of the functionality provided by the C runtime (safe stack, libraries,
  // etc.) but we don't want to have to do this ourselves. We accomplish this by setting the
  // current thread's process handle to the shared process' handle, and then spawning a thread
  // using C++ primitives.
  auto prev_proc = thrd_set_zx_process(restricted_proc.get());
  zx_status_t result = ZX_ERR_INTERNAL;
  std::thread restricted_thread(RunRestrictedMode, cs_addr, stack_addr,
                                std::optional<zx_excp_type_t>(ZX_EXCP_FATAL_PAGE_FAULT), &result);
  thrd_set_zx_process(prev_proc);

  // Wait for the restricted thread to complete.
  restricted_thread.join();
  ASSERT_OK(result);
}
