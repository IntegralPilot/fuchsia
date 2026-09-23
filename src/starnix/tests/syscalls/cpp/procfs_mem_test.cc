// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <fcntl.h>
#include <lib/fit/defer.h>
#include <string.h>
#include <sys/fsuid.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <unistd.h>

#include <gtest/gtest.h>
#include <linux/capability.h>

#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/proc_test_base.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

class ProcSelfMemProts : public ProcTestBase, public ::testing::WithParamInterface<int> {};

TEST_P(ProcSelfMemProts, CanWriteToPrivateAnonymousMappings) {
  if (access("/proc/self/mem", R_OK | W_OK) == -1) {
    // Host tests run with read-only /proc, so we can't run this test there.
    // See: https://fxbug.dev/328301908
    // TODO(https://fxbug.dev/317285180) don't skip on baseline
    GTEST_SKIP() << "Cannot write to /proc/self/mem";
  }

  uint8_t buf[16] = {0};
  int prot = GetParam();

  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* mapped = mmap(nullptr, page_size, prot, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(mapped, MAP_FAILED) << "mmap: " << std::strerror(errno);
  auto cleanup = fit::defer([mapped, page_size]() { EXPECT_EQ(munmap(mapped, page_size), 0); });

  fbl::unique_fd fd = fbl::unique_fd(open("/proc/self/mem", O_RDWR));
  ASSERT_TRUE(fd.is_valid()) << "open /proc/self/mem: " << std::strerror(errno);

  const off64_t offset = static_cast<off64_t>(reinterpret_cast<uintptr_t>(mapped));
  ASSERT_EQ(lseek64(fd.get(), offset, SEEK_SET), offset) << "lseek: " << std::strerror(errno);

  memset(buf, 'a', sizeof(buf));

  ssize_t n = write(fd.get(), buf, sizeof(buf));
  EXPECT_NE(n, -1) << "write: " << std::strerror(errno);
  EXPECT_EQ(static_cast<size_t>(n), sizeof(buf));

  ASSERT_EQ(mprotect(mapped, page_size, PROT_READ), 0) << "mprotect: " << std::strerror(errno);
  EXPECT_EQ(memcmp(mapped, buf, sizeof(buf)), 0);
}

inline std::string ProtToString(const testing::TestParamInfo<int>& info) {
  std::string prot = "";
  if (info.param == PROT_NONE) {
    return "None";
  }
  if (info.param & PROT_READ) {
    prot += "Read";
  }
  if (info.param & PROT_WRITE) {
    prot += "Write";
  }
  if (info.param & PROT_EXEC) {
    prot += "Execute";
  }
  return prot;
}

INSTANTIATE_TEST_SUITE_P(/* no prefix */, ProcSelfMemProts,
                         ::testing::Values(PROT_NONE, PROT_READ, PROT_WRITE, PROT_EXEC,
                                           PROT_READ | PROT_WRITE, PROT_READ | PROT_EXEC,
                                           PROT_WRITE | PROT_EXEC,
                                           PROT_READ | PROT_WRITE | PROT_EXEC),
                         ProtToString);

TEST_F(ProcTestBase, ProcMemAccessGatedByFsUidSymmetric) {
  if (access("/proc/self/mem", R_OK | W_OK) == -1) {
    GTEST_SKIP() << "Cannot write to /proc/self/mem";
  }

  if (getuid() != 0) {
    GTEST_SKIP() << "This test must be run as root";
  }

  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    test_helper::Rendezvous fork_ready = test_helper::MakeRendezvous();

    // Fork Child 2 (Target B) inside Child 1
    pid_t target_pid = SAFE_SYSCALL(fork());
    if (target_pid == 0) {
      prctl(PR_SET_DUMPABLE, 1);
      test_helper::DropAllCapabilities();
      fork_ready.poker.poke();
      while (true) {
        pause();
      }
      exit(0);
    }

    fork_ready.holder.hold();

    // Now Child 1 (Caller A) downgrades privileges but keeps fsuid=0
    SAFE_SYSCALL(prctl(PR_SET_KEEPCAPS, 1));
    SAFE_SYSCALL(setresuid(1000, 1000, 1000));

    // Restore CAP_SETUID to call setfsuid
    test_helper::SetCapabilityEffective(CAP_SETUID);
    SAFE_SYSCALL(setfsuid(0));

    // Drop caps again
    test_helper::UnsetCapabilityEffective(CAP_SETUID);
    test_helper::UnsetCapabilityEffective(CAP_SYS_PTRACE);

    char path[64];
    snprintf(path, sizeof(path), "/proc/%d/mem", target_pid);

    fbl::unique_fd fd = fbl::unique_fd(open(path, O_RDONLY));
    EXPECT_TRUE(fd.is_valid()) << "open failed: " << strerror(errno);

    // Cleanup Child 2
    test_helper::SetCapabilityEffective(CAP_KILL);
    SAFE_SYSCALL(kill(target_pid, SIGKILL));
    SAFE_SYSCALL(waitpid(target_pid, nullptr, 0));
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_F(ProcTestBase, ProcSelfPagemapMappedAndUnmapped) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  const size_t num_pages = 4;
  const size_t mapping_len = num_pages * page_size;

  auto mapped = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, mapping_len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));

  // Touch each page so real Linux marks them present in CPU page tables.
  volatile char* p = static_cast<volatile char*>(mapped.mapping());
  for (size_t i = 0; i < num_pages; ++i) {
    p[i * page_size] = static_cast<char>(i + 1);
  }

  fbl::unique_fd fd = fbl::unique_fd(open("/proc/self/pagemap", O_RDONLY));
  ASSERT_TRUE(fd.is_valid()) << "open /proc/self/pagemap: " << std::strerror(errno);

  const uintptr_t vaddr = reinterpret_cast<uintptr_t>(mapped.mapping());
  const off64_t pagemap_offset = static_cast<off64_t>((vaddr / page_size) * sizeof(uint64_t));

  std::vector<uint64_t> entries(num_pages);
  ssize_t bytes_read =
      pread64(fd.get(), entries.data(), num_pages * sizeof(uint64_t), pagemap_offset);
  ASSERT_EQ(bytes_read, static_cast<ssize_t>(num_pages * sizeof(uint64_t)))
      << "pread64: " << std::strerror(errno);

  const uint64_t kPresentBit = 1ULL << 63;
  const uint64_t kPfnMask = (1ULL << 55) - 1;

  for (size_t i = 0; i < num_pages; ++i) {
    uint64_t entry = entries[i];
    EXPECT_TRUE(entry & kPresentBit) << "Page " << i << " should have bit 63 (present) set";
  }

  if (test_helper::HasCapability(CAP_SYS_ADMIN)) {
    for (size_t i = 0; i < num_pages; ++i) {
      EXPECT_NE(entries[i] & kPfnMask, 0ULL)
          << "Page " << i << " should have non-zero PFN when CAP_SYS_ADMIN is held";
    }
  }

  // Verify unmapped memory reads as zeros.
  // Find a free/unmapped page address by mmapping and immediately unmapping it.
  uintptr_t unmapped_vaddr;
  {
    auto temp = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
        nullptr, page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    unmapped_vaddr = reinterpret_cast<uintptr_t>(temp.mapping());
  }

  off64_t unmapped_offset = static_cast<off64_t>((unmapped_vaddr / page_size) * sizeof(uint64_t));
  uint64_t unmapped_entry = 0xdeadbeef;
  bytes_read = pread64(fd.get(), &unmapped_entry, sizeof(unmapped_entry), unmapped_offset);
  EXPECT_EQ(bytes_read, static_cast<ssize_t>(sizeof(unmapped_entry)));
  EXPECT_EQ(unmapped_entry, 0ULL) << "Unmapped address should read back 0 from pagemap";
}

