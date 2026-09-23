// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cpuid.h>
#include <lib/zx/result.h>
#include <pthread.h>
#include <x86intrin.h>
#include <zircon/syscalls.h>

#include <thread>

#include <zxtest/zxtest.h>

namespace {

// Returns whether the CPU supports the {rd,wr}{fs,gs}base instructions.
bool x86_feature_fsgsbase() {
  uint32_t eax, ebx, ecx, edx;
  __cpuid_count(7, 0, eax, ebx, ecx, edx);
  return ebx & bit_FSGSBASE;
}

[[gnu::target("fsgsbase")]] void GsBaseTestThread(pthread_barrier_t& barrier, uintptr_t gs_base) {
  uintptr_t fs_base = 0;
  if (x86_feature_fsgsbase()) {
    _writegsbase_u64(gs_base);
    // We don't want to modify fs_base because it is used by libc etc.,
    // but we might as well check that it is also preserved.
    fs_base = _readfsbase_u64();
  }

  // Wait until all the test threads reach this point.
  int rv = pthread_barrier_wait(&barrier);
  EXPECT_TRUE(rv == 0 || rv == PTHREAD_BARRIER_SERIAL_THREAD);

  if (x86_feature_fsgsbase()) {
    EXPECT_EQ(_readgsbase_u64(), gs_base);
    EXPECT_EQ(_readfsbase_u64(), fs_base);
  }
}

// This tests whether the gs_base register on x86 is preserved across
// context switches.
//
// We do this by launching multiple threads that set gs_base to different
// values.  After all the threads have set gs_base, the threads wake up and
// check that gs_base was preserved.
TEST(RegisterStateTest, ContextSwitchOfGsBase) {
  // We run the rest of the test even if the fsgsbase instructions aren't
  // available, so that at least the test's threading logic gets
  // exercised.

  // We launch more threads than there are CPUs.  This ensures that there
  // should be at least one CPU that has >1 of our threads scheduled on
  // it, so saving and restoring gs_base between those threads should get
  // exercised.
  const uint32_t thread_count = zx_system_get_num_cpus() * 2;
  ASSERT_GT(thread_count, 0);

  pthread_barrier_t barrier;
  ASSERT_EQ(pthread_barrier_init(&barrier, nullptr, thread_count), 0);
  {
    // All the threads will be joined when the vector is destroyed.
    std::vector<std::jthread> threads;
    threads.reserve(thread_count);
    for (uint32_t i = 0; i < thread_count; ++i) {
      // Give each thread a different test value for gs_base.
      threads.emplace_back(GsBaseTestThread, std::ref(barrier), i * 0x10004);
    }
  }
  ASSERT_EQ(pthread_barrier_destroy(&barrier), 0);
}

#define DEFINE_REGISTER_ACCESSOR(REG)                                                            \
  [[gnu::no_stack_protector, clang::no_sanitize("safe-stack")]] void set_##REG(uint16_t value) { \
    __asm__ volatile("mov %0, %%" #REG : : "r"(value));                                          \
  }                                                                                              \
  [[gnu::no_stack_protector, clang::no_sanitize("safe-stack")]] uint16_t get_##REG(void) {       \
    uint16_t value;                                                                              \
    __asm__ volatile("mov %%" #REG ", %0" : "=r"(value));                                        \
    return value;                                                                                \
  }

DEFINE_REGISTER_ACCESSOR(ds)
DEFINE_REGISTER_ACCESSOR(es)
DEFINE_REGISTER_ACCESSOR(fs)
DEFINE_REGISTER_ACCESSOR(gs)

#undef DEFINE_REGISTER_ACCESSOR

// This test demonstrates that if the segment selector registers are set to
// 1, they will eventually be reset to 0 when an interrupt occurs.  This is
// mostly a property of the x86 architecture rather than the kernel: The
// IRET instruction has the side effect of resetting these registers when
// returning from the kernel to userland (but not when returning to kernel
// code).
TEST(RegisterContextSwitchTests, SegmentSelectorsZeroedOnInterrupt) {
  ZXTEST_SKIP() << "Disabled because some versions of non-KVM QEMU don't implement IRET fully";

  // We skip setting %fs because that breaks libc's TLS.
  set_ds(1);
  set_es(1);
  set_gs(1);

  // This could be interrupted by an interrupt that causes a context
  // switch, but on an unloaded machine it is more likely to be
  // interrupted by an interrupt where the handler returns without doing
  // a context switch.
  while (get_gs() == 1) {
    __asm__ volatile("pause");
  }

  EXPECT_EQ(get_ds(), 0);
  EXPECT_EQ(get_es(), 0);
  EXPECT_EQ(get_gs(), 0);
}

struct Bases {
  uint64_t fs, gs;
};

// This function can mess with %fs.base, which is part of the Fuchsia Compiler
// ABI.  It's used for both stack-protector and safe-stack, so those must be
// disabled to ensure that nothing will use it, as well as avoiding anything
// using thread_local variables.  No functions compiled with the normal Fuchsia
// Compiler ABI can be called from here, so it calls only the vDSO and the
// helpers above that use the same attributes.
[[gnu::target("fsgsbase"), gnu::no_stack_protector, clang::no_sanitize("safe-stack")]]
zx::result<Bases> TestBasesWithContextSwitches() {
  set_gs(1);
  uint64_t orig_fs_base = 0;
  if (x86_feature_fsgsbase()) {
    orig_fs_base = _readfsbase_u64();
    set_gs(1);
    _writegsbase_u64(1);
    set_fs(1);
    _writefsbase_u64(1);
  }
  set_es(1);
  set_ds(1);

  // Now that all the registers have now been set to 1, sleep repeatedly until
  // the segment selector registers have been cleared. Of course it's possible
  // that a context switch has already occurred and cleared some or all of them
  // so be sure to only terminate the loop once we have observed the last one
  // set (ds) was cleared.
  //
  // Sleeping should cause a context switch away from this thread (to the
  // kernel's idle thread) and another context switch back.
  //
  // Why loop? A single short sleep may not be sufficient to trigger a context
  // switch. By the time this thread has entered the kernel, the duration may
  // have already elapsed.
  //
  // This test is not as precise as we'd like it to be. It is possible that
  // this thread will be interrupted by an interrupt, which would also clear
  // the segment selector registers. Keep the sleep duration short to reduce
  // the chance of that happening.
  zx_duration_mono_t duration = ZX_MSEC(1);
  zx_status_t status = ZX_OK;
  while (get_ds() == 1 && duration < ZX_SEC(10)) {
    status = zx_nanosleep(zx_deadline_after(duration));
    if (status != ZX_OK) {
      break;
    }
    duration *= 2;
  }

  Bases bases;
  if (x86_feature_fsgsbase()) {
    bases = {.fs = _readfsbase_u64(), .gs = _readgsbase_u64()};
    _writefsbase_u64(orig_fs_base);
    _writegsbase_u64(0);
  } else {
    bases = {1, 1};
  }

  return zx::make_result(status, bases);
}

// Test that the kernel also resets the segment selector registers on a
// context switch, to avoid leaking their values and to match what happens
// on an interrupt.
TEST(RegisterContextSwitchTests, SegmentSelectorsZeroedOnContextSwitch) {
  auto result = TestBasesWithContextSwitches();
  ASSERT_OK(result);

  // See that gs_base and fs_base are preserved across a context switch.
  EXPECT_EQ(result->gs, 1);
  EXPECT_EQ(result->fs, 1);

  // See that ds, es, fs, and gs are cleared by a context switch.
  EXPECT_EQ(get_ds(), 0);
  EXPECT_EQ(get_es(), 0);
  EXPECT_EQ(get_fs(), 0);
  EXPECT_EQ(get_gs(), 0);
}

}  // namespace
