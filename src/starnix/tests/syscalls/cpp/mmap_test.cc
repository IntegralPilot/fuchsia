// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <inttypes.h>
#include <lib/fit/defer.h>
#include <lib/stdcompat/string_view.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/inotify.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <unistd.h>

#include <atomic>
#include <charconv>
#include <cstdint>
#include <cstdio>
#include <optional>
#include <ostream>
#include <string>
#include <thread>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/split_string.h"
#include "src/lib/fxl/strings/string_number_conversions.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/proc_test_base.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

constexpr size_t PAGE_SIZE = 0x1000;

#ifndef MAP_FIXED_NOREPLACE
#define MAP_FIXED_NOREPLACE 0x100000
#endif

#ifndef MREMAP_DONTUNMAP
#define MREMAP_DONTUNMAP 4
#endif

namespace {

#if __x86_64__

constexpr size_t MMAP_FILE_SIZE = 64;
constexpr intptr_t LIMIT_4GB = 0x80000000;

TEST(MmapTest, UnmapPartialMapped) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  uint8_t* mmap_addr = reinterpret_cast<uint8_t*>(
      mmap(nullptr, page_size * 2, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  ASSERT_NE(mmap_addr, MAP_FAILED) << strerror(errno);

  EXPECT_EQ(munmap(mmap_addr, page_size), 0) << strerror(errno);
  EXPECT_EQ(munmap(mmap_addr + page_size, page_size), 0) << strerror(errno);
}

TEST(MmapTest, Map32Test) {
  char* tmp = getenv("TEST_TMPDIR");
  std::string path = tmp == nullptr ? "/tmp/mmaptest" : std::string(tmp) + "/mmaptest";
  int fd = open(path.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0777);
  ASSERT_GE(fd, 0);
  for (unsigned char i = 0; i < MMAP_FILE_SIZE; i++) {
    ASSERT_EQ(write(fd, &i, sizeof(i)), 1);
  }
  close(fd);

  int fdm = open(path.c_str(), O_RDWR);
  ASSERT_GE(fdm, 0);

  void* mapped =
      mmap(nullptr, MMAP_FILE_SIZE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_32BIT, fdm, 0);
  intptr_t maploc = reinterpret_cast<intptr_t>(mapped);
  intptr_t limit = LIMIT_4GB - MMAP_FILE_SIZE;
  ASSERT_GT(maploc, 0);
  ASSERT_LE(maploc, limit);

  ASSERT_EQ(munmap(mapped, MMAP_FILE_SIZE), 0);
  close(fd);

  unlink(path.c_str());
}
#endif

TEST(MmapTest, MprotectMultipleMappings) {
  char* page1 = (char*)mmap(nullptr, PAGE_SIZE * 2, PROT_READ | PROT_WRITE,
                            MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(page1, MAP_FAILED) << strerror(errno);
  char* page2 = (char*)mmap(page1 + PAGE_SIZE, PAGE_SIZE, PROT_READ | PROT_WRITE,
                            MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
  ASSERT_NE(page2, MAP_FAILED) << strerror(errno);
  memset(page1, 'F', PAGE_SIZE * 2);
  // This gets the starnix mapping state out of sync with the real zircon mappings...
  ASSERT_EQ(mprotect(page1, PAGE_SIZE * 2, PROT_READ), 0) << strerror(errno);
  // ...so madvise clears a page that is not mapped.
  ASSERT_EQ(madvise(page2, PAGE_SIZE, MADV_DONTNEED), 0) << strerror(errno);
  ASSERT_EQ(*page1, 'F');
  ASSERT_EQ(*page2, 0);  // would fail
}

TEST(MmapTest, MprotectSecondPageStringRead) {
  char* addr = static_cast<char*>(
      mmap(nullptr, PAGE_SIZE * 2, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));

  mprotect(addr + PAGE_SIZE, PAGE_SIZE, 0);
  strcpy(addr, "/dev/null");
  int fd = open(addr, O_RDONLY);
  EXPECT_NE(fd, -1);
  close(fd);
  munmap(addr, PAGE_SIZE * 2);
}

TEST(MMapTest, MapFileThenGrow) {
  char* tmp = getenv("TEST_TMPDIR");
  std::string path = tmp == nullptr ? "/tmp/mmap_grow_test" : std::string(tmp) + "/mmap_grow_test";
  int fd = open(path.c_str(), O_RDWR | O_CREAT | O_TRUNC, 0777);

  size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

  // Resize the file to be 3 pages long.
  size_t file_size = page_size * 3;
  SAFE_SYSCALL(ftruncate(fd, file_size));

  // Create a file-backed mapping that is 8 pages long.
  size_t mapping_len = page_size * 8;
  std::byte* mapping_addr = static_cast<std::byte*>(
      mmap(nullptr, mapping_len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0));
  ASSERT_NE(mapping_addr, MAP_FAILED);

  // Resize the file to be 6.5 pages long.
  file_size = page_size * 6 + page_size / 2;
  SAFE_SYSCALL(ftruncate(fd, file_size));

  // Stores to the area past the original mapping should be reflected in the underlying file.
  off_t store_offset = page_size * 4;
  *reinterpret_cast<volatile std::byte*>(mapping_addr + store_offset) = std::byte{1};

  SAFE_SYSCALL(msync(mapping_addr + store_offset, page_size, MS_SYNC));
  std::byte file_value;
  SAFE_SYSCALL(pread(fd, &file_value, 1, store_offset));
  EXPECT_EQ(file_value, std::byte{1});

  // Writes to the file past the original mapping should be reflected in the mapping.
  off_t load_offset = page_size * 5;
  std::byte stored_value{2};
  SAFE_SYSCALL(pwrite(fd, &stored_value, 1, load_offset));

  SAFE_SYSCALL(msync(mapping_addr + load_offset, page_size, MS_SYNC));
  std::byte read_value = *reinterpret_cast<volatile std::byte*>(mapping_addr + load_offset);
  EXPECT_EQ(read_value, stored_value);

  // Loads and stores to the page corresponding to the end of the file work, even past the end of
  // the file.
  store_offset = file_size + 16;
  *reinterpret_cast<volatile std::byte*>(mapping_addr + store_offset) = std::byte{3};
  load_offset = store_offset;
  read_value = *reinterpret_cast<volatile std::byte*>(mapping_addr + load_offset);
  EXPECT_EQ(read_value, std::byte{3});

  // Note: https://man7.org/linux/man-pages/man2/mmap.2.html#BUGS says that stores to memory past
  // the end of the file may be visible to other memory mappings of the same file even after the
  // file is closed and unmapped.

  SAFE_SYSCALL(munmap(mapping_addr, mapping_len));

  close(fd);
  unlink(path.c_str());
}

TEST(MMapTest, MapFixedUnalignedFails) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* mmap_addr = mmap(nullptr, page_size * 2, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(mmap_addr, MAP_FAILED);

  void* unaligned_addr = reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(mmap_addr) + 1);

  EXPECT_EQ(
      mmap(unaligned_addr, page_size, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0),
      MAP_FAILED);
  EXPECT_EQ(errno, EINVAL);
}

TEST(MmapTest, FileCreatedWithLessPermsPrivate) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  char* tmp = getenv("TEST_TMPDIR");
  std::string dir = tmp == nullptr ? "/tmp" : std::string(tmp);
  std::string path = dir + "/test_mmap_file_without_perms_for_private";

  int fd = SAFE_SYSCALL(creat(path.c_str(), 0));

  void* addr = mmap(nullptr, page_size, PROT_NONE, MAP_PRIVATE, fd, 0);
  EXPECT_EQ(addr, MAP_FAILED);
  EXPECT_EQ(errno, EACCES);

  SAFE_SYSCALL(close(fd));
  SAFE_SYSCALL(unlink(path.c_str()));
}

TEST(MmapTest, FileCreatedWithLessPermsShared) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  char* tmp = getenv("TEST_TMPDIR");
  std::string dir = tmp == nullptr ? "/tmp" : std::string(tmp);
  std::string path = dir + "/test_mmap_file_without_perms_for_shared";

  int fd = SAFE_SYSCALL(creat(path.c_str(), 0));

  void* addr = mmap(nullptr, page_size, PROT_NONE, MAP_SHARED, fd, 0);
  EXPECT_EQ(addr, MAP_FAILED);
  EXPECT_EQ(errno, EACCES);

  SAFE_SYSCALL(close(fd));
  SAFE_SYSCALL(unlink(path.c_str()));
}

TEST(MmapTest, CannotMmapNoexecAsExecutable) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Test uses mount() but running without sysadmin. Skipping.";
  }

  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  test_helper::ScopedTempDir temp_dir;
  SAFE_SYSCALL(mount(nullptr, temp_dir.path().c_str(), "tmpfs", MS_NOEXEC, nullptr));

  std::string file_path = temp_dir.path() + "/executable_file";
  close(SAFE_SYSCALL(creat(file_path.c_str(), 755)));
  fbl::unique_fd fd(open(file_path.c_str(), O_RDONLY));

  void* res = mmap(nullptr, page_size, PROT_READ | PROT_EXEC, MAP_SHARED, fd.get(), 0);
  EXPECT_EQ(res, MAP_FAILED);
  EXPECT_EQ(errno, EPERM);
  if (res != MAP_FAILED) {
    munmap(res, page_size);
  }
}

TEST(MmapTest, CanMprotectNonExecMmapToExecutable) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Test uses mount() but running without sysadmin. Skipping.";
  }

  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  test_helper::ScopedTempDir temp_dir;
  SAFE_SYSCALL(mount(nullptr, temp_dir.path().c_str(), "tmpfs", 0, nullptr));

  std::string file_path = temp_dir.path() + "/executable_file";
  close(SAFE_SYSCALL(creat(file_path.c_str(), 0755)));
  fbl::unique_fd fd(open(file_path.c_str(), O_RDONLY));

  void* res = mmap(nullptr, page_size, PROT_READ, MAP_SHARED, fd.get(), 0);
  ASSERT_NE(res, MAP_FAILED);

  EXPECT_THAT(mprotect(res, page_size, PROT_READ | PROT_EXEC), SyscallSucceeds());
  munmap(res, page_size);
}

TEST(MmapTest, CannotMprotectNoexecAsExecutable) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Test uses mount() but running without sysadmin. Skipping.";
  }

  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  test_helper::ScopedTempDir temp_dir;
  SAFE_SYSCALL(mount(nullptr, temp_dir.path().c_str(), "tmpfs", MS_NOEXEC, nullptr));

  std::string file_path = temp_dir.path() + "/executable_file";
  close(SAFE_SYSCALL(creat(file_path.c_str(), 0755)));
  fbl::unique_fd fd(open(file_path.c_str(), O_RDONLY));

  void* res = mmap(nullptr, page_size, PROT_READ, MAP_SHARED, fd.get(), 0);
  ASSERT_NE(res, MAP_FAILED);

  EXPECT_THAT(mprotect(res, page_size, PROT_READ | PROT_EXEC), SyscallFailsWithErrno(EACCES));
  munmap(res, page_size);
}

class MMapProcTest : public ProcTestBase {};

