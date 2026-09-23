// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <optional>
#include <string_view>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/split_string.h"
#include "src/starnix/tests/syscalls/cpp/proc_test_base.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

class MlockProcTest : public ProcTestBase {
 protected:
  std::optional<size_t> GetVmLckInKb() {
    std::string status;
    if (!files::ReadFileToString(proc_path() + "/self/status", &status)) {
      return std::nullopt;
    }

    const std::string_view prefix = "VmLck";
    for (const auto& line :
         fxl::SplitString(status, "\n", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty)) {
      std::vector<std::string_view> fields =
          fxl::SplitString(line, ":", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
      if (fields[0] == prefix) {
        if (fields.size() != 2) {
          return std::nullopt;
        }
        return test_helper::parse_field_in_kb(fields[1]);
      }
    }
    return std::nullopt;
  }
};

TEST_F(MlockProcTest, UnalignedAddress) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  auto mapping = test_helper::ScopedMMap::MMap(nullptr, page_size, PROT_READ,
                                               MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_THAT(mapping, SyscallResultIsOk());

  auto mapped_page = reinterpret_cast<uint8_t*>(mapping->mapping());
  ASSERT_EQ(mlock(reinterpret_cast<void*>(mapped_page + 10), page_size - 10), 0) << strerror(errno);

  std::string smaps;
  ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/smaps", &smaps));

  auto smap = test_helper::find_memory_mapping_ext(reinterpret_cast<uintptr_t>(mapped_page), smaps);
  ASSERT_NE(smap, std::nullopt);
  ASSERT_TRUE(smap->ContainsFlag("lo")) << *smap;
}

TEST_F(MlockProcTest, UnlignedLength) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  auto mapping = test_helper::ScopedMMap::MMap(nullptr, page_size, PROT_READ,
                                               MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_THAT(mapping, SyscallResultIsOk());

  ASSERT_EQ(mlock(mapping->mapping(), page_size - 10), 0) << strerror(errno);

  std::string smaps;
  ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/smaps", &smaps));

  auto smap =
      test_helper::find_memory_mapping_ext(reinterpret_cast<uintptr_t>(mapping->mapping()), smaps);
  ASSERT_NE(smap, std::nullopt);
  ASSERT_TRUE(smap->ContainsFlag("lo")) << *smap;
}

TEST_F(MlockProcTest, RoundingIsCorrect) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  auto mapping = test_helper::ScopedMMap::MMap(nullptr, page_size * 2, PROT_READ,
                                               MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_THAT(mapping, SyscallResultIsOk());

  // Ask to lock two bytes, starting one byte before the page boundary. This should produce a 2-page
  // lock.
  auto midpoint = reinterpret_cast<uint8_t*>(mapping->mapping()) + page_size;
  ASSERT_EQ(mlock(midpoint - 1, 2), 0) << strerror(errno);

  std::string smaps;
  ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/smaps", &smaps));

  auto smap =
      test_helper::find_memory_mapping_ext(reinterpret_cast<uintptr_t>(mapping->mapping()), smaps);
  ASSERT_NE(smap, std::nullopt);
  ASSERT_EQ(smap->start, reinterpret_cast<uintptr_t>(mapping->mapping()));
  ASSERT_EQ(smap->end, reinterpret_cast<uintptr_t>(mapping->mapping()) + (2 * page_size));
  ASSERT_TRUE(smap->ContainsFlag("lo")) << *smap;
}

TEST_F(MlockProcTest, SplitMapping) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  auto mapping = test_helper::ScopedMMap::MMap(nullptr, page_size * 3, PROT_READ,
                                               MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_THAT(mapping, SyscallResultIsOk());

  auto first_page = reinterpret_cast<uint8_t*>(mapping->mapping());
  auto first_addr = reinterpret_cast<uintptr_t>(first_page);
  auto second_page = reinterpret_cast<void*>(first_page + page_size);
  auto second_addr = reinterpret_cast<uintptr_t>(second_page);
  auto third_addr = second_addr + page_size;

  // Lock the middle page of the mapping.
  ASSERT_EQ(mlock(second_page, page_size), 0) << strerror(errno);

  std::string smaps;
  ASSERT_TRUE(files::ReadFileToString(proc_path() + "/self/smaps", &smaps));

  auto first_mapping = test_helper::find_memory_mapping_ext(first_addr, smaps);
  ASSERT_NE(first_mapping, std::nullopt);
  ASSERT_EQ(first_mapping->start, first_addr) << *first_mapping;
  ASSERT_EQ(first_mapping->end, second_addr) << *first_mapping;
  ASSERT_FALSE(first_mapping->ContainsFlag("lo")) << *first_mapping;

  auto second_mapping = test_helper::find_memory_mapping_ext(second_addr, smaps);
  ASSERT_NE(second_mapping, std::nullopt);
  ASSERT_EQ(second_mapping->start, second_addr) << *second_mapping;
  ASSERT_EQ(second_mapping->end, third_addr) << *second_mapping;
  ASSERT_TRUE(second_mapping->ContainsFlag("lo")) << *second_mapping;

  auto third_mapping = test_helper::find_memory_mapping_ext(third_addr, smaps);
  ASSERT_NE(third_mapping, std::nullopt);
  ASSERT_EQ(third_mapping->start, third_addr) << *third_mapping;
  ASSERT_EQ(third_mapping->end, third_addr + page_size) << *third_mapping;
  ASSERT_FALSE(third_mapping->ContainsFlag("lo")) << *third_mapping;
}

TEST_F(MlockProcTest, MlockOnePageIncreasesVmLck) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  const size_t map_size = 2 * page_size;

  // Map 2 private-anonymous pages.
  auto mapping = test_helper::ScopedMMap::MMap(nullptr, map_size, PROT_READ | PROT_WRITE,
                                               MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_THAT(mapping, SyscallResultIsOk());

  // Touch them to ensure non-zero number of committed bytes.
  reinterpret_cast<volatile char*>(mapping->mapping())[0] = 'a';
  reinterpret_cast<volatile char*>(mapping->mapping())[page_size] = 'a';

  // Record the `VmLck` value.
  auto vmlck_before = GetVmLckInKb();
  ASSERT_NE(vmlck_before, std::nullopt);

  // Test that locking any of the two pages results in the increase of VmLck by one page
  for (int i = 0; i < 2; i++) {
    // Lock one page.
    ASSERT_EQ(mlock(reinterpret_cast<uint8_t*>(mapping->mapping()) + (i * page_size), page_size), 0)
        << strerror(errno);

    // Record the `VmLck` value again.
    auto vmlck_after = GetVmLckInKb();
    ASSERT_NE(vmlck_after, std::nullopt);

    // Check that the new value is greater than the base one by exactly one page.
    EXPECT_GT(*vmlck_after, *vmlck_before);
    EXPECT_EQ(*vmlck_after - *vmlck_before, page_size / 1024);

    ASSERT_EQ(munlock(reinterpret_cast<uint8_t*>(mapping->mapping()) + (i * page_size), page_size),
              0)
        << strerror(errno);
  }
}
