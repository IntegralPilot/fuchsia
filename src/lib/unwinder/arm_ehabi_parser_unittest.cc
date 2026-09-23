// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/arm_ehabi_parser.h"

#include <filesystem>

#include <gtest/gtest.h>

#include "src/lib/unwinder/arm_ehabi_module.h"
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

// Tests that EHABI instructions are properly serialized when inlined in ARM.exidx using a curated
// binary. Note that if changes are made to the binary, then this test will need to be updated.
TEST(ArmEhAbiParser, CollectInstructionsIndexInline) {
  // This only works if p_offset == p_vaddr in the elf file.
  FileMemory memory(GetTestFilePath());

  // These values were hand-picked out of the test binary, if this needs to change in the future,
  // you can use `fx binutils readelf -u` to examine the unwind tables and reproduce the values
  // in the manually constructed index header below.
  //
  // Entry {
  //   FunctionAddress: 0x13A3C
  //   Model: Compact (Inline)
  //   PersonalityIndex: 0
  //   Opcodes [
  //     0xB2 0x0E ; vsp = vsp + 572
  //     0xAF      ; pop {r4, r5, r6, r7, r8, r9, r10, fp, lr}
  //   ]
  // }
  constexpr ArmEhAbiModule::IdxHeader idx_entry = {
      .header =
          {
              .fn_addr = 0x13a3c,
              .data = 0x80b20eaf,
          },
      .type = ArmEhAbiModule::IdxHeader::Type::kCompactInline,
  };
  constexpr std::array<uint8_t, 3> expected = {0xb2, 0x0e, 0xaf};

  ArmEhAbiParser parser(&memory, idx_entry);
  auto result = parser.CollectInstructions();
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(expected.size(), result.value().size());

  for (size_t i = 0; i < expected.size(); i++) {
    EXPECT_EQ(expected[i], result.value()[i]);
  }
}

// Tests that EHABI instructions are properly serialized from ARM.extab using a curated
// binary. Note that if changes are made to the binary, then this test will need to be updated.
TEST(ArmEhAbiParser, CollectInstructionsTableLookup) {
  // This only works if p_offset == p_vaddr in the elf file.
  FileMemory memory(GetTestFilePath());

  // Use the higher level Module class to figure out what the offset is for us so we don't have to
  // do it by hand. Since this is using a file rather than a properly loaded ELF file, the load
  // address is 0. These tests can be improved in the future to accommodate a non-zero load address.
  constexpr uint32_t kLoadAddress = 0x0;
  Module elf_module(kLoadAddress, &memory, Module::AddressMode::kProcess);
  LoadedElfModule loaded_elf_module(elf_module);
  ASSERT_TRUE(loaded_elf_module.Load().is_ok());
  ArmEhAbiModule ehabi_module(loaded_elf_module, loaded_elf_module.binary_memory(),
                              static_cast<uint32_t>(loaded_elf_module.load_address()));
  ASSERT_TRUE(ehabi_module.Load().is_ok());

  constexpr uint32_t kTargetPc = 0x13e54;

  // This will pull out this entry from the index, which we can hand to the parser below with the
  // proper offsets, which also work when the file offsets are equal to the addrs for each
  // section.
  //
  // Entry {
  //   FunctionAddress: 0x13E50
  //   ExceptionHandlingTable: .ARM.extab
  //   TableEntryAddress: 0xCA4
  //   Model: Compact
  //   PersonalityIndex: 1
  //   Opcodes [
  //     0xB2 0x37 ; vsp = vsp + 736
  //     0x84 0x8F ; pop {r4, r5, r6, r7, fp, lr}
  //     0xB0      ; finish
  //     0xB0      ; finish
  //   ]
  // }
  ArmEhAbiModule::IdxHeader header;
  ASSERT_TRUE(ehabi_module.Search(kTargetPc, header).ok());
  EXPECT_EQ(header.type, ArmEhAbiModule::IdxHeader::Type::kCompact);

  constexpr std::array<uint8_t, 4> expected = {0xb2, 0x37, 0x84, 0x8f};

  ArmEhAbiParser parser(&memory, header);
  auto result = parser.CollectInstructions();
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(expected.size(), result.value().size());

  for (size_t i = 0; i < expected.size(); i++) {
    EXPECT_EQ(expected[i], result.value()[i]);
  }
}

}  // namespace unwinder
