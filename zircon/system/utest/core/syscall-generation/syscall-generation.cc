// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/stdcompat/bit.h>
#include <lib/zx/channel.h>
#include <lib/zx/exception.h>
#include <string.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/exception.h>
#include <zircon/testonly-syscalls.h>

#include <thread>

#include <zxtest/zxtest.h>

namespace {

// This does a reinterpret_cast for a function pointer to a system call,
// ensuring that the compiler can't see through and make any assumptions about
// the original system call function's type signature.
template <typename T, typename U>
T* SyscallAs(U* syscall) {
  T* result;
  __asm__("" : "=g"(result) : "0"(syscall));
  return result;
}

TEST(SyscallGenerationTest, Wrapper) {
  ASSERT_EQ(zx_syscall_test_wrapper(1, 2, 3), 6, "syscall_test_wrapper doesn't add up");
  ASSERT_EQ(zx_syscall_test_wrapper(-1, 2, 3), ZX_ERR_INVALID_ARGS,
            "vdso should have checked args");
  ASSERT_EQ(zx_syscall_test_wrapper(10, 20, 30), ZX_ERR_OUT_OF_RANGE,
            "vdso should have checked the return");
}

TEST(SyscallGenerationTest, Syscall) {
  ASSERT_EQ(zx_syscall_test_8(1, 2, 3, 4, 5, 6, 7, 8), 36, "syscall8_test doesn't add up");
  ASSERT_EQ(zx_syscall_test_7(1, 2, 3, 4, 5, 6, 7), 28, "syscall7_test doesn't add up");
  ASSERT_EQ(zx_syscall_test_6(1, 2, 3, 4, 5, 6), 21, "syscall6_test doesn't add up");
  ASSERT_EQ(zx_syscall_test_5(1, 2, 3, 4, 5), 15, "syscall5_test doesn't add up");
  ASSERT_EQ(zx_syscall_test_4(1, 2, 3, 4), 10, "syscall4_test doesn't add up");
  ASSERT_EQ(zx_syscall_test_3(1, 2, 3), 6, "syscall3_test doesn't add up");
  ASSERT_EQ(zx_syscall_test_2(1, 2), 3, "syscall2_test doesn't add up");
  ASSERT_EQ(zx_syscall_test_1(1), 1, "syscall1_test doesn't add up");
  ASSERT_EQ(zx_syscall_test_0(), 0, "syscall0_test doesn't add up");
}

TEST(SyscallGenerationTest, RustWrapper) {
  ASSERT_EQ(zx_syscall_test_rust_wrapper(1, 2, 3), 6, "syscall_test_rust_wrapper doesn't add up");
  ASSERT_EQ(zx_syscall_test_rust_wrapper(-1, 2, 3), ZX_ERR_INVALID_ARGS,
            "vdso should have checked args");
  ASSERT_EQ(zx_syscall_test_rust_wrapper(10, 20, 30), ZX_ERR_OUT_OF_RANGE,
            "vdso should have checked the return");
}

TEST(SyscallGenerationTest, RustSyscall) {
  ASSERT_EQ(zx_syscall_test_rust_0(), 0, "syscall_test_rust_0 doesn't add up");
  ASSERT_EQ(zx_syscall_test_rust_1(1), 1, "syscall_test_rust_1 doesn't add up");
  ASSERT_EQ(zx_syscall_test_rust_2(1, 2), 3, "syscall_test_rust_2 doesn't add up");
  ASSERT_EQ(zx_syscall_test_rust_3(1, 2, 3), 6, "syscall_test_rust_3 doesn't add up");
  ASSERT_EQ(zx_syscall_test_rust_4(1, 2, 3, 4), 10, "syscall_test_rust_4 doesn't add up");
  ASSERT_EQ(zx_syscall_test_rust_5(1, 2, 3, 4, 5), 15, "syscall_test_rust_5 doesn't add up");
  ASSERT_EQ(zx_syscall_test_rust_6(1, 2, 3, 4, 5, 6), 21, "syscall_test_rust_6 doesn't add up");
  ASSERT_EQ(zx_syscall_test_rust_7(1, 2, 3, 4, 5, 6, 7), 28, "syscall_test_rust_7 doesn't add up");
  ASSERT_EQ(zx_syscall_test_rust_8(1, 2, 3, 4, 5, 6, 7, 8), 36,
            "syscall_test_rust_8 doesn't add up");
}

TEST(SyscallGenerationTest, RustPointerSyscalls) {
  int32_t value = 42;
  int32_t out_value = 0;
  ASSERT_OK(zx_syscall_test_rust_inptr(&value, &out_value));
  EXPECT_EQ(42, out_value);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_syscall_test_rust_inptr(nullptr, &out_value));

