// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/zx/clock.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>

#include <gtest/gtest.h>

namespace {

__attribute__((noinline)) uint64_t count(int n) {
  if (n == 0) {
    return 1;
  }
  return 1 + count(n - 1);
}
__attribute__((noinline)) void add(uint64_t* addr) { *addr = *addr + 1; }
__attribute__((noinline)) void sub(uint64_t* addr) { *addr = *addr - 1; }
__attribute__((noinline)) void collatz(uint64_t* addr) {
  switch (*addr % 2) {
    case 0:
      *addr /= 2;
      break;
    case 1:
      *addr *= 3;
      *addr += 1;
      break;
  }
}

void MakeWork(uint64_t* ptr, async_dispatcher_t* dispatcher, zx::time until) {
  if (zx::clock::get_monotonic() > until) {
    return;
  }
  for (unsigned i = 0; i < 1000000; i++) {
    switch (rand() % 4) {
      case 0:
        add(ptr);
        break;
      case 1:
        sub(ptr);
        break;
      case 2:
        collatz(ptr);
        break;
      case 3:
        *ptr = count(20);
        break;
    }
  }
  EXPECT_EQ(1, 1);
  async::PostTask(dispatcher, [ptr, dispatcher, until]() { MakeWork(ptr, dispatcher, until); });
}

// A simple test case that spins for a few seconds so we can profile it.
//
// This isn't intended as a standalone test to verify correctness, but as a simple target to point
// the profiler at to ensure it is able to profile a test.
TEST(GtestTest, MakeWorkTest) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();

  // Map a vmo and write random data to it
  zx::vmo vmo;
  zx_status_t result = zx::vmo::create(zx_system_get_page_size(), 0, &vmo);
  if (result != ZX_OK) {
    return;
  }
  zx_vaddr_t out;
  result = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                      zx_system_get_page_size(), &out);
  if (result != ZX_OK) {
    return;
  }
  uint64_t* ptr = reinterpret_cast<uint64_t*>(out);
  *ptr = 0;

  zx::time until = zx::deadline_after(zx::sec(5));
  async::PostTask(dispatcher, [ptr, dispatcher, until]() { MakeWork(ptr, dispatcher, until); });
  loop.RunUntilIdle();
}

}  // namespace