TEST_F(MMapProcTest, CommonMappingsHavePathnames) {
  uintptr_t stack_addr = reinterpret_cast<uintptr_t>(__builtin_frame_address(0));
  uintptr_t vdso_addr = static_cast<uintptr_t>(getauxval(AT_SYSINFO_EHDR));

  std::string maps;
  ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/maps", &maps));
  auto stack_mapping = test_helper::find_memory_mapping(stack_addr, maps);
  ASSERT_NE(stack_mapping, std::nullopt);
  EXPECT_EQ(stack_mapping->pathname, "[stack]");

  if (vdso_addr) {
    auto vdso_mapping = test_helper::find_memory_mapping(vdso_addr, maps);
    ASSERT_NE(vdso_mapping, std::nullopt);
    EXPECT_EQ(vdso_mapping->pathname, "[vdso]");
  }
}

TEST_F(MMapProcTest, MapFileWithNewlineInName) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  char* tmp = getenv("TEST_TMPDIR");
  std::string dir = tmp == nullptr ? "/tmp" : std::string(tmp);
  std::string path = dir + "/mmap\nnewline";
  fbl::unique_fd fd = fbl::unique_fd(open(path.c_str(), O_RDWR | O_CREAT | O_TRUNC, 0777));
  ASSERT_TRUE(fd);
  SAFE_SYSCALL(ftruncate(fd.get(), page_size));
  void* p = mmap(nullptr, page_size, PROT_READ, MAP_SHARED, fd.get(), 0);
  std::string address_formatted = fxl::StringPrintf("%8" PRIxPTR, (uintptr_t)p);

  std::string maps;
  ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/maps", &maps));
  auto mapping = test_helper::find_memory_mapping(reinterpret_cast<uintptr_t>(p), maps);
  EXPECT_NE(mapping, std::nullopt);
  EXPECT_EQ(mapping->pathname, dir + "/mmap\\012newline");

  munmap(p, page_size);
  unlink(path.c_str());
}

TEST_F(MMapProcTest, MapDeletedField) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  char* tmp = getenv("TEST_TMPDIR");
  std::string dir = tmp == nullptr ? "/tmp" : std::string(tmp);
  std::string path = dir + "/tmpfile";
  fbl::unique_fd fd = fbl::unique_fd(open(path.c_str(), O_RDWR | O_CREAT | O_TRUNC, 0777));
  ASSERT_TRUE(fd);
  SAFE_SYSCALL(ftruncate(fd.get(), page_size));
  void* p = mmap(nullptr, page_size, PROT_READ, MAP_SHARED, fd.get(), 0);
  std::string address_formatted = fxl::StringPrintf("%8" PRIxPTR, (uintptr_t)p);
  fd.reset();
  unlink(path.c_str());

  std::string maps;
  ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/maps", &maps));
  auto mapping = test_helper::find_memory_mapping(reinterpret_cast<uintptr_t>(p), maps);
  EXPECT_NE(mapping, std::nullopt);
  EXPECT_EQ(mapping->pathname, dir + "/tmpfile (deleted)");

  munmap(p, page_size);
}

TEST_F(MMapProcTest, AdjacentFileMappings) {
  size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  char* tmp = getenv("TEST_TMPDIR");
  std::string dir = tmp == nullptr ? "/tmp" : std::string(tmp);
  std::string path = dir + "/mmap_test";
  int fd = open(path.c_str(), O_RDWR | O_CREAT | O_TRUNC, 0777);
  ASSERT_GE(fd, 0);
  SAFE_SYSCALL(ftruncate(fd, page_size * 2));

  // Find two adjacent available pages in memory.
  void* p = mmap(nullptr, page_size * 2, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(MAP_FAILED, p);
  SAFE_SYSCALL(munmap(p, page_size * 2));

  // Map the first page of the file into the first page of our available space.
  ASSERT_NE(MAP_FAILED, mmap(p, page_size, PROT_READ, MAP_SHARED | MAP_FIXED, fd, 0));
  // Map the second page of the file into the second page of our available space.
  ASSERT_NE(MAP_FAILED, mmap((void*)((intptr_t)p + page_size), page_size, PROT_READ,
                             MAP_SHARED | MAP_FIXED, fd, page_size));
  std::string maps;
  ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/maps", &maps));

  // Expect one line for this file covering 2 pages

  std::vector<std::string_view> lines =
      fxl::SplitString(maps, "\n", fxl::kKeepWhitespace, fxl::kSplitWantNonEmpty);
  bool found_entry = false;
  for (auto line : lines) {
    if (cpp20::ends_with(line, path)) {
      EXPECT_FALSE(found_entry) << "extra entry found: " << line;
      found_entry = true;
    }
  }

  close(fd);
  unlink(path.c_str());
}

TEST_F(MMapProcTest, OrderOfLayout) {
  size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  static int anchor;
  uintptr_t executable_addr = reinterpret_cast<uintptr_t>(&anchor);
  uintptr_t program_break = reinterpret_cast<uintptr_t>(sbrk(0));
  uintptr_t mmap_general_addr = reinterpret_cast<uintptr_t>(
      mmap(nullptr, page_size, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
  ASSERT_NE((void*)mmap_general_addr, MAP_FAILED);
  uintptr_t stack_addr = reinterpret_cast<uintptr_t>(__builtin_frame_address(0));

  EXPECT_LT(executable_addr, program_break);
  EXPECT_LT(program_break, mmap_general_addr);
  EXPECT_LT(mmap_general_addr, stack_addr);
  SAFE_SYSCALL(munmap((void*)mmap_general_addr, page_size));
}

TEST_F(MMapProcTest, MremapDontUnmapKeepsFlags) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

  // Reserve 5 pages with no protection and use this range for the new mappings. This is to ensure
  // the mappings will not be merged with anything else.
  void* reserved = mmap(nullptr, 5 * page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(reserved, MAP_FAILED);

  void* source = reinterpret_cast<void*>(reinterpret_cast<intptr_t>(reserved) + page_size);
  void* dest = reinterpret_cast<void*>(reinterpret_cast<intptr_t>(reserved) + 3 * page_size);

  source = mmap(source, page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
                -1, 0);
  ASSERT_NE(source, MAP_FAILED);
  reinterpret_cast<volatile char*>(source)[1] = 'a';

  void* remapped =
      mremap(source, page_size, page_size, MREMAP_MAYMOVE | MREMAP_DONTUNMAP | MREMAP_FIXED, dest);
  EXPECT_NE(remapped, MAP_FAILED);
  ASSERT_EQ(remapped, dest);

  std::string smaps;
  ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/smaps", &smaps));

  auto source_mapping =
      test_helper::find_memory_mapping_ext(reinterpret_cast<uintptr_t>(source), smaps);
  ASSERT_NE(source_mapping, std::nullopt);
  EXPECT_EQ(source_mapping->rss, 0u);

  auto remapped_mapping =
      test_helper::find_memory_mapping_ext(reinterpret_cast<uintptr_t>(remapped), smaps);
  ASSERT_NE(remapped_mapping, std::nullopt);
  EXPECT_NE(remapped_mapping->rss, 0u);

  EXPECT_EQ(source_mapping->vm_flags, remapped_mapping->vm_flags);

  SAFE_SYSCALL(munmap(reserved, 5 * page_size));
}

TEST_F(MMapProcTest, PrivateMemoryOnFork) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  const size_t kSize = 3 * page_size;

  int flags = MAP_ANON | MAP_PRIVATE;
  // Map three pages and mark extra ones as having PROT_NONE to avoid coalescing
  void* mapped = mmap(nullptr, kSize, PROT_READ | PROT_WRITE, flags, -1, 0);
  ASSERT_NE(mapped, nullptr) << "errno=" << errno << ", " << strerror(errno);
  SAFE_SYSCALL(mprotect(mapped, page_size, PROT_NONE));
  SAFE_SYSCALL(
      mprotect(reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(mapped) + 2 * page_size),
               page_size, PROT_NONE));

  // Commit the allocated page by writing some data.
  volatile char* data = reinterpret_cast<char*>(mapped) + page_size;
  data[0] = 42;

  std::string maps;
  ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/smaps", &maps));
  auto base_mapping =
      test_helper::find_memory_mapping_ext(reinterpret_cast<uintptr_t>(mapped) + page_size, maps);
  ASSERT_NE(base_mapping, std::nullopt);
  EXPECT_EQ(base_mapping->pathname, "");
  EXPECT_EQ(base_mapping->perms, "rw-p");
  EXPECT_EQ(base_mapping->rss, page_size / 1024);

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    std::string maps;
    ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/smaps", &maps));
    auto forked_mapping =
        test_helper::find_memory_mapping_ext(reinterpret_cast<uintptr_t>(mapped) + page_size, maps);
    ASSERT_NE(forked_mapping, std::nullopt);
    EXPECT_EQ(forked_mapping->pathname, "");
    EXPECT_EQ(forked_mapping->perms, "rw-p");
    // Rss reflects a full page, not shared with the parent
    EXPECT_EQ(forked_mapping->rss, page_size / 1024);
  });

  SAFE_SYSCALL(munmap(mapped, kSize));
}

TEST_F(MMapProcTest, SmapsRssSplitMappings) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  const size_t kSize = 5 * page_size;

  int flags = MAP_ANON | MAP_PRIVATE;
  // Map 5 pages.
  void* mapped = mmap(nullptr, kSize, PROT_READ | PROT_WRITE, flags, -1, 0);
  ASSERT_NE(mapped, nullptr) << "errno=" << errno << ", " << strerror(errno);

  // Mark pages 0 and 4 as non-readable/non-writable. They act as guard pages to avoid coalescing.
  SAFE_SYSCALL(mprotect(mapped, page_size, PROT_NONE));
  SAFE_SYSCALL(
      mprotect(reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(mapped) + (4 * page_size)),
               page_size, PROT_NONE));

  // Add some attribute that is invisible to Zircon. That will make Starnix see 3 separate mappings
  // where Zircon will only see one.
  SAFE_SYSCALL(
      madvise(reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(mapped) + (2 * page_size)),
              page_size, MADV_WIPEONFORK));

  // Commit pages 1-3 by writing some data.
  for (int i = 1; i < 4; i++) {
    volatile char* data = reinterpret_cast<char*>(mapped) + i * page_size;
    data[0] = 42;
  }

  std::string smaps;
  ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/smaps", &smaps));

  // Check that all the mappings (pages 1-3) are present in smaps, with a non-zero Rss.
  auto first_mapping =
      test_helper::find_memory_mapping_ext(reinterpret_cast<uintptr_t>(mapped) + page_size, smaps);
  ASSERT_NE(first_mapping, std::nullopt);
  EXPECT_EQ(first_mapping->end - first_mapping->start, page_size);
  EXPECT_GT(first_mapping->rss, 0u);
  auto middle_mapping = test_helper::find_memory_mapping_ext(
      reinterpret_cast<uintptr_t>(mapped) + (2 * page_size), smaps);
  ASSERT_NE(middle_mapping, std::nullopt);
  EXPECT_EQ(middle_mapping->end - middle_mapping->start, page_size);
  EXPECT_GT(middle_mapping->rss, 0u);
  auto last_mapping = test_helper::find_memory_mapping_ext(
      reinterpret_cast<uintptr_t>(mapped) + (3 * page_size), smaps);
  ASSERT_NE(last_mapping, std::nullopt);
  EXPECT_EQ(last_mapping->end - last_mapping->start, page_size);
  EXPECT_GT(last_mapping->rss, 0u);

  SAFE_SYSCALL(munmap(mapped, kSize));
}