  int32_t out_ptr = 0;
  ASSERT_OK(zx_syscall_test_rust_outptr(99, &out_ptr));
  EXPECT_EQ(99, out_ptr);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_syscall_test_rust_outptr(99, nullptr));

  int32_t inout_ptr = 15;
  ASSERT_OK(zx_syscall_test_rust_inoutptr(&inout_ptr));
  EXPECT_EQ(30, inout_ptr);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_syscall_test_rust_inoutptr(nullptr));
}

TEST(SyscallGenerationTest, RustHandleSyscall) {
  zx_handle_t handle = 0x1234567b;
  uint32_t out_value = 0;
  ASSERT_OK(zx_syscall_test_rust_handle(handle, &out_value));
  EXPECT_EQ(0x1234567b, out_value);
}

TEST(SyscallGenerationTest, HandleCreateSuccess) {
  zx_handle_t handle = ZX_HANDLE_INVALID;
  ASSERT_OK(zx_syscall_test_handle_create(ZX_OK, &handle));

  EXPECT_NE(ZX_HANDLE_INVALID, handle);
  EXPECT_OK(zx_handle_close(handle));
}

// Catch and swallow the ZX_EXCP_POLICY_CODE_HANDLE_LEAK exception exactly once.
void CatchLeakedHandleException(zx::channel* exception_channel) {
  ASSERT_OK(exception_channel->wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));

  // Read the exception out of the channel and verify it's a policy error.
  uint32_t actual_num_handles, actual_num_bytes;
  zx::exception exception;
  zx_exception_info_t info;
  ASSERT_OK(exception_channel->read(0, &info, exception.reset_and_get_address(), sizeof(info), 1,
                                    &actual_num_bytes, &actual_num_handles));
  EXPECT_TRUE(exception.is_valid());
  EXPECT_EQ(actual_num_bytes, sizeof(info));
  EXPECT_EQ(actual_num_handles, 1);

  // Get the exception report in order to check the synth code.
  zx::thread test_thread;
  ASSERT_OK(exception.get_thread(&test_thread));
  size_t actual;
  size_t avail;
  zx_exception_report_t report;
  ASSERT_OK(test_thread.get_info(ZX_INFO_THREAD_EXCEPTION_REPORT, &report,
                                 sizeof(zx_exception_report_t), &actual, &avail));
  EXPECT_EQ(actual, avail);
  EXPECT_EQ(actual, 1);

  ASSERT_TRUE(info.type == ZX_EXCP_POLICY_ERROR &&
              report.context.synth_code == ZX_EXCP_POLICY_CODE_HANDLE_LEAK);

  constexpr uint32_t kExceptionState = ZX_EXCEPTION_STATE_HANDLED;
  ASSERT_OK(
      exception.set_property(ZX_PROP_EXCEPTION_STATE, &kExceptionState, sizeof(kExceptionState)));
}

TEST(SyscallGenerationTest, HandleCreateFailure) {
  zx_handle_t handle = ZX_HANDLE_INVALID;
  ASSERT_EQ(ZX_ERR_UNAVAILABLE, zx_syscall_test_handle_create(ZX_ERR_UNAVAILABLE, &handle));

  // Returning a non-OK status from the syscall should prevent the abigen
  // wrapper from copying handles out.
  EXPECT_EQ(ZX_HANDLE_INVALID, handle);
}

TEST(SyscallGenerationTest, HandleCopyoutFailure) {
  // Create an exception handler to swallow up leaked handle exceptions.
  zx::channel exception_channel;
  ASSERT_OK(zx::thread::self()->create_exception_channel(0, &exception_channel));
  std::thread exception_handler(CatchLeakedHandleException, &exception_channel);

  // Attempting to copy out to nullptr will result in a ZX_ERR_INVALID_ARGS.
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_syscall_test_handle_create(ZX_OK, nullptr));
  exception_handler.join();
}