TEST_F(ProcTestBase, ProcSelfPagemapCapabilityGating) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  auto mapped = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));

  // Touch page so Linux commits it.
  *static_cast<volatile char*>(mapped.mapping()) = 1;

  const uintptr_t vaddr = reinterpret_cast<uintptr_t>(mapped.mapping());
  const off64_t pagemap_offset = static_cast<off64_t>((vaddr / page_size) * sizeof(uint64_t));
  const uint64_t kPfnMask = (1ULL << 55) - 1;

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // Unprivileged child drops all capabilities (including CAP_SYS_ADMIN).
    test_helper::DropAllCapabilities();

    fbl::unique_fd fd = fbl::unique_fd(open("/proc/self/pagemap", O_RDONLY));
    ASSERT_TRUE(fd.is_valid()) << "open /proc/self/pagemap in child: " << std::strerror(errno);

    uint64_t entry = 0;
    ssize_t bytes_read = pread64(fd.get(), &entry, sizeof(entry), pagemap_offset);
    ASSERT_EQ(bytes_read, static_cast<ssize_t>(sizeof(entry)));

    // Bit 63 should still be present, but PFN (bits 0..54) must be 0 for unprivileged tasks.
    EXPECT_TRUE(entry & (1ULL << 63)) << "Present bit should be visible to unprivileged tasks";
    EXPECT_EQ(entry & kPfnMask, 0ULL) << "PFN must be zeroed when CAP_SYS_ADMIN is absent";
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_F(ProcTestBase, ProcSelfPagemapSharedMappingDeduplication) {
  if (!test_helper::HasCapability(CAP_SYS_ADMIN)) {
    GTEST_SKIP() << "Needs CAP_SYS_ADMIN to inspect PFNs";
  }

  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

  // Create a shared memory fd (memfd).
  int memfd = SAFE_SYSCALL(memfd_create("pagemap_test_shm", 0));
  auto close_fd = fit::defer([memfd]() { close(memfd); });
  ASSERT_EQ(ftruncate(memfd, page_size), 0);

  // Map the same shared page at two different virtual addresses.
  auto map1 = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, page_size, PROT_READ | PROT_WRITE, MAP_SHARED, memfd, 0));
  auto map2 = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, page_size, PROT_READ | PROT_WRITE, MAP_SHARED, memfd, 0));

  ASSERT_NE(map1.mapping(), map2.mapping());

  // Touch both virtual mappings so Linux installs PTEs in page tables for both.
  *static_cast<volatile char*>(map1.mapping()) = 42;
  *static_cast<volatile char*>(map2.mapping()) = 42;

  fbl::unique_fd fd = fbl::unique_fd(open("/proc/self/pagemap", O_RDONLY));
  ASSERT_TRUE(fd.is_valid());

  uint64_t entry1 = 0, entry2 = 0;
  off64_t offset1 = static_cast<off64_t>((reinterpret_cast<uintptr_t>(map1.mapping()) / page_size) *
                                         sizeof(uint64_t));
  off64_t offset2 = static_cast<off64_t>((reinterpret_cast<uintptr_t>(map2.mapping()) / page_size) *
                                         sizeof(uint64_t));

  ASSERT_EQ(pread64(fd.get(), &entry1, sizeof(entry1), offset1),
            static_cast<ssize_t>(sizeof(entry1)));
  ASSERT_EQ(pread64(fd.get(), &entry2, sizeof(entry2), offset2),
            static_cast<ssize_t>(sizeof(entry2)));

  const uint64_t kPfnMask = (1ULL << 55) - 1;

  // Both mappings share the same underlying memory at offset 0, so their PFNs must match.
  EXPECT_EQ(entry1 & kPfnMask, entry2 & kPfnMask)
      << "Shared memory mapped at two virtual addresses must have identical PFNs";
}