TEST_F(MMapProcTest, SmapsRssSplitMappingsStrict) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  const size_t kSize = 5 * page_size;

  int flags = MAP_ANON | MAP_PRIVATE;
  // Map 5 pages.
  void* mapped = mmap(nullptr, kSize, PROT_READ | PROT_WRITE, flags, -1, 0);
  ASSERT_NE(mapped, nullptr) << "errno=" << errno << ", " << strerror(errno);

  // Mark pages 0 and 4 as non-readable/non-writable. They act as guard pages to avoid coalescing.
  SAFE_SYSCALL(mprotect(mapped, page_size, PROT_NONE));
  SAFE_SYSCALL(
      mprotect(reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(mapped) + (4 * page_size)),
               page_size, PROT_NONE));

  // Add some attribute that is invisible to Zircon. That will make Starnix see 3 separate mappings
  // where Zircon will only see one.
  SAFE_SYSCALL(
      madvise(reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(mapped) + (2 * page_size)),
              page_size, MADV_WIPEONFORK));

  // Commit the middle page by writing some data.
  volatile char* data = reinterpret_cast<char*>(mapped) + (2 * page_size);
  data[0] = 42;

  std::string smaps;
  ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/smaps", &smaps));

  // Check that all the mappings (pages 1-3) are present in smaps, with a non-zero Rss.
  auto first_mapping =
      test_helper::find_memory_mapping_ext(reinterpret_cast<uintptr_t>(mapped) + page_size, smaps);
  ASSERT_NE(first_mapping, std::nullopt);
  EXPECT_EQ(first_mapping->end - first_mapping->start, page_size);
  // First mapping is uncommitted
  EXPECT_EQ(first_mapping->rss, 0u);
  auto middle_mapping = test_helper::find_memory_mapping_ext(
      reinterpret_cast<uintptr_t>(mapped) + (2 * page_size), smaps);
  ASSERT_NE(middle_mapping, std::nullopt);
  EXPECT_EQ(middle_mapping->end - middle_mapping->start, page_size);
  // Second mapping is committed.
  EXPECT_GT(middle_mapping->rss, 0u);
  auto last_mapping = test_helper::find_memory_mapping_ext(
      reinterpret_cast<uintptr_t>(mapped) + (3 * page_size), smaps);
  ASSERT_NE(last_mapping, std::nullopt);
  EXPECT_EQ(last_mapping->end - last_mapping->start, page_size);
  // Third mapping is uncommitted.
  EXPECT_EQ(last_mapping->rss, 0u);

  SAFE_SYSCALL(munmap(mapped, kSize));
}

class MMapProcStatmTest : public ProcTestBase, public testing::WithParamInterface<int> {
 protected:
  void ReadStatm(size_t* vm_size_out, size_t* rss_size_out) {
    std::string statm;
    ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/statm", &statm));

    auto parts = SplitString(statm, " ", fxl::kTrimWhitespace, fxl::kSplitWantAll);
    EXPECT_EQ(parts.size(), 7U) << statm;

    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    if (vm_size_out) {
      size_t vm_size_pages = 0;
      EXPECT_TRUE(fxl::StringToNumberWithError(parts[0], &vm_size_pages)) << parts[0];
      *vm_size_out = vm_size_pages * page_size;
    }

    if (rss_size_out) {
      size_t rss_size_pages = 0;
      EXPECT_TRUE(fxl::StringToNumberWithError(parts[1], &rss_size_pages)) << parts[1];
      *rss_size_out = rss_size_pages * page_size;
    }
  }
};

TEST_P(MMapProcStatmTest, RssAfterUnmap) {
  const size_t kSize = 4 * 1024 * 1024;

  size_t vm_size_base;
  size_t rss_base;
  ASSERT_NO_FATAL_FAILURE(ReadStatm(&vm_size_base, &rss_base));

  int flags = MAP_ANON | GetParam();
  void* mapped = mmap(nullptr, kSize, PROT_READ | PROT_WRITE, flags, -1, 0);
  ASSERT_NE(mapped, nullptr) << "errno=" << errno << ", " << strerror(errno);

  size_t vm_size_mapped;
  size_t rss_mapped;
  ReadStatm(&vm_size_mapped, &rss_mapped);
  EXPECT_GT(vm_size_mapped, vm_size_base);

  // Commit the allocated pages by writing some data.
  volatile char* data = reinterpret_cast<char*>(mapped);
  size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  for (size_t i = 0; i < kSize; i += page_size) {
    data[i] = 42;
  }

  size_t vm_size_committed;
  size_t rss_committed;
  ReadStatm(&vm_size_committed, &rss_committed);
  EXPECT_GT(vm_size_committed, vm_size_base);
  EXPECT_GT(rss_committed, rss_base);
  EXPECT_GT(rss_committed, rss_mapped);

  // Unmap half of the allocation.
  SAFE_SYSCALL(munmap(mapped, kSize / 2));

  size_t vm_size_unmapped_half;
  size_t rss_unmapped_half;
  ReadStatm(&vm_size_unmapped_half, &rss_unmapped_half);
  EXPECT_GT(vm_size_unmapped_half, vm_size_base);
  EXPECT_LT(vm_size_unmapped_half, vm_size_mapped);
  EXPECT_GT(rss_unmapped_half, rss_mapped);
  EXPECT_LT(rss_unmapped_half, rss_committed);

  // Unmap the rest of the allocation
  SAFE_SYSCALL(munmap(reinterpret_cast<char*>(mapped) + kSize / 2, kSize / 2));
  size_t vm_size_unmapped_all;
  size_t rss_unmapped_all;
  ReadStatm(&vm_size_unmapped_all, &rss_unmapped_all);
  EXPECT_LT(vm_size_unmapped_all, vm_size_unmapped_half);
  EXPECT_LT(rss_unmapped_all, rss_unmapped_half);
}

TEST_P(MMapProcStatmTest, RssAfterMapOverride) {
  const size_t kSize = 4 * 1024 * 1024;

  size_t vm_size_base;
  size_t rss_base;
  ASSERT_NO_FATAL_FAILURE(ReadStatm(&vm_size_base, &rss_base));

  int flags = MAP_ANON | MAP_POPULATE | GetParam();
  void* mapped = mmap(nullptr, kSize, PROT_READ | PROT_WRITE, flags, -1, 0);
  ASSERT_NE(mapped, nullptr) << "errno=" << errno << ", " << strerror(errno);

  size_t vm_size_mapped;
  size_t rss_mapped;
  ReadStatm(&vm_size_mapped, &rss_mapped);
  EXPECT_GT(vm_size_mapped, vm_size_base);
  EXPECT_GT(rss_mapped, rss_base);

  // Map middle the region again without MAP_POPULATE. This should release memory.
  flags = MAP_ANON | MAP_FIXED | GetParam();
  void* remap_addr = reinterpret_cast<char*>(mapped) + kSize / 4;
  void* mapped2 = mmap(remap_addr, kSize / 2, PROT_READ | PROT_WRITE, flags, -1, 0);
  EXPECT_EQ(mapped2, remap_addr);

  size_t vm_size_remapped;
  size_t rss_remapped;
  ReadStatm(&vm_size_remapped, &rss_remapped);
  EXPECT_LT(rss_remapped, rss_mapped);

  munmap(mapped, kSize);
}

INSTANTIATE_TEST_SUITE_P(Private, MMapProcStatmTest, testing::Values(MAP_PRIVATE));
INSTANTIATE_TEST_SUITE_P(Shared, MMapProcStatmTest, testing::Values(MAP_SHARED));