// zx_syscall_test_widening_* take four args of 64-bit, 32-bit, 16-bit, and
// 8-bit types, respectively.  The actual calling convention will use a 64-bit
// register for each of these arguments, with varying definitions per machine
// ABI about whose responsibility it is to zero-extend or sign-extend the low
// bits of the register.  So here we call each syscall as if its arguments were
// all full 64-bit values.  The kernel cannot safely assume anything about the
// high bits in argument registers for narrower-typed arguments.  So regardless
// of what the machine ABI says, we set extra high bits to ensure the kernel
// ignores them.  The *_narrow and *_wide syscalls differ in how the kernel's
// source code uses the values that the compiler could treat differently so as
// to cover more permutations of risky code generation possibilities.

using WidenedUnsignedArgs = uint64_t(uint64_t, uint64_t, uint64_t, uint64_t);
using WidenedSignedArgs = int64_t(int64_t, int64_t, int64_t, int64_t);

TEST(SyscallGenerationTest, WideningUnsignedNarrow) {
  const auto syscall = SyscallAs<WidenedUnsignedArgs>(&_zx_syscall_test_widening_unsigned_narrow);
  constexpr uint64_t k64 = (uint64_t{1} << 33) | 1;
  constexpr uint64_t k32 = (uint64_t{1} << 33) | 2;
  constexpr uint64_t k16 = (uint64_t{1} << 17) | 3;
  constexpr uint64_t k8 = (uint64_t{1} << 9) | 4;
  EXPECT_EQ(static_cast<uint64_t>(k64) + static_cast<uint32_t>(k32) +  //
                static_cast<uint16_t>(k16) + static_cast<uint8_t>(k8),
            syscall(k64, k32, k16, k8));
}

TEST(SyscallGenerationTest, WideningUnsignedWide) {
  const auto syscall = SyscallAs<WidenedUnsignedArgs>(&_zx_syscall_test_widening_unsigned_wide);
  constexpr uint64_t k64 = (uint64_t{1} << 33) | 1;
  constexpr uint64_t k32 = (uint64_t{1} << 33) | 2;
  constexpr uint64_t k16 = (uint64_t{1} << 17) | 3;
  constexpr uint64_t k8 = (uint64_t{1} << 9) | 4;
  EXPECT_EQ(static_cast<uint64_t>(k64) + static_cast<uint32_t>(k32) +  //
                static_cast<uint16_t>(k16) + static_cast<uint8_t>(k8),
            syscall(k64, k32, k16, k8));
}

TEST(SyscallGenerationTest, WideningSignedNarrow) {
  const auto syscall = SyscallAs<WidenedSignedArgs>(&_zx_syscall_test_widening_signed_narrow);
  constexpr int64_t k64s = -(int64_t{1} << 33);
  constexpr uint64_t k32s = (uint64_t{1} << 33) | cpp20::bit_cast<uint32_t>(int32_t{-2});
  constexpr uint64_t k16s = (uint64_t{1} << 17) | cpp20::bit_cast<uint16_t>(int16_t{-3});
  constexpr uint64_t k8s = (uint64_t{1} << 9) | cpp20::bit_cast<uint8_t>(int8_t{-4});
  EXPECT_EQ(static_cast<int64_t>(k64s) + static_cast<int32_t>(k32s) +  //
                static_cast<int16_t>(k16s) + static_cast<int8_t>(k8s),
            syscall(k64s, k32s, k16s, k8s));
}

TEST(SyscallGenerationTest, WideningSignedWide) {
  const auto syscall = SyscallAs<WidenedSignedArgs>(&_zx_syscall_test_widening_signed_wide);
  constexpr int64_t k64s = -(int64_t{1} << 33);
  constexpr uint64_t k32s = (uint64_t{1} << 33) | cpp20::bit_cast<uint32_t>(int32_t{-2});
  constexpr uint64_t k16s = (uint64_t{1} << 17) | cpp20::bit_cast<uint16_t>(int16_t{-3});
  constexpr uint64_t k8s = (uint64_t{1} << 9) | cpp20::bit_cast<uint8_t>(int8_t{-4});
  EXPECT_EQ(static_cast<int64_t>(k64s) + static_cast<int32_t>(k32s) +  //
                static_cast<int16_t>(k16s) + static_cast<int8_t>(k8s),
            syscall(k64s, k32s, k16s, k8s));
}

}  // namespace