TEST_F(ProcTestBase, ProcSelfPagemapMultipleMappingsAndHoles) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

  // Reserve 5 pages of address space to ensure a contiguous virtual layout.
  auto reservation = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      nullptr, 5 * page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));

  uintptr_t base = reinterpret_cast<uintptr_t>(reservation.mapping());

  // Mapping 1: 2 pages of private anonymous memory at [base, base + 2 * page_size).
  auto map1 = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMMap::MMap(
      reinterpret_cast<void*>(base), 2 * page_size, PROT_READ | PROT_WRITE,
      MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0));

  // Touch pages so Linux installs PTEs.
  static_cast<volatile char*>(map1.mapping())[0] = 1;
  static_cast<volatile char*>(map1.mapping())[page_size] = 2;

  // Unmap the middle page to leave an unmapped hole at [base + 2 * page_size, base + 3 *
  // page_size).
  void* hole = reinterpret_cast<void*>(base + 2 * page_size);
  ASSERT_EQ(munmap(hole, page_size), 0) << "munmap hole: " << std::strerror(errno);

  // Mapping 2: 2 pages of shared memory (memfd) at [base + 3 * page_size, base + 5 * page_size).
  int memfd = SAFE_SYSCALL(memfd_create("pagemap_multi_shm", 0));
  auto close_fd = fit::defer([memfd]() { close(memfd); });
  ASSERT_EQ(ftruncate(memfd, 2 * page_size), 0);

  auto map2 = ASSERT_RESULT_SUCCESS_AND_RETURN(
      test_helper::ScopedMMap::MMap(reinterpret_cast<void*>(base + 3 * page_size), 2 * page_size,
                                    PROT_READ | PROT_WRITE, MAP_SHARED | MAP_FIXED, memfd, 0));

  static_cast<volatile char*>(map2.mapping())[0] = 3;
  static_cast<volatile char*>(map2.mapping())[page_size] = 4;

  fbl::unique_fd fd = fbl::unique_fd(open("/proc/self/pagemap", O_RDONLY));
  ASSERT_TRUE(fd.is_valid()) << "open /proc/self/pagemap: " << std::strerror(errno);

  // Read all 5 pages in a single pread64 call spanning mapping 1, the hole, and mapping 2.
  // This exercises the general path in ProcPagemapFile that handles multiple mappings
  // and unmapped memory.
  const off64_t pagemap_offset = static_cast<off64_t>((base / page_size) * sizeof(uint64_t));
  std::vector<uint64_t> entries(5);
  ssize_t bytes_read =
      pread64(fd.get(), entries.data(), entries.size() * sizeof(uint64_t), pagemap_offset);
  ASSERT_EQ(bytes_read, static_cast<ssize_t>(entries.size() * sizeof(uint64_t)))
      << "pread64: " << std::strerror(errno);

  const uint64_t kPresentBit = 1ULL << 63;
  const uint64_t kPfnMask = (1ULL << 55) - 1;

  // Pages 0 and 1 belong to mapping 1 (anonymous private).
  EXPECT_TRUE(entries[0] & kPresentBit);
  EXPECT_TRUE(entries[1] & kPresentBit);

  // Page 2 is the unmapped hole; must read back 0.
  EXPECT_EQ(entries[2], 0ULL) << "Unmapped hole between mappings should read back 0";

  // Pages 3 and 4 belong to mapping 2 (shared memfd).
  EXPECT_TRUE(entries[3] & kPresentBit);
  EXPECT_TRUE(entries[4] & kPresentBit);

  if (test_helper::HasCapability(CAP_SYS_ADMIN)) {
    EXPECT_NE(entries[0] & kPfnMask, 0ULL);
    EXPECT_NE(entries[1] & kPfnMask, 0ULL);
    EXPECT_EQ(entries[2] & kPfnMask, 0ULL);
    EXPECT_NE(entries[3] & kPfnMask, 0ULL);
    EXPECT_NE(entries[4] & kPfnMask, 0ULL);
  }

  // Also verify reading starting from the unmapped hole and continuing into mapping 2.
  std::vector<uint64_t> hole_entries(3);
  const off64_t hole_offset =
      static_cast<off64_t>(((base + 2 * page_size) / page_size) * sizeof(uint64_t));
  bytes_read =
      pread64(fd.get(), hole_entries.data(), hole_entries.size() * sizeof(uint64_t), hole_offset);
  ASSERT_EQ(bytes_read, static_cast<ssize_t>(hole_entries.size() * sizeof(uint64_t)))
      << "pread64 starting at hole: " << std::strerror(errno);
  EXPECT_EQ(hole_entries[0], 0ULL);
  EXPECT_TRUE(hole_entries[1] & kPresentBit);
  EXPECT_TRUE(hole_entries[2] & kPresentBit);
}

}  // namespace
