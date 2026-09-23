// Copyright 2025 The Fuchsia Authors. All rights reserved.
//
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test-thread.h"

#include <lib/elfldltl/machine.h>
#include <lib/zx/vmo.h>

#include <zxtest/zxtest.h>

TestThread::~TestThread() {
  if (thread_) {
    // Make sure the thread is dead and gone before the stack is unmapped.
    Wait();
  }
  if (stack_) {
    // Destroy the whole VMAR to unmap the stack and guard area.
    // Nothing else kept the stack VMO alive, so it dies now too.
    EXPECT_OK(stack_.destroy());
  }
}

void TestThread::Wait() {
  if (thread_) {
    zx_signals_t pending;
    ASSERT_OK(thread_.wait_one(ZX_THREAD_TERMINATED, zx::time::infinite(), &pending));
    ASSERT_TRUE(pending & ZX_THREAD_TERMINATED, "%#x", pending);
  }
}

void TestThread::Init(std::string_view name, zx::unowned_vmo stack_vmo, zx::unowned_process process,
                      zx::unowned_vmar vmar, size_t stack_size, size_t guard_size) {
  ASSERT_OK(
      zx::thread::create(*process, name.data(), static_cast<uint32_t>(name.size()), 0, &thread_));

  zx::vmo anonymous_stack;
  if (!stack_vmo->is_valid()) {
    ASSERT_OK(zx::vmo::create(stack_size, 0, &anonymous_stack));
    stack_vmo = anonymous_stack.borrow();
  }

  uintptr_t stack_base;
  ASSERT_OK(vmar->allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
                           guard_size + stack_size, &stack_, &stack_base));
  ASSERT_OK(stack_.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, guard_size, *stack_vmo,
                       0, stack_size, &stack_base));
  sp_ = elfldltl::AbiTraits<>::InitialStackPointer(stack_base, stack_size);
}

void TestThread::Start(uintptr_t entry, uintptr_t arg) {
  ASSERT_OK(thread_.start(entry, sp_, arg, 0));
}
