// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/elf_memory_region.h"

#include <string.h>

#include <filesystem>

#include <gtest/gtest.h>
#include <llvm/BinaryFormat/ELF.h>

#include "src/developer/debug/zxdb/common/host_util.h"
#include "src/lib/elflib/elflib.h"

namespace zxdb {

namespace {
std::filesystem::path GetTestElfFile() {
  return std::filesystem::path(GetSelfPath()).parent_path() /
         "test_data/zxdb/libzxdb_symbol_test.targetso";
}
}  // namespace

TEST(ElfMemoryRegion, RandomAccess) {
  // Fake load address that will be offset for any addresses we want to read from the ELF file.
  constexpr uint64_t kLoadAddress = 0x1000;

  auto elflib = elflib::ElfLib::Create(GetTestElfFile());
  ElfMemoryRegion elf_memory_region(kLoadAddress, GetTestElfFile());

  // We should be able to transparently read the ELF header.
  elflib::Elf64_Ehdr ehdr_read;
  elf_memory_region.ReadBytes(kLoadAddress, sizeof(ehdr_read), &ehdr_read);
  ASSERT_TRUE(ehdr_read.checkMagic());

  auto ehdr = elflib->GetEhdr();

  // The ELF header we read should be identical to elflib's.
  EXPECT_EQ(memcmp(ehdr, &ehdr_read, sizeof(ehdr_read)), 0);
}

TEST(ElfMemoryRegion, CompressedAccess) {
  constexpr uint64_t kLoadAddress = 0xdeadbeef;

  auto elflib = elflib::ElfLib::Create(GetTestElfFile());
  ElfMemoryRegion elf_memory_region(kLoadAddress, GetTestElfFile());

  std::optional<elflib::Elf64_Shdr> debug_info_section_header = std::nullopt;
  auto data = elflib->GetSectionData(".shstrtab");
  for (const auto& shdr : elflib->GetSectionHeaders()) {
    constexpr char kDebugInfoSectionName[] = ".debug_info";

    const char* section_name = reinterpret_cast<const char*>(&data.ptr[shdr.sh_name]);
    if (strncmp(section_name, kDebugInfoSectionName, strlen(kDebugInfoSectionName)) == 0) {
      debug_info_section_header = shdr;
      break;
    }
  }

  ASSERT_NE(debug_info_section_header, std::nullopt);

  // The debug_info section should be compressed.
  ASSERT_NE(debug_info_section_header->sh_flags & elflib::SHF_COMPRESSED, 0u);

  auto debug_info_offset = debug_info_section_header->sh_offset;

  // Requesting a read at the offset into the file should trigger the debug_info section to be
  // decompressed by the ElfMemoryRegion reader.
  constexpr size_t kBufferSize = 128;
  uint8_t buf[kBufferSize];  // Contents don't matter.
  auto err = elf_memory_region.ReadBytes(kLoadAddress + debug_info_offset, kBufferSize, buf);
  ASSERT_TRUE(err.ok()) << err.msg();

  // There should be exactly one decompressed section (.debug_info).
  EXPECT_EQ(elf_memory_region.decompressed_sections_.size(), 1u);

  // The whole section should be decompressed and loaded into the objects cache, the decompressed
  // section should be larger than what the ELF file section size is, since that's the compressed
  // size.
  EXPECT_GT(elf_memory_region.decompressed_sections_.begin()->second.size(),
            debug_info_section_header->sh_size);

  // The compressed section header will report the exact decompressed size.
  elflib::Elf64_Chdr compressed_header;
  auto debug_info_memory = elflib->GetSectionData(".debug_info");
  memcpy(&compressed_header, debug_info_memory.ptr, sizeof(compressed_header));

  EXPECT_EQ(compressed_header.ch_size,
            elf_memory_region.decompressed_sections_.begin()->second.size());

  // The purpose of this test isn't to validate the debug_info, that can be evaluated elsewhere, but
  // we should at least make sure that the first few bytes of the decompressed data is not the same
  // as the compressed header.
  EXPECT_NE(memcmp(reinterpret_cast<elflib::Elf64_Chdr*>(buf), &compressed_header,
                   sizeof(compressed_header)),
            0);

  // Because the decompressed section is larger than the section size reported by the section
  // header, we need to make sure that we can read from the end of the decompressed section, which
  // would appear like it belongs to the next section if it is simply compared against the section
  // size as reported by the header.
}

}  // namespace zxdb