// The initial layout for each test is:
//
// ---- 0x00000000
//  ~~
// ---- lowest_addr_                      - start of the playground area, offset 0
//  ~~
// ---- lowest_guard_region_page          - start of guard region (not a mapping)
// 256 pages
// ---- initial_grows_down_low            - start of MAP_GROWSDOWN mapping at the start of the test
// 2 pages (initially, expected to grow)
// ---- grows_down_high                   - end of MAP_GROWSDOWN mapping
// 16 pages
// ---- highest_addr_                     - end of the playground area, offset playground_size()
class MapGrowsdownTest : public testing::Test {
 protected:
  void SetUp() override {
    page_size_ = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    playground_size_ = 8 * 1024 * page_size_;

    // Find a large portion of unused address space to use in tests.
    void* base_addr =
        mmap(nullptr, playground_size_, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    ASSERT_NE(base_addr, MAP_FAILED) << "mmap failed: " << strerror(errno) << "(" << errno << ")";
    SAFE_SYSCALL(munmap(base_addr, playground_size_));
    lowest_addr_ = static_cast<std::byte*>(base_addr);
    highest_addr_ = lowest_addr_ + playground_size_;

    // Create a new mapping with MAP_GROWSDOWN a bit below the top of the playground.
    initial_grows_down_size_ = 2 * page_size();
    initial_grows_down_low_offset_ = playground_size() - 16 * page_size();

    void* grow_initial_low_address =
        MapRelative(initial_grows_down_low_offset_, initial_grows_down_size_,
                    PROT_READ | PROT_WRITE, MAP_GROWSDOWN);
    ASSERT_NE(grow_initial_low_address, MAP_FAILED)
        << "mmap failed: " << strerror(errno) << "(" << errno << ")";
    ASSERT_EQ(grow_initial_low_address, OffsetToAddress(initial_grows_down_low_offset_));
    grows_down_high_offset_ = initial_grows_down_low_offset_ + initial_grows_down_size_;
  }

  void TearDown() override { SAFE_SYSCALL(munmap(lowest_addr_, playground_size_)); }

  void* MapRelative(size_t offset, size_t len, int prot, int flags) {
    return mmap(OffsetToAddress(offset), len, prot, flags | MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS,
                -1, 0);
  }

  // Tests that a read at |offset| within the playground generates a fault.
  bool TestThatReadSegfaults(intptr_t offset) {
    return TestThatAccessSegfaults(OffsetToAddress(offset), test_helper::AccessType::Read);
  }

  // Tests that a write at |offset| within the playground generates a fault.
  bool TestThatWriteSegfaults(intptr_t offset) {
    return TestThatAccessSegfaults(OffsetToAddress(offset), test_helper::AccessType::Write);
  }

  std::byte* OffsetToAddress(intptr_t offset) { return lowest_addr_ + offset; }

  char ReadAtOffset(intptr_t offset) {
    return std::to_integer<char>(*static_cast<volatile std::byte*>(OffsetToAddress(offset)));
  }

  void WriteAtOffset(intptr_t offset) {
    *static_cast<volatile std::byte*>(OffsetToAddress(offset)) = std::byte{};
  }

  void PrintCurrentMappingsToStderr() {
    std::string maps;
    ASSERT_TRUE(files::ReadFileToString("/proc/self/maps", &maps));
    fprintf(stderr, "Playground area is [%p, %p)\n", lowest_addr_, highest_addr_);
    fprintf(stderr, "MAP_GROWSDOWN region initially mapped to [%p, %p)\n",
            OffsetToAddress(initial_grows_down_low_offset_),
            OffsetToAddress(grows_down_high_offset_));
    fprintf(stderr, "%s\n", maps.c_str());
  }

  size_t page_size() const { return page_size_; }
  size_t playground_size() const { return playground_size_; }

  size_t initial_grows_down_size() const { return initial_grows_down_size_; }
  intptr_t initial_grows_down_low_offset() const { return initial_grows_down_low_offset_; }
  intptr_t grows_down_high_offset() const { return grows_down_high_offset_; }

  std::byte* lowest_addr() const { return lowest_addr_; }
  std::byte* highest_addr() const { return highest_addr_; }

 private:
  size_t page_size_;
  size_t initial_grows_down_size_;
  intptr_t initial_grows_down_low_offset_;
  intptr_t grows_down_high_offset_;
  size_t playground_size_;
  std::byte* lowest_addr_;
  std::byte* highest_addr_;
};

TEST_F(MapGrowsdownTest, Grow) {
  size_t expected_guard_region_size = 256 * page_size();

  // Create a mapping 4 guard page regions below the first mapping to constrain growth.
  size_t gap_to_next_mapping = 4 * expected_guard_region_size;
  intptr_t constraint_offset = initial_grows_down_low_offset() - gap_to_next_mapping;
  void* constraint_mapping = MapRelative(constraint_offset, page_size(), PROT_NONE, 0);
  ASSERT_NE(constraint_mapping, MAP_FAILED)
      << "mmap failed: " << strerror(errno) << "(" << errno << ")";

  // Read from pages sequentially in the guard regions from just below the MAP_GROWSDOWN mapping
  // down to the edge of the second mapping.
  for (size_t i = 0; i < 4 * expected_guard_region_size / page_size(); i += 128) {
    ASSERT_EQ(ReadAtOffset(initial_grows_down_low_offset() - i * page_size()), 0);
  }
  ASSERT_EQ(
      ReadAtOffset(initial_grows_down_low_offset() - 4 * expected_guard_region_size + page_size()),
      0);

  // We should have grown our MAP_GROWSDOWN mapping to touch constraint_mapping. Test by trying to
  // make a new mapping immediately above constraint_mapping with MAP_FIXED_NOREPLACE - this should
  // fail with EXXIST.
  intptr_t test_mapping_offset = constraint_offset + page_size();
  void* desired_test_mapping_address = OffsetToAddress(test_mapping_offset);
  void* rv = mmap(desired_test_mapping_address, page_size(), PROT_READ,
                  MAP_FIXED_NOREPLACE | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  EXPECT_EQ(rv, MAP_FAILED);
  EXPECT_EQ(errno, EEXIST);

  size_t expected_growsdown_final_size = initial_grows_down_size() + 4 * expected_guard_region_size;
  intptr_t final_grows_down_offset = grows_down_high_offset() - expected_growsdown_final_size;
  std::byte* final_grows_down_address = OffsetToAddress(final_grows_down_offset);
  SAFE_SYSCALL(munmap(final_grows_down_address, expected_growsdown_final_size));
  SAFE_SYSCALL(munmap(constraint_mapping, gap_to_next_mapping));
}

TEST_F(MapGrowsdownTest, TouchPageAbove) {
  // The page immediately above the MAP_GROWSDOWN region is unmapped so issuing a read should SEGV.
  ASSERT_TRUE(TestThatReadSegfaults(grows_down_high_offset()));
}

TEST_F(MapGrowsdownTest, TouchHighestGuardRegionPage) {
  intptr_t highest_guard_region_page_offset = initial_grows_down_low_offset() - page_size();
  intptr_t lowest_guard_region_page_offset = highest_guard_region_page_offset - 512 * page_size();

  // Try making a NOREPLACE mapping just below the guard region.
  intptr_t test_offset = lowest_guard_region_page_offset - page_size();
  std::byte* test_address = OffsetToAddress(test_offset);
  void* test_mapping = mmap(test_address, page_size(), PROT_READ,
                            MAP_FIXED_NOREPLACE | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(test_mapping, MAP_FAILED) << "mmap failed: " << strerror(errno) << "(" << errno << ")";
  ASSERT_EQ(test_mapping, test_address);
  SAFE_SYSCALL(munmap(test_mapping, page_size()));

  // Read from the highest guard region page. This should trigger growth of the MAP_GROWSDOWN
  // mapping by one page.
  EXPECT_EQ(ReadAtOffset(test_offset), '\0');

  // Now mapping the page we just touched should fail.
  void* rv = mmap(test_address, page_size(), PROT_READ,
                  MAP_FIXED_NOREPLACE | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_EQ(rv, MAP_FAILED);
  ASSERT_EQ(errno, EEXIST);

  // And we shouldn't be able to make a NOREPLACE mapping in the new guard region.
  test_mapping = mmap(test_address, page_size(), PROT_READ,
                      MAP_FIXED_NOREPLACE | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_EQ(test_mapping, MAP_FAILED);
  ASSERT_EQ(errno, EEXIST);
}

TEST_F(MapGrowsdownTest, MapNoreplaceInGuardRegion) {
  // Make a MAP_GROWSDOWN mapping slightly below the top of the playground area.
  size_t initial_grows_down_size = 2 * page_size();
  intptr_t grow_low_offset = playground_size() - 16 * page_size();

  void* grow_initial_low_address =
      MapRelative(grow_low_offset, initial_grows_down_size, PROT_READ, MAP_GROWSDOWN);
  ASSERT_NE(grow_initial_low_address, MAP_FAILED)
      << "mmap failed: " << strerror(errno) << "(" << errno << ")";
  ASSERT_EQ(grow_initial_low_address, OffsetToAddress(grow_low_offset));

  // The page immediately below grow_low_address is the highest guard page. Try making a new mapping
  // in this region.
  intptr_t highest_guard_region_page_offset = grow_low_offset - page_size();
  std::byte* highest_guard_region_page_address = OffsetToAddress(highest_guard_region_page_offset);
  void* rv = mmap(highest_guard_region_page_address, page_size(), PROT_READ,
                  MAP_FIXED_NOREPLACE | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_EQ(rv, highest_guard_region_page_address);

  // Now that we've mapped something else into the guard region, touching the pages below the new
  // mapping will no longer trigger growth of our MAP_GROWSDOWN section.
  intptr_t test_offset = highest_guard_region_page_offset - page_size();
  ASSERT_TRUE(TestThatReadSegfaults(test_offset));

  // Unmap our mapping in the guard region.
  SAFE_SYSCALL(munmap(highest_guard_region_page_address, page_size()));

  // Now the region is growable again.
  ASSERT_EQ(ReadAtOffset(test_offset), '\0');

  // Since we've grown the region, we can no longer map into what used to be the top of the guard
  // region.
  rv = mmap(highest_guard_region_page_address, page_size(), PROT_READ,
            MAP_FIXED_NOREPLACE | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_EQ(rv, MAP_FAILED);
  ASSERT_EQ(errno, EEXIST);
}

TEST_F(MapGrowsdownTest, MapHintInGuardRegion) {
  // Make a MAP_GROWSDOWN mapping slightly below the top of the playground area.
  size_t initial_grows_down_size = 2 * page_size();
  intptr_t grow_low_offset = playground_size() - 16 * page_size();

  void* grow_initial_low_address =
      MapRelative(grow_low_offset, initial_grows_down_size, PROT_READ, MAP_GROWSDOWN);
  ASSERT_NE(grow_initial_low_address, MAP_FAILED)
      << "mmap failed: " << strerror(errno) << "(" << errno << ")";
  ASSERT_EQ(grow_initial_low_address, OffsetToAddress(grow_low_offset));

  // The page immediately below grow_low_address is the highest guard page. Try making a new mapping
  // in this region.
  intptr_t highest_guard_region_page_offset = grow_low_offset - page_size();
  std::byte* highest_guard_region_page_address = OffsetToAddress(highest_guard_region_page_offset);
  void* rv = mmap(highest_guard_region_page_address, page_size(), PROT_READ,
                  MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(rv, MAP_FAILED);
  ASSERT_NE(rv, highest_guard_region_page_address);

  // Unmap our new mapping, which could have been placed outside the playground.
  SAFE_SYSCALL(munmap(rv, page_size()));
}

TEST_F(MapGrowsdownTest, MprotectBeforeGrow) {
  // Reduce the protection on the low page of the growsdown region to read-only
  SAFE_SYSCALL(mprotect(OffsetToAddress(initial_grows_down_low_offset()), page_size(),
                        PROT_READ | PROT_GROWSDOWN));

  // The high page of the initial region should still be writable.
  WriteAtOffset(grows_down_high_offset() - page_size());

  // Grow the region by touching a page in the guard region.
  intptr_t test_offset = initial_grows_down_low_offset() - page_size();
  ASSERT_EQ(ReadAtOffset(test_offset), 0);

  // The new page should have only PROT_READ protections as the mprotect on the bottom of the
  // growsdown region extends to new pages.
  ASSERT_TRUE(TestThatWriteSegfaults(test_offset));
}

TEST_F(MapGrowsdownTest, MprotectAfterGrow) {
  // Grow the region down by 2 pages by accessing a page in the guard region.
  intptr_t test_offset = initial_grows_down_low_offset() - 2 * page_size();
  ASSERT_EQ(ReadAtOffset(initial_grows_down_low_offset() - 2 * page_size()), 0);

  // Set protection on low page of the initial growsdown region to PROT_NONE | PROT_GROWSDOWN.
  SAFE_SYSCALL(mprotect(OffsetToAddress(initial_grows_down_low_offset()), page_size(),
                        PROT_NONE | PROT_GROWSDOWN));

  // This also changes the protection of pages below the mprotect() region, so we can no longer read
  // at |test_offset|.
  ASSERT_TRUE(TestThatReadSegfaults(test_offset));
}

TEST_F(MapGrowsdownTest, MprotectMixGrowsdownAndRegular) {
  // Grow the region down by 3 pages by accessing a page in the guard region.
  intptr_t test_offset = initial_grows_down_low_offset() - 3 * page_size();
  ASSERT_EQ(ReadAtOffset(test_offset), 0);

  // Now there are 5 pages with protection PROT_READ | PROT_WRITE below grows_down_high_offset().
  // Reduce the protections on the second-lowest page to PROT_READ without the PROT_GROWSDOWN flag.
  // This applies only to the specified range of addresses - one page, in this case.
  SAFE_SYSCALL(mprotect(OffsetToAddress(initial_grows_down_low_offset() - 2 * page_size()),
                        page_size(), PROT_READ));
  // The lowest page of the mapping should still be PROT_READ | PROT_WRITE
  ASSERT_EQ(ReadAtOffset(test_offset), '\0');
  WriteAtOffset(test_offset);

  // Now set the second-highest page to PROT_READ with the MAP_GROWSDOWN flag.
  // Unlike mprotect() without the PROT_GROWSDOWN flag, this protection applies from the specified
  // range down to the next manually specified protection region.
  SAFE_SYSCALL(mprotect(OffsetToAddress(initial_grows_down_low_offset()), page_size(),
                        PROT_READ | PROT_GROWSDOWN));

  // This page and the page below it are now read-only.
  ASSERT_TRUE(TestThatWriteSegfaults(initial_grows_down_low_offset()));
  ASSERT_EQ(ReadAtOffset(initial_grows_down_low_offset()), '\0');

  ASSERT_TRUE(TestThatWriteSegfaults(initial_grows_down_low_offset() - page_size()));
  ASSERT_EQ(ReadAtOffset(initial_grows_down_low_offset() - page_size()), '\0');

  // The lowest page of the mapping should still be PROT_READ | PROT_WRITE.
  WriteAtOffset(test_offset);
}

TEST_F(MapGrowsdownTest, ProtectionAfterGrowWithoutProtGrowsdownFlag) {
  // Reduce protection on the lowest page of the growsdown region to PROT_READ without the
  // PROT_GROWSDOWN flag.
  SAFE_SYSCALL(mprotect(OffsetToAddress(initial_grows_down_low_offset()), page_size(), PROT_READ));

  // Grow the region down by one page with a read.
  intptr_t test_offset = initial_grows_down_low_offset() - page_size();
  ASSERT_EQ(ReadAtOffset(test_offset), '\0');

  // The new page has protections PROT_READ from the bottom of the growsdown region, even though
  // that protection was specified without the PROT_GROWSDOWN flag.
  ASSERT_TRUE(TestThatWriteSegfaults(test_offset));
}

TEST_F(MapGrowsdownTest, MprotectOnAdjacentGrowsdownMapping) {
  // Create a second MAP_GROWSDOWN mapping immediately below the initial mapping with PROT_READ |
  // PROT_WRITE.
  intptr_t second_mapping_offset = initial_grows_down_low_offset() - page_size();
  void* rv = MapRelative(second_mapping_offset, page_size(), PROT_READ | PROT_WRITE, MAP_GROWSDOWN);
  ASSERT_NE(rv, MAP_FAILED) << "mmap failed: " << strerror(errno) << "(" << errno << ")";
  ASSERT_EQ(rv, OffsetToAddress(second_mapping_offset));

  // Reduce protection on top mapping with MAP_GROWSDOWN flag.
  SAFE_SYSCALL(mprotect(OffsetToAddress(initial_grows_down_low_offset()), page_size(),
                        PROT_READ | PROT_GROWSDOWN));

  // Strangely enough, this applies through to the second mapping.
  ASSERT_TRUE(TestThatWriteSegfaults(second_mapping_offset));
}

TEST_F(MapGrowsdownTest, MprotectOnAdjacentNonGrowsdownMappingBelow) {
  // Create a second mapping immediately below the initial mapping with PROT_READ | PROT_WRITE.
  intptr_t second_mapping_offset = initial_grows_down_low_offset() - page_size();
  void* rv = MapRelative(second_mapping_offset, page_size(), PROT_READ | PROT_WRITE, 0);
  ASSERT_NE(rv, MAP_FAILED) << "mmap failed: " << strerror(errno) << "(" << errno << ")";
  ASSERT_EQ(rv, OffsetToAddress(second_mapping_offset));

  // Reduce protection on top mapping with PROT_GROWSDOWN flag.
  SAFE_SYSCALL(mprotect(OffsetToAddress(initial_grows_down_low_offset()), page_size(),
                        PROT_READ | PROT_GROWSDOWN));

  // The protection change does not propagate to the adjacent non-MAP_GROWSDOWN mapping so it's
  // still PROT_READ | PROT_WRITE.
  WriteAtOffset(second_mapping_offset);
}

TEST_F(MapGrowsdownTest, SyscallReadsBelowGrowsdown) {
  // This address is not in any mapping but it is just below a MAP_GROWSDOWN mapping.
  std::byte* address_below_growsdown =
      OffsetToAddress(initial_grows_down_low_offset() - page_size());
  int fds[2];
  SAFE_SYSCALL(pipe(fds));
  // This syscall should grow the region to include the address read from and insert a '\0' into the
  // pipe.
  ASSERT_THAT(write(fds[1], address_below_growsdown, 1), SyscallSucceeds());
  char buf;
  SAFE_SYSCALL(read(fds[0], &buf, 1));
  EXPECT_EQ(buf, '\0');
}

TEST_F(MapGrowsdownTest, SyscallWritesBelowGrowsdown) {
  // This address is not in any mapping but it is just below a MAP_GROWSDOWN mapping.
  std::byte* address_below_growsdown =
      OffsetToAddress(initial_grows_down_low_offset() - page_size());
  int fds[2];
  SAFE_SYSCALL(pipe(fds));
  char buf = 'a';
  ASSERT_THAT(write(fds[1], &buf, 1), SyscallSucceeds());
  // This syscall should grow the region to include the address written to and read an 'a' from the
  // pipe.
  SAFE_SYSCALL(read(fds[0], address_below_growsdown, 1));
  EXPECT_EQ(std::to_integer<char>(*address_below_growsdown), 'a');
}

TEST(Mprotect, ProtGrowsdownOnNonGrowsdownMapping) {
  size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* rv = mmap(nullptr, page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(rv, MAP_FAILED) << "mmap failed: " << strerror(errno) << "(" << errno << ")";
  EXPECT_EQ(mprotect(rv, page_size, PROT_READ | PROT_GROWSDOWN), -1);
  EXPECT_EQ(errno, EINVAL);
}

TEST(Mprotect, UnalignedMprotectEnd) {
  size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* rv = mmap(nullptr, page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(rv, MAP_FAILED) << "mmap failed: " << strerror(errno) << "(" << errno << ")";
  EXPECT_EQ(mprotect(rv, 5, PROT_READ), 0);
}

TEST_F(MMapProcTest, MProtectIsThreadSafe) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    const size_t page_size = sysconf(_SC_PAGE_SIZE);
    void* mmap1 = mmap(nullptr, page_size, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    ASSERT_NE(mmap1, MAP_FAILED);
    uintptr_t addr = reinterpret_cast<uintptr_t>(mmap1);
    ASSERT_TRUE(test_helper::TryRead(addr));
    ASSERT_FALSE(test_helper::TryWrite(addr));

    std::atomic<bool> start = false;
    std::atomic<int> count = 2;

    std::thread protect_rw([addr, &start, &count, page_size]() {
      count -= 1;
      while (!start) {
      }
      ASSERT_EQ(0, mprotect(reinterpret_cast<void*>(addr), page_size, PROT_READ | PROT_WRITE));
    });

    std::thread protect_none([addr, &start, &count, page_size]() {
      count -= 1;
      while (!start) {
      }
      ASSERT_EQ(0, mprotect(reinterpret_cast<void*>(addr), page_size, PROT_NONE));
    });

    while (count != 0) {
    }
    start = true;
    protect_none.join();
    protect_rw.join();

    std::string maps;

    ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/maps", &maps));
    auto mapping = test_helper::find_memory_mapping(addr, maps);
    ASSERT_NE(mapping, std::nullopt);

    std::string perms = mapping->perms;
    ASSERT_FALSE(perms.empty());

    if (cpp20::starts_with(std::string_view(perms), "---p")) {
      // protect_none was the last one. We should not be able to read nor
      // write in this mapping.
      EXPECT_FALSE(test_helper::TryRead(addr));
      EXPECT_FALSE(test_helper::TryWrite(addr));
    } else if (cpp20::starts_with(std::string_view(perms), "rw-p")) {
      // protect_rw was the last one. We should be able to read and write
      // in this mapping.
      EXPECT_TRUE(test_helper::TryRead(addr));
      EXPECT_TRUE(test_helper::TryWrite(addr));
      volatile uint8_t* ptr = reinterpret_cast<volatile uint8_t*>(addr);
      *ptr = 5;
      EXPECT_EQ(*ptr, 5);
    } else {
      ASSERT_FALSE(true) << "invalid perms for mapping: " << perms;
    }
  });
}

TEST(Mprotect, GrowTempFilePermissions) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  char* tmp = getenv("TEST_TMPDIR");
  std::string dir = tmp == nullptr ? "/tmp" : std::string(tmp);
  std::string path = dir + "/grow_temp_file_permissions";
  {
    uint8_t buf[] = {'a'};
    fbl::unique_fd fd = fbl::unique_fd(open(path.c_str(), O_RDWR | O_CREAT | O_TRUNC, 0777));
    ASSERT_TRUE(fd);
    ASSERT_EQ(write(fd.get(), &buf[0], sizeof(buf)), 1) << errno << ": " << strerror(errno);
  }
  ASSERT_EQ(0, chmod(path.c_str(), S_IRUSR | S_IRGRP | S_IROTH));

  std::string before;
  ASSERT_TRUE(files::ReadFileToString(path, &before));

  {
    uint8_t buf[] = {'b'};
    fbl::unique_fd fd = fbl::unique_fd(open(path.c_str(), O_RDONLY));
    ASSERT_EQ(-1, write(fd.get(), buf, sizeof(buf)));

    void* ptr = mmap(nullptr, page_size, PROT_READ, MAP_SHARED, fd.get(), 0);
    EXPECT_NE(ptr, MAP_FAILED);

    EXPECT_NE(mprotect(ptr, page_size, PROT_READ | PROT_WRITE), 0);
    EXPECT_TRUE(test_helper::TestThatAccessSegfaults(ptr, test_helper::AccessType::Write));
  }
  std::string after;
  ASSERT_TRUE(files::ReadFileToString(path, &after));
  EXPECT_EQ(before, after);
  ASSERT_EQ(0, unlink(path.c_str()));
}

TEST_F(MMapProcTest, MprotectFailureIsConsistent) {
  // Test that even if mprotect fails, we either see the new mapping or the old
  // one, and the accesses are consistent with what is reported by the kernel.
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  char* tmp = getenv("TEST_TMPDIR");
  std::string dir = tmp == nullptr ? "/tmp" : std::string(tmp);
  std::string path = dir + "/test_mprotect_consistent_failure";
  {
    uint8_t buf[] = {1};
    fbl::unique_fd fd = fbl::unique_fd(open(path.c_str(), O_RDWR | O_CREAT | O_TRUNC, 0777));
    ASSERT_TRUE(fd);
    ASSERT_EQ(write(fd.get(), &buf[0], sizeof(buf)), 1);
  }
  fbl::unique_fd fd = fbl::unique_fd(open(path.c_str(), O_RDONLY));
  ASSERT_TRUE(fd);

  void* ptr = mmap(0, page_size * 3, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(ptr, MAP_FAILED);
  uintptr_t ptr_addr = reinterpret_cast<uintptr_t>(ptr);

  ASSERT_NE(mmap(reinterpret_cast<void*>(ptr_addr + page_size), page_size, PROT_READ,
                 MAP_SHARED | MAP_FIXED, fd.get(), 0),
            MAP_FAILED);

  ASSERT_NE(mprotect(reinterpret_cast<void*>(ptr_addr), page_size * 3,
                     PROT_READ | PROT_WRITE | PROT_EXEC),
            0);

  std::string maps;
  ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/maps", &maps));

  auto second_page = test_helper::find_memory_mapping(ptr_addr + page_size, maps);
  ASSERT_NE(second_page, std::nullopt);
  EXPECT_EQ(second_page->perms, "r--s");
  EXPECT_TRUE(test_helper::TryRead(ptr_addr + page_size));
  EXPECT_FALSE(test_helper::TryWrite(ptr_addr + page_size));

  auto test_consistency = [](const auto& mapping, uintptr_t addr) {
    auto new_perms = "rwxp";
    auto old_perms = "---p";
    if (mapping->perms == new_perms) {
      EXPECT_TRUE(test_helper::TryRead(addr));
      EXPECT_TRUE(test_helper::TryWrite(addr));

      volatile uint8_t* ptr = reinterpret_cast<volatile uint8_t*>(addr);
      *ptr = 5;
      EXPECT_EQ(*ptr, 5);
    } else if (mapping->perms == old_perms) {
      EXPECT_FALSE(test_helper::TryRead(addr));
      EXPECT_FALSE(test_helper::TryWrite(addr));
    } else {
      ASSERT_FALSE(true) << "invalid perms for mapping: " << mapping->perms;
    }
  };

  auto first_page = test_helper::find_memory_mapping(ptr_addr, maps);
  ASSERT_NE(first_page, std::nullopt);
  test_consistency(first_page, ptr_addr);

  auto third_page = test_helper::find_memory_mapping(ptr_addr + page_size * 2, maps);
  ASSERT_NE(third_page, std::nullopt);
  test_consistency(third_page, ptr_addr + page_size * 2);

  munmap(ptr, page_size * 3);
  unlink(path.c_str());
}

TEST_F(MMapProcTest, MProtectAppliedPartially) {
  // Calls mprotect on a region that contains 3 adjacent mappings:
  // The first and third mapping can be mprotected with RW, but the second can't
  // because it's a mapping of a read-only file.
  // Tests that mprotect fails, but still changes the permissions of the
  // first mapping.

  // Create a file
  test_helper::ScopedTempDir tmp_dir;
  std::string path = tmp_dir.path() + "/test_mprotect_applied_partially";
  fbl::unique_fd fd = fbl::unique_fd(open(path.c_str(), O_RDONLY | O_CREAT | O_TRUNC, 0777));
  ASSERT_TRUE(fd);

  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

  // Find unused address space to hold the 3 adjacent mappings
  char* base_address = nullptr;
  {
    auto mapping = test_helper::ScopedMMap::MMap(nullptr, page_size * 3, PROT_NONE,
                                                 MAP_ANONYMOUS | MAP_PRIVATE, -1, 0);
    EXPECT_TRUE(*mapping);
    base_address = static_cast<char*>(mapping->mapping());
  }

  // Create the 3 adjacent mappings
  auto first_mapping = test_helper::ScopedMMap::MMap(
      base_address, page_size, PROT_NONE, MAP_ANONYMOUS | MAP_PRIVATE | MAP_FIXED, -1, 0);
  auto second_mapping = test_helper::ScopedMMap::MMap(
      base_address + page_size, page_size, PROT_READ, MAP_SHARED | MAP_FIXED, fd.get(), 0);
  auto third_mapping =
      test_helper::ScopedMMap::MMap(base_address + 2 * page_size, page_size, PROT_NONE,
                                    MAP_ANONYMOUS | MAP_PRIVATE | MAP_FIXED, -1, 0);
  ASSERT_TRUE(*first_mapping);
  ASSERT_TRUE(*second_mapping);
  ASSERT_TRUE(*third_mapping);

  // Helper that checks if the permissions of `mapping` match `expected_perms`.
  auto perms_of_mapping_match = [this](test_helper::ScopedMMap& mapping,
                                       std::string expected_perms) -> testing::AssertionResult {
    std::string maps;
    if (!files::ReadFileToString(proc_path() + "/self/maps", &maps)) {
      return testing::AssertionFailure() << "reading /proc/self/maps failed";
    }
    auto first_mapping_report =
        test_helper::find_memory_mapping(reinterpret_cast<uintptr_t>(mapping.mapping()), maps);
    if (!first_mapping_report.has_value()) {
      return testing::AssertionFailure() << "mapping not found in /proc/self/maps";
    }
    if (first_mapping_report->perms != expected_perms) {
      return testing::AssertionFailure()
             << "expected perms " << expected_perms << ", got " << first_mapping_report->perms;
    }
    return testing::AssertionSuccess();
  };

  // Check the permissions before and after `mprotect`.
  EXPECT_TRUE(perms_of_mapping_match(*first_mapping, "---p"));
  EXPECT_TRUE(perms_of_mapping_match(*third_mapping, "---p"));
  errno = 0;
  EXPECT_EQ(mprotect(first_mapping->mapping(), page_size * 3, PROT_READ | PROT_WRITE), -1);
  EXPECT_EQ(errno, EACCES);
  EXPECT_TRUE(perms_of_mapping_match(*first_mapping, "rw-p"));
  EXPECT_TRUE(perms_of_mapping_match(*third_mapping, "---p"));
}

class MMapAllProtectionsTest : public testing::TestWithParam<std::tuple<int, int>> {};

TEST_P(MMapAllProtectionsTest, PrivateFileMappingAllowAllProtections) {
  // Calls with the given protection levels `mmap` with `MAP_PRIVATE` and `mprotect`.
  // Does so over various file descriptors, and expect the calls to succeed.

  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

  test_helper::ScopedTempDir tmp_dir;
  std::string path = tmp_dir.path() + "/private_mapped_file";

  std::vector<fbl::unique_fd> fds;
  fds.emplace_back(test_helper::MemFdCreate("try_read", O_RDONLY));
  fds.emplace_back(open("/proc/self/exe", O_RDONLY));
  fds.emplace_back(open(path.c_str(), O_RDONLY | O_CREAT | O_TRUNC, 0666));

  const auto [mmap_prot, mprotect_flag] = MMapAllProtectionsTest::GetParam();
  for (const auto& fd : fds) {
    ASSERT_TRUE(fd.is_valid());
    auto mapping =
        test_helper::ScopedMMap::MMap(nullptr, page_size, mmap_prot, MAP_PRIVATE, fd.get(), 0);
    EXPECT_THAT(mapping, SyscallResultIsOk());
    if (mapping.is_ok()) {
      auto addr = mapping->mapping();
      EXPECT_EQ(mprotect(addr, page_size, mprotect_flag), 0)
          << "mprotect failed: " << std::strerror(errno);
    }
  }
}

namespace {

std::string ProtectionToString(int prot) {
  std::string result;
  if (prot & PROT_READ) {
    result += "r";
  } else {
    result += "_";
  }
  if (prot & PROT_WRITE) {
    result += "w";
  } else {
    result += "_";
  }
  if (prot & PROT_EXEC) {
    result += "x";
  } else {
    result += "_";
  }
  return result;
}

const auto kAllMmapProtections =
    testing::Values(PROT_READ, PROT_READ | PROT_WRITE, PROT_READ | PROT_EXEC,
                    PROT_READ | PROT_WRITE | PROT_EXEC, PROT_NONE);

}  // namespace

INSTANTIATE_TEST_SUITE_P(MMapAllProtectionsTest, MMapAllProtectionsTest,
                         testing::Combine(kAllMmapProtections, kAllMmapProtections),
                         [](const testing::TestParamInfo<std::tuple<int, int>>& info) {
                           return ProtectionToString(std::get<0>(info.param)) + "_and_" +
                                  ProtectionToString(std::get<1>(info.param));
                         });

bool IsMapped(uintptr_t addr) {
  static const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  int rv = msync(reinterpret_cast<void*>(addr & ~(page_size - 1)), page_size, MS_ASYNC);

  if (rv == 0) {
    return true;
  }
  if (errno != ENOMEM) {
    ADD_FAILURE() << "Unexpected msync error " << errno << " on addr " << addr;
    abort();
  }
  return false;
}

// Creates a mapping 4 pages long:
//  | first_page | second_page | third_page | fourth_page |
//  ^
//  |
//  +--- mapping
//
// Then we mark the first 3 pages as MADV_DONTFORK, undo the annotation on the third page with
// DOFORK, remap the first page to a different location, and create a new mapping at the location
// previously occupied by the first page.
//
//    DONTFORK     DONTFORK                                           DONTFORK              DONTFORK
//  | new page   | second_page | third_page | | fourth_page |  .... | remapped_first_page |
//  remapped_extended | ^                                                               ^ | | | +---
//  remapped
//  |
//  +--- mapping
//
// After forking, in the child process we expect the new mapping and the third page of the original
// mapping to exist in the child. The first page retains its DONTFORK behavior from the madvise()
// call even in its new location. The second page in the remapped location inherits the DONTFORK
// flag from the allocation it is extending. The second page of the original mapping preserves its
// DONTFORK flag from the madvise() call. The remapped first page does not have a DONTFORK flag set
// since it is a new allocation despite it existing in a memory range that had DONTFORK set
// previously.
TEST(Madvise, SetDontForkThenRemap) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* mapping = mmap(nullptr, 4 * page_size, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(mapping, MAP_FAILED);

  SAFE_SYSCALL(madvise(mapping, page_size * 3, MADV_DONTFORK));
  SAFE_SYSCALL(
      madvise(reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(mapping) + page_size * 2),
              page_size, MADV_DOFORK));

  void* remapped = mremap(mapping, page_size, page_size * 2, MREMAP_MAYMOVE);
  ASSERT_NE(remapped, MAP_FAILED);

  void* new_mapping = mmap(mapping, page_size, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
  ASSERT_EQ(new_mapping, mapping);

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    EXPECT_TRUE(IsMapped(reinterpret_cast<uintptr_t>(mapping)));
    EXPECT_TRUE(IsMapped(reinterpret_cast<uintptr_t>(mapping) + 2 * page_size));
    EXPECT_TRUE(IsMapped(reinterpret_cast<uintptr_t>(mapping) + 3 * page_size));
    EXPECT_FALSE(IsMapped(reinterpret_cast<uintptr_t>(mapping) + page_size));
    EXPECT_FALSE(IsMapped(reinterpret_cast<uintptr_t>(remapped)));
    EXPECT_FALSE(IsMapped(reinterpret_cast<uintptr_t>(remapped) + page_size));
  });
  EXPECT_TRUE(helper.WaitForChildren());

  munmap(mapping, 4 * page_size);
  munmap(remapped, 2 * page_size);
}

TEST(Mremap, RemapMayMoveSpanningMappings) {
  if (test_helper::IsKernelVersionAtLeast(6, 17)) {
    GTEST_SKIP() << "Moving multiple mappings is supported in Linux 6.17 and newer, skipping.";
  }
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* mapping =
      mmap(nullptr, 2 * page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(mapping, MAP_FAILED);

  SAFE_SYSCALL(mprotect(mapping, page_size, PROT_READ));

  void* destination =
      mmap(nullptr, 2 * page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(destination, MAP_FAILED);

  SAFE_SYSCALL(munmap(destination, 2 * page_size));

  void* remapped =
      mremap(mapping, 2 * page_size, 2 * page_size, MREMAP_MAYMOVE | MREMAP_FIXED, destination);
  EXPECT_EQ(remapped, MAP_FAILED);
  EXPECT_EQ(errno, EFAULT);

  SAFE_SYSCALL(munmap(mapping, 2 * page_size));
}

TEST(Mremap, BadAddressReturnsEfault) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  // Map and immediately unmap a 2-page region so we can obtain two guaranteed
  // unmapped, non-overlapping addresses (`addr` and `dst`).
  void* base = mmap(nullptr, 2 * page_size, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(base, MAP_FAILED);
  SAFE_SYSCALL(munmap(base, 2 * page_size));

  void* addr = base;
  void* dst = reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(base) + page_size);

  // Expanding unmapped address should return EFAULT.
  EXPECT_EQ(mremap(addr, page_size, 2 * page_size, 0), MAP_FAILED);
  EXPECT_EQ(errno, EFAULT);

  EXPECT_EQ(mremap(addr, page_size, 2 * page_size, MREMAP_MAYMOVE), MAP_FAILED);
  EXPECT_EQ(errno, EFAULT);

  // Shrinking unmapped address should return EFAULT.
  EXPECT_EQ(mremap(addr, 2 * page_size, page_size, 0), MAP_FAILED);
  EXPECT_EQ(errno, EFAULT);

  // Remapping with the same size on unmapped address should return EFAULT.
  EXPECT_EQ(mremap(addr, page_size, page_size, 0), MAP_FAILED);
  EXPECT_EQ(errno, EFAULT);

  // Moving unmapped address to a non-overlapping destination with FIXED should return EFAULT.
  EXPECT_EQ(mremap(addr, page_size, page_size, MREMAP_MAYMOVE | MREMAP_FIXED, dst), MAP_FAILED);
  EXPECT_EQ(errno, EFAULT);

  // Remapping with old_length == 0 on unmapped address should return EFAULT.
  EXPECT_EQ(mremap(addr, 0, page_size, MREMAP_MAYMOVE), MAP_FAILED);
  EXPECT_EQ(errno, EFAULT);

  EXPECT_EQ(mremap(addr, 0, page_size, 0), MAP_FAILED);
  EXPECT_EQ(errno, EFAULT);

  // Unaligned address should return EINVAL, not EFAULT (alignment check happens first).
  void* unaligned_addr = reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(addr) + 1);
  EXPECT_EQ(mremap(unaligned_addr, page_size, 2 * page_size, 0), MAP_FAILED);
  EXPECT_EQ(errno, EINVAL);
}

TEST(Mremap, RemapPartOfMapping) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* mapping =
      mmap(nullptr, 3 * page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(mapping, MAP_FAILED);

  reinterpret_cast<volatile char*>(mapping)[0] = 'a';
  reinterpret_cast<volatile char*>(mapping)[page_size] = 'b';
  reinterpret_cast<volatile char*>(mapping)[2 * page_size] = 'c';

  void* target =
      mmap(nullptr, 3 * page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(target, MAP_FAILED);

  reinterpret_cast<volatile char*>(target)[0] = 'x';
  reinterpret_cast<volatile char*>(target)[page_size] = 'y';
  reinterpret_cast<volatile char*>(target)[2 * page_size] = 'z';

  void* remap_source = reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(mapping) + page_size);
  void* remap_destination =
      reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(target) + page_size);

  void* remapped =
      mremap(remap_source, page_size, page_size, MREMAP_MAYMOVE | MREMAP_FIXED, remap_destination);
  ASSERT_EQ(remapped, remap_destination);

  EXPECT_EQ('a', reinterpret_cast<volatile char*>(mapping)[0]);
  EXPECT_TRUE(test_helper::TestThatAccessSegfaults(static_cast<std::byte*>(mapping) + page_size,
                                                   test_helper::AccessType::Read));
  EXPECT_EQ('c', reinterpret_cast<volatile char*>(mapping)[2 * page_size]);

  EXPECT_EQ('x', reinterpret_cast<volatile char*>(target)[0]);
  EXPECT_EQ('b', reinterpret_cast<volatile char*>(target)[page_size]);
  EXPECT_EQ('z', reinterpret_cast<volatile char*>(target)[2 * page_size]);
}

TEST(Mremap, MremapSharedCopy) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* source =
      mmap(nullptr, page_size, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(source, MAP_FAILED);
  reinterpret_cast<volatile char*>(source)[0] = 'a';

  void* remapped = mremap(source, 0, page_size, MREMAP_MAYMOVE);
  ASSERT_NE(remapped, MAP_FAILED);
  ASSERT_NE(remapped, source);
  EXPECT_EQ('a', reinterpret_cast<volatile char*>(remapped)[0]);
  EXPECT_EQ('a', reinterpret_cast<volatile char*>(source)[0]);

  // Changes are shared
  reinterpret_cast<volatile char*>(remapped)[0] = 'b';
  EXPECT_EQ('b', reinterpret_cast<volatile char*>(source)[0]);

  SAFE_SYSCALL(munmap(source, page_size));
  SAFE_SYSCALL(munmap(remapped, page_size));
}

TEST(Mremap, MremapDontUnmap) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* source =
      mmap(nullptr, page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(source, MAP_FAILED);
  reinterpret_cast<volatile char*>(source)[1] = 'a';

  void* remapped = mremap(source, page_size, page_size, MREMAP_MAYMOVE | MREMAP_DONTUNMAP, 0);
  ASSERT_NE(remapped, MAP_FAILED);
  ASSERT_NE(remapped, source);
  EXPECT_EQ('a', reinterpret_cast<volatile char*>(remapped)[1]);
  // MREMAP_DONTUNMAP leaves the source mapped but makes any new access to the unmapped range a
  // pagefault that will be zero-filled in the absence of userfaultfd.
  EXPECT_EQ('\0', reinterpret_cast<volatile char*>(source)[1]);

  SAFE_SYSCALL(munmap(source, page_size));
  SAFE_SYSCALL(munmap(remapped, page_size));
}

TEST(Mremap, MremapDontUnmapFixed) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* source =
      mmap(nullptr, page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(source, MAP_FAILED);

  void* available =
      mmap(nullptr, 2 * page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(available, MAP_FAILED);
  SAFE_SYSCALL(munmap(available, 2 * page_size));

  // Check that the specified address wasn't ignored: if it was, remap would land on
  // available + page_size instead as this is the next unused range.
  void* remapped = mremap(source, page_size, page_size,
                          MREMAP_MAYMOVE | MREMAP_DONTUNMAP | MREMAP_FIXED, available);
  ASSERT_EQ(remapped, available);

  SAFE_SYSCALL(munmap(source, page_size));
  SAFE_SYSCALL(munmap(remapped, page_size));
}

TEST(Mremap, MremapDontUnmapSharedAnon) {
  if (!test_helper::IsStarnix() && !test_helper::IsKernelVersionAtLeast(5, 13)) {
    GTEST_SKIP()
        << "MREMAP_DONTUNMAP on shared memory isn't supported on Linux with kernel version older"
        << " than 5.13, skipping.";
  }
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* source =
      mmap(nullptr, page_size, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(source, MAP_FAILED);
  reinterpret_cast<volatile char*>(source)[0] = 'a';

  void* remapped = mremap(source, page_size, page_size, MREMAP_MAYMOVE | MREMAP_DONTUNMAP, 0);
  ASSERT_NE(remapped, MAP_FAILED);
  ASSERT_NE(remapped, source);
  EXPECT_EQ('a', reinterpret_cast<volatile char*>(remapped)[0]);
  // MREMAP_DONTUNMAP on shared anonymous memory creates a new mapping of the same memory.
  EXPECT_EQ('a', reinterpret_cast<volatile char*>(source)[0]);

  SAFE_SYSCALL(munmap(source, page_size));
  SAFE_SYSCALL(munmap(remapped, page_size));
}

TEST(Mremap, MremapDontUnmapGap) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* source =
      mmap(nullptr, 3 * page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(source, MAP_FAILED);
  SAFE_SYSCALL(
      munmap(reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(source) + page_size), page_size));

  void* remapped =
      mremap(source, 3 * page_size, 3 * page_size, MREMAP_MAYMOVE | MREMAP_DONTUNMAP, 0);
  ASSERT_EQ(remapped, MAP_FAILED);
  SAFE_SYSCALL(munmap(source, 3 * page_size));
}

TEST(Mremap, MremapDontUnmapTwoSharedAnon) {
  if (!test_helper::IsStarnix() && !test_helper::IsKernelVersionAtLeast(5, 13)) {
    GTEST_SKIP()
        << "MREMAP_DONTUNMAP on shared memory isn't supported on Linux with kernel version older"
        << " than 5.13, skipping.";
  }
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* source =
      mmap(nullptr, 2 * page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(source, MAP_FAILED);
  SAFE_SYSCALL(munmap(source, 2 * page_size));
  void* page1 = mmap(source, page_size, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(page1, MAP_FAILED);
  void* page2 = mmap(reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(source) + page_size),
                     page_size, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(page2, MAP_FAILED);

  void* remapped =
      mremap(source, 2 * page_size, 2 * page_size, MREMAP_MAYMOVE | MREMAP_DONTUNMAP, 0);
  ASSERT_EQ(remapped, MAP_FAILED);
  SAFE_SYSCALL(munmap(source, 2 * page_size));
}

TEST(Mremap, GrowThenGrow) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* space =
      mmap(nullptr, 3 * page_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(space, MAP_FAILED);
  munmap(space, 3 * page_size);

  void* mapping = mmap(space, page_size, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
  ASSERT_EQ(mapping, space);

  void* first_remap = mremap(mapping, page_size, 2 * page_size, 0);
  ASSERT_EQ(first_remap, mapping);

  void* second_remap = mremap(mapping, 2 * page_size, 3 * page_size, 0);
  ASSERT_EQ(second_remap, mapping);

  munmap(mapping, 3 * page_size);
}

TEST(Mmap, ProtExecInChild) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    void* mapped =
        mmap(nullptr, page_size, PROT_READ | PROT_EXEC, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    ASSERT_NE(mapped, MAP_FAILED);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Mmap, ChoosesSameAddress) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* addr1 = mmap(nullptr, page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(addr1, MAP_FAILED);
  ASSERT_EQ(munmap(addr1, page_size), 0);
  void* addr2 = mmap(nullptr, page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_EQ(addr1, addr2);
  ASSERT_EQ(munmap(addr2, page_size), 0);
}

TEST(Mmap, AddressesAreInDescendingOrder) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  std::vector<void*> addresses;
  auto restorer = fit::defer([&]() {
    for (auto addr : addresses) {
      EXPECT_EQ(munmap(addr, page_size), 0);
    }
  });

  for (size_t i = 0; i < 10; i++) {
    void* addr = mmap(nullptr, page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    ASSERT_NE(addr, MAP_FAILED);
    addresses.push_back(addr);
  }

  for (size_t i = 1; i < addresses.size(); i++) {
    EXPECT_LT(addresses[i], addresses[i - 1]);
  }
}

TEST(Mmap, HintIgnoredIfInUse) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* page_in_use = mmap(nullptr, page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(page_in_use, MAP_FAILED);

  // Probe for the next available address
  void* next_addr = mmap(nullptr, page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(next_addr, page_in_use);
  ASSERT_EQ(munmap(next_addr, page_size), 0);

  // Try to mmap at the address that is unavailable, without an overwrite flag.
  void* hint_result = mmap(page_in_use, page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  // mmap should have given us the next available address, ignoring the hint
  EXPECT_EQ(hint_result, next_addr);
  ASSERT_EQ(munmap(hint_result, page_size), 0);

  ASSERT_EQ(munmap(page_in_use, page_size), 0);
}

TEST(Mmap, HintRoundedDownIfMisaligned) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  // Probe for a next available 1-page and 2-page gaps
  void* next_onepage = mmap(nullptr, page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(next_onepage, MAP_FAILED);
  void* next_twopage = mmap(nullptr, 2 * page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(next_twopage, MAP_FAILED);
  ASSERT_EQ(munmap(next_onepage, page_size), 0);
  ASSERT_EQ(munmap(next_twopage, 2 * page_size), 0);

  void* hint_result = mmap(static_cast<char*>(next_twopage) + 1, page_size, PROT_NONE,
                           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(hint_result, MAP_FAILED);
  EXPECT_NE(hint_result,
            next_onepage);  // Not ignoring the hint by allocating the next available 1-page gap
  EXPECT_EQ(hint_result, next_twopage);  // Instead, rounding down the hinted address and using it.
  ASSERT_EQ(munmap(hint_result, page_size), 0);

  hint_result = mmap(static_cast<char*>(next_twopage) + page_size - 1, page_size, PROT_NONE,
                     MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(hint_result, MAP_FAILED);
  EXPECT_NE(hint_result,
            next_onepage);  // Not ignoring the hint by allocating the next available 1-page gap
  EXPECT_EQ(hint_result, next_twopage);  // Instead, rounding down the hinted address and using it.
  ASSERT_EQ(munmap(hint_result, page_size), 0);
}

TEST(Mmap, FixedAddressTooLow) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* low_addr = reinterpret_cast<void*>(page_size);
  void* addr = mmap(low_addr, page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
  ASSERT_EQ(addr, MAP_FAILED);
}

TEST(Mmap, HintedAddressTooLow) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  void* low_addr = reinterpret_cast<void*>(page_size);
  void* addr = mmap(low_addr, page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(addr, MAP_FAILED);
  ASSERT_NE(addr, low_addr);
  ASSERT_EQ(munmap(addr, page_size), 0);
}

TEST(Madvise, MadvRemoveZeroesMemory) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  std::vector<char> test_data(page_size, 'a');
  std::vector<char> zero_data(page_size, '\0');

  int fd = SAFE_SYSCALL(test_helper::MemFdCreate("madv_remove", 0));
  SAFE_SYSCALL(write(fd, test_data.data(), test_data.size()));

  void* addr = mmap(nullptr, page_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  ASSERT_NE(addr, MAP_FAILED);
  close(fd);

  EXPECT_THAT(madvise(addr, page_size, MADV_REMOVE), SyscallSucceeds());
  EXPECT_EQ(memcmp(addr, zero_data.data(), zero_data.size()), 0);
  SAFE_SYSCALL(munmap(addr, page_size));
}

TEST(MMapTest, MsyncFlags) {
  char* mutable_storage = getenv("MUTABLE_STORAGE");
  // TODO(https://fxbug.dev/317285180) Fallback to /tmp if "MUTABLE_STORAGE" isn't set. This env var
  // is required once bare host tests removed.
  std::string dir = (mutable_storage != nullptr) ? std::string(mutable_storage) : "/tmp";
  ASSERT_FALSE(dir.empty());

  std::string test_folder = dir + "/msync_flags_XXXXXX";
  ASSERT_NE(mkdtemp(test_folder.data()), nullptr)
      << "failed to create test folder: " << std::strerror(errno);

  std::string path = test_folder + "/msync_flags_test";

  auto cleanup = fit::defer([&]() {
    remove(path.c_str());
    rmdir(test_folder.c_str());
  });

  int fd = open(path.c_str(), O_RDWR | O_CREAT | O_TRUNC, 0777);
  ASSERT_GE(fd, 0) << "Failed to open test file: " << strerror(errno);

  size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  SAFE_SYSCALL(ftruncate(fd, page_size));

  void* addr = mmap(nullptr, page_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  ASSERT_NE(addr, MAP_FAILED);

  // Test MS_ASYNC
  // Should return immediately (handled as no-op or scheduled).
  *reinterpret_cast<volatile char*>(addr) = 'a';
  EXPECT_THAT(msync(addr, page_size, MS_ASYNC), SyscallSucceeds());

  // Test MS_SYNC
  // Should persist data.
  *reinterpret_cast<volatile char*>(addr) = 'b';
  EXPECT_THAT(msync(addr, page_size, MS_SYNC), SyscallSucceeds());

  // Verify data on disk
  char file_content;
  EXPECT_EQ(pread(fd, &file_content, 1, 0), 1);
  EXPECT_EQ(file_content, 'b');

  // Test MS_INVALIDATE (combined with MS_SYNC)
  *reinterpret_cast<volatile char*>(addr) = 'c';
  EXPECT_THAT(msync(addr, page_size, MS_SYNC | MS_INVALIDATE), SyscallSucceeds());
  EXPECT_EQ(pread(fd, &file_content, 1, 0), 1);
  EXPECT_EQ(file_content, 'c');

  // Test MS_SYNC | MS_ASYNC (invalid)
  EXPECT_THAT(msync(addr, page_size, MS_SYNC | MS_ASYNC), SyscallFailsWithErrno(EINVAL));

  // Test MS_INVALIDATE only (should be like MS_ASYNC)
  *reinterpret_cast<volatile char*>(addr) = 'd';
  EXPECT_THAT(msync(addr, page_size, MS_INVALIDATE), SyscallSucceeds());
  // Data might not be synced

  // Test 0 flags (should be like MS_ASYNC)
  *reinterpret_cast<volatile char*>(addr) = 'e';
  EXPECT_THAT(msync(addr, page_size, 0), SyscallSucceeds());
  // Data might not be synced

  // Final sync to check last write if needed
  *reinterpret_cast<volatile char*>(addr) = 'f';
  EXPECT_THAT(msync(addr, page_size, MS_SYNC), SyscallSucceeds());
  EXPECT_EQ(pread(fd, &file_content, 1, 0), 1);
  EXPECT_EQ(file_content, 'f');

  SAFE_SYSCALL(munmap(addr, page_size));
  close(fd);
}

// Tests that mmap_min_addr specifies the minimum address that unprivileged processes can mmap.
TEST(MmapTest, MmapMinAddr) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Need CAP_SYS_ADMIN to run MmapTest.MmapMinAddr";
  }

  const char kMmapMinAddr[] = "/proc/sys/vm/mmap_min_addr";
  std::string original_value;
  EXPECT_TRUE(files::ReadFileToString(kMmapMinAddr, &original_value));

  // Restore the original value after the test.
  auto cleanup = fit::defer(
      [original_value, kMmapMinAddr]() { files::WriteFile(kMmapMinAddr, original_value); });

  // Tries to mmap at `target_addr` as an unprivileged process.
  // Returns 0 on success, and the error number on failure.
  auto try_mmap_unprivileged = [](size_t target_addr) -> int {
    test_helper::ForkHelper helper;
    helper.OnlyWaitForForkedChildren();
    pid_t pid = helper.RunInForkedProcess([target_addr] {
      // Drop all capabilities to become unprivileged.
      test_helper::DropAllCapabilities();
      fit::result<int, test_helper::ScopedMMap> mmap_result = test_helper::ScopedMMap::MMap(
          reinterpret_cast<void*>(target_addr), 4096, PROT_READ | PROT_WRITE,
          MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
      if (mmap_result.is_error()) {
        _exit(mmap_result.error_value());
      }
      _exit(0);
    });
    test_helper::ForkResult result = helper.WaitForChild(pid);
    if (result.determined_result == testing::AssertionSuccess()) {
      return 0;
    }
    return result.subprocess_exit_status;
  };

  constexpr size_t k5Mb = 5242880;

  // Write 10MB to mmap_min_addr.
  EXPECT_TRUE(files::WriteFile(kMmapMinAddr, "10485760\n"));

  // mmap at 5MB should fail because 5MB < 10MB.
  EXPECT_EQ(try_mmap_unprivileged(k5Mb), EPERM);

  // Write 0MB to mmap_min_addr.
  EXPECT_TRUE(files::WriteFile(kMmapMinAddr, "0\n"));

  // mmap at 5MB should succeed now.
  EXPECT_EQ(try_mmap_unprivileged(k5Mb), 0);
}

// Verifies that closing a file descriptor while it remains mapped defers the
// close event until the memory mapping is unmapped.
//
// When a file is mapped and its file descriptor is closed, the underlying file
// description remains open for the lifetime of the mapping.
//
// An inotify watch is used to observe this deferred-close behavior: closing the
// file descriptor does not trigger an IN_CLOSE_* event while the mapping is
// active; the event is delivered only after unmapping the memory.
TEST(MmapTest, FileCloseDeferredWhileMapped) {
  test_helper::ScopedTempDir temp_dir;
  const std::string test_file_path = temp_dir.path() + "/mapped_file";

  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  std::string file_contents(page_size, 'a');
  ASSERT_TRUE(files::WriteFile(test_file_path, file_contents));

  // Use an inotify watch to observe when the close event occurs.
  fbl::unique_fd inotify_fd(inotify_init1(IN_NONBLOCK | IN_CLOEXEC));
  ASSERT_THAT(inotify_fd.get(), SyscallSucceeds());

  int wd = inotify_add_watch(inotify_fd.get(), test_file_path.c_str(),
                             IN_CLOSE_NOWRITE | IN_CLOSE_WRITE);
  ASSERT_THAT(wd, SyscallSucceeds());

  // Open the file and map it into memory.
  fbl::unique_fd file_fd(open(test_file_path.c_str(), O_RDONLY));
  ASSERT_THAT(file_fd.get(), SyscallSucceeds());

  void* mapped_addr = mmap(nullptr, page_size, PROT_READ, MAP_SHARED, file_fd.get(), 0);
  ASSERT_NE(mapped_addr, MAP_FAILED) << strerror(errno);

  // Close the file descriptor while the mapping is still active.
  file_fd.reset();

  // No close event should be emitted yet while the mapping exists.
  char event_buf[sizeof(struct inotify_event) + NAME_MAX + 1];
  EXPECT_THAT(read(inotify_fd.get(), event_buf, sizeof(event_buf)), SyscallFailsWithErrno(EAGAIN));

  // Unmap the memory.
  ASSERT_THAT(munmap(mapped_addr, page_size), SyscallSucceeds());

  // The close event should now be delivered.
  ssize_t bytes_read = read(inotify_fd.get(), event_buf, sizeof(event_buf));
  ASSERT_THAT(bytes_read, SyscallSucceeds());
  ASSERT_GE(bytes_read, static_cast<ssize_t>(sizeof(struct inotify_event)));
  const auto* event = reinterpret_cast<const struct inotify_event*>(event_buf);
  EXPECT_EQ(event->wd, wd);
  EXPECT_TRUE(event->mask & IN_CLOSE_NOWRITE);
}

}  // namespace
