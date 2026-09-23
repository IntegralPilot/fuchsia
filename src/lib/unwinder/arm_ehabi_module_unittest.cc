// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/arm_ehabi_module.h"

#include <cstdint>
#include <filesystem>
#include <limits>

#include <gtest/gtest.h>

#include "src/lib/unwinder/memory.h"

namespace {
#ifdef __Fuchsia__
constexpr char file_path[] = "/pkg/testdata/libunwind_info_test_data.targetso";
#else
constexpr char file_path[] = "test_data/unwinder/libunwind_info_test_data.targetso";
#endif

std::string GetTestFilePath() {
  std::string ret;
#ifdef __Fuchsia__
  ret = file_path;
#else
  char fullpath[PATH_MAX];
  realpath("/proc/self/exe", fullpath);
  std::filesystem::path self_path = fullpath;
  ret = self_path.parent_path() / file_path;
#endif
  return ret;
}

}  // namespace

namespace unwinder {

// This tests the upper bounds searching algorithm with a curated binary. It is important to use the
// checked in binary so the addresses don't change. If changes are made to the test data source
// file the checked in file also needs to be updated.
TEST(ArmEhAbiModule, Search) {
  // This only works if p_offset == p_vaddr in the elf file.
  FileMemory memory(GetTestFilePath());
  // We're faking a loaded process, but since we're not actually loaded anywhere into memory the
  // load address is zero.
  Module elf_module(0, &memory, Module::AddressMode::kProcess);
  LoadedElfModule loaded_elf_module(elf_module);
  ASSERT_TRUE(loaded_elf_module.Load().is_ok());
  ArmEhAbiModule ehabi_module(loaded_elf_module, loaded_elf_module.binary_memory(),
                              static_cast<uint32_t>(loaded_elf_module.load_address()));

  ASSERT_TRUE(ehabi_module.Load().is_ok());

  ArmEhAbiModule::IdxHeader entry;

  // Test with an address below all of the entries. This is an error.
  Error error = ehabi_module.Search(0, entry);
  ASSERT_TRUE(error.has_err());

  // Test with an address above all of the entries. This won't return an error, but instead will
  // return the last entry in the table.
  error = ehabi_module.Search(std::numeric_limits<uint32_t>::max(), entry);
  ASSERT_TRUE(error.ok());
  EXPECT_EQ(entry.header.fn_addr, 0x160A4u);

  // This should get us the specific entry with a function address at 0x11FAC and compact inline
  // representation.
  error = ehabi_module.Search(0x11FB0, entry);
  ASSERT_TRUE(error.ok());
  EXPECT_EQ(entry.header.fn_addr, 0x11FACu);
  EXPECT_EQ(entry.type, ArmEhAbiModule::IdxHeader::Type::kCompactInline);

  // This should get us the specific entry with a function address at 0x13284 and an offset to the
  // .ARM.extab section with the unwinding instructions.
  error = ehabi_module.Search(0x13290, entry);
  ASSERT_TRUE(error.ok());
  EXPECT_EQ(entry.header.fn_addr, 0x13284u);
  EXPECT_EQ(entry.type, ArmEhAbiModule::IdxHeader::Type::kCompact);
}

}  // namespace unwinder
