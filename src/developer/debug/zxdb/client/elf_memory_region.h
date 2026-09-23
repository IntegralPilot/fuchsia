// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ELF_MEMORY_REGION_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ELF_MEMORY_REGION_H_

#include "gtest/gtest_prod.h"
#include "src/developer/debug/zxdb/common/err.h"
#include "src/lib/elflib/elflib.h"
#include "src/lib/unwinder/memory.h"

namespace zxdb {

// Memory region backed by an ELF file on disk, which may contain compressed debug sections.
class ElfMemoryRegion : public unwinder::Memory {
 public:
  ElfMemoryRegion(uint64_t load_address, const std::string& path)
      : load_address_(load_address),
        file_(fopen(path.c_str(), "rb"), fclose),
        elflib_(
            elflib::ElfLib::Create(file_.get(), elflib::ElfLib::Ownership::kDontTakeOwnership)) {}

  // unwinder::Memory implementation.
  unwinder::Error ReadBytes(uint64_t addr, uint64_t size, void* dst) override;

 private:
  // For access to |decompressed_sections_|.
  FRIEND_TEST(ElfMemoryRegion, CompressedAccess);

  // Performs a simple read at the given file offset. This is to read all of the non-compressed
  // metadata in an ELF file.
  unwinder::Error ReadFromOffset(uint64_t offset, uint64_t size, void* dst);

  // Decompresses the section data corresponding to the given header and index and stores the
  // decompressed data in |decompressed_sections_|.
  Err DecompressSection(const elflib::Elf64_Shdr& hdr);

  uint64_t load_address_;

  // Decompressed debug_info sections, indexed by offsets in the elf file, not load offsets.
  std::map<uint64_t, std::vector<uint8_t>> decompressed_sections_;

  std::unique_ptr<FILE, decltype(&fclose)> file_;

  // Elflib helps us calculate section offsets and fetching raw section data. We need to properly
  // handle compressed debug_info regions while being able to supply the unwinder with direct file
  // access. Due to this, it's not trivial to supply the data from the lowlevel LLVM objects, which
  // are capable of decompressing the different sections, but don't provide a nice interface to
  // easily access it like a file.
  std::unique_ptr<elflib::ElfLib> elflib_ = nullptr;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ELF_MEMORY_REGION_H_
