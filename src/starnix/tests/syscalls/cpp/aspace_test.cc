// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/mman.h>
#include <unistd.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

#if defined(__x86_64__)
// Source: //zircon/kernel/arch/x86/include/arch/kernel_aspace.h
#define LOWEST_MAPPABLE_ADDRESS ((uintptr_t)0x200000)
#define HIGHEST_MAPPABLE_ADDRESS ((uintptr_t)(1ULL << 46))
#elif defined(__aarch64__)
// Source: //zircon/kernel/arch/arm64/include/arch/kernel_aspace.h
#define LOWEST_MAPPABLE_ADDRESS ((uintptr_t)0x200000)
#define HIGHEST_MAPPABLE_ADDRESS ((uintptr_t)(1ULL << 47))
#elif defined(__arm)
// Arch32 can only address 4GB
#define LOWEST_MAPPABLE_ADDRESS ((uintptr_t)0x200000)
#define HIGHEST_MAPPABLE_ADDRESS ((uintptr_t)(0xffff0000))
// Source: //zircon/kernel/arch/riscv64/include/arch/kernel_aspace.h
#elif defined(__riscv)
// Assuming Sv39 on RISC-V as we do not currently support any other addressing modes.
#define LOWEST_MAPPABLE_ADDRESS ((uintptr_t)0x200000)
#define HIGHEST_MAPPABLE_ADDRESS ((uintptr_t)(1ULL << 37))
#endif

struct TestAddress {
  uintptr_t address;
  bool upper_bound;
};

class AspaceTest : public testing::TestWithParam<TestAddress> {
 protected:
  uintptr_t test_mapping_base_address() const {
    if (GetParam().upper_bound) {
      // The page containing the upper bound of the address space starts one page before
      // the upper bound address.
      const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
      return GetParam().address - page_size;
    } else {
      return GetParam().address;
    }
  }
};

TEST_P(AspaceTest, RangeFault) {
  // Create a read-only mapping at the target address.
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* mapped = mmap(reinterpret_cast<void*>(test_mapping_base_address()), page_size, PROT_READ,
                      MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, 0, 0);
  ASSERT_NE(mapped, MAP_FAILED);
  int pipefd[2];
  SAFE_SYSCALL(pipe(pipefd));
  char buf[] = {'a'};
  write(pipefd[1], buf, sizeof(buf));
  // Induce a kernel-mode write to the mapped memory.
  // This should trigger a page fault caught by Starnix.
  EXPECT_EQ(read(pipefd[0], mapped, 1), -1);
  EXPECT_EQ(errno, EFAULT);
  // Reads from this address should work fine.
  EXPECT_EQ(write(pipefd[1], mapped, 1), 1);
}

INSTANTIATE_TEST_SUITE_P(LowestAddress, AspaceTest,
                         testing::Values(TestAddress{.address = LOWEST_MAPPABLE_ADDRESS,
                                                     .upper_bound = false}));
INSTANTIATE_TEST_SUITE_P(HighestAddress, AspaceTest,
                         testing::Values(TestAddress{.address = HIGHEST_MAPPABLE_ADDRESS,
                                                     .upper_bound = true}));
