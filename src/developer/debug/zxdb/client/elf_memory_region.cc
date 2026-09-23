// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/elf_memory_region.h"

#include <vector>

#include <zstd/zstd.h>

#include "lib/syslog/cpp/macros.h"
#include "src/developer/debug/shared/largest_less_or_equal.h"
#include "src/developer/debug/zxdb/common/err.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/unwinder/error.h"

namespace zxdb {

namespace {
unwinder::Error ReadFromData(const std::vector<uint8_t>& data, uint64_t offset, uint64_t size,
                             void* dst) {
  if (offset + size > data.size()) {
    return unwinder::Error("Read out of bounds.");
  }

  memcpy(dst, data.data() + offset, size);
  return unwinder::Success();
}
}  // namespace

unwinder::Error ElfMemoryRegion::ReadFromOffset(uint64_t offset, uint64_t size, void* dst) {
  if (!file_) {
    return unwinder::Error("File not open!");
  }

  if (fseek(file_.get(), static_cast<int64_t>(offset), SEEK_SET) == -1) {
    return unwinder::Error(fxl::StringPrintf("fseek failed: %s", strerror(errno)));
  }

  if (fread(dst, 1, size, file_.get()) != size) {
    return unwinder::Error("short read!");
  }

  return unwinder::Success();
}

Err ElfMemoryRegion::DecompressSection(const elflib::Elf64_Shdr& hdr) {
  auto section_data = elflib_->GetSectionData(&hdr);

  if (section_data.size == 0 || section_data.ptr == nullptr) {
    return Err("Section data not found");
  }

  bool is_32bit = elflib_->GetEhdr()->e_ident[elflib::EI_CLASS] == elflib::ELFCLASS32;

  elflib::Elf64_Chdr compressed_section_header;
  if (is_32bit) {
    // Upcast from a 32 bit header to a 64 bit.
    elflib::Elf32_Chdr compressed_header32;
    memcpy(&compressed_header32, section_data.ptr, sizeof(compressed_header32));

    compressed_section_header.ch_type = compressed_header32.ch_type;
    compressed_section_header.ch_size = compressed_header32.ch_size;
    compressed_section_header.ch_addralign = compressed_header32.ch_addralign;
  } else {
    memcpy(&compressed_section_header, section_data.ptr, sizeof(compressed_section_header));
  }

  if (compressed_section_header.ch_type != elflib::ELFCOMPRESS_ZSTD) {
    return Err("Only zstd compression is supported for this file. Got %d.",
               compressed_section_header.ch_type);
  }

  std::vector<uint8_t> data;
  data.resize(compressed_section_header.ch_size);
  auto dec = ZSTD_decompress(data.data(), data.size(),
                             section_data.ptr + sizeof(compressed_section_header),
                             section_data.size - sizeof(compressed_section_header));

  if (ZSTD_isError(dec)) {
    return Err("%s", ZSTD_getErrorName(dec));
  }

  decompressed_sections_[hdr.sh_offset] = std::move(data);

  return Err();
}

unwinder::Error ElfMemoryRegion::ReadBytes(uint64_t addr, uint64_t size, void* dst) {
  if (addr < load_address_) {
    return unwinder::Error("out of boundary");
  }

  size_t offset = addr - load_address_;

  // Reading anything from the ELF header or any of the section headers is never compressed.
  if (offset < sizeof(elflib::Elf64_Ehdr) || offset >= elflib_->GetEhdr()->e_shoff) {
    // Just reading from the non-compressed headers.
    return ReadFromOffset(offset, size, dst);
  }

  // Read from any program segments, which are not compressed.
  for (auto& phdr : elflib_->GetSegmentHeaders()) {
    if (phdr.p_offset <= offset && phdr.p_offset + phdr.p_filesz > offset) {
      return ReadFromOffset(offset, size, dst);
    }
  }

  // We need some non-allocated section data. We need to first check if we already have some
  // decompressed data that spans this range, since the real "span" of a compressed section will be
  // larger than if we just looked at the section header data.
  //
  // For example, consider we have a compressed section that reports like the following:
  // Section Headers:
  //   [Nr] Name              Type            Address          Off    Size   ES Flg Lk Inf Al
  //   [ 0]                   NULL            0000000000000000 000000 000000 00      0   0  0
  //   [ 1] .note.gnu.build-id NOTE           0000000000000238 000238 000024 00   A  0   0  4
  //   [ 2] .dynsym           DYNSYM          0000000000000260 000260 000210 18   A  5   1  8
  //   [ 3] .gnu.hash         GNU_HASH        0000000000000470 000470 00008c 00   A  2   0  8
  //   [ 4] .dynamic          DYNAMIC         0000000000000500 000500 0000e0 10   A  5   0  8
  //   [ 5] .dynstr           STRTAB          00000000000005e0 0005e0 0001e5 00   A  0   0  1
  //   [ 6] .rel.plt          REL             00000000000007c8 0007c8 000030 10  AI  2  12  8
  //   [ 7] .rodata           PROGBITS        0000000000000800 000800 000065 00 AMS  0   0 16
  //   [ 8] .eh_frame_hdr     PROGBITS        0000000000000868 000868 0000f4 00   A  0   0  4
  //   [ 9] .eh_frame         PROGBITS        0000000000000960 000960 0002c4 00   A  0   0  8
  //   [10] .text             PROGBITS        0000000000001000 001000 000516 00  AX  0   0 32
  //   [11] .plt              PROGBITS        0000000000001520 001520 000040 00  AX  0   0 16
  //   [12] .got.plt          PROGBITS        0000000000002000 002000 000030 00  WA  0   0  8
  //   [13] .relro_padding    NOBITS          0000000000002030 002030 000fd0 00  WA  0   0  1
  //   [14] .debug_abbrev     PROGBITS        0000000000000000 002030 000273 00   C  0   0  1
  //   [15] .debug_info       PROGBITS        0000000000000000 0022a3 0004d3 00   C  0   0  1
  //   [16] .debug_rnglists   PROGBITS        0000000000000000 002776 000089 00   C  0   0  1
  //   ...
  //
  // The size reported by readelf is the size of the _compressed_ section, and the first several
  // bytes of the compressed data section correspond to a compression header, which tells us the
  // compression scheme and expected size of the decompressed data.
  //
  // Suppose a user of this class requests a read from offset 0x2780 into this file. If we just
  // look at the section offsets and sizes from the header information, we'll end up reading from
  // the .debug_rnglists section, even though that could _also_ be spanning the decompressed range
  // of the .debug_info section as well. In this case this class will return the decompressed data
  // spanning the requested span.
  //
  // There is additional ambiguity, suppose a user requests to read an address within the compressed
  // .debug_abbrev section, which is appropriately decompressed. The new decompressed size of that
  // section will overlap with the also compressed .debug_info section. If a user requests a read at
  // offset 0x22a3 now, do we return data from the decompressed debug_abbrev section or do we
  // decompress the .debug_info section and return that data?
  //
  // For now, the users of this class (//src/lib/unwinder namely) are typically just looking for
  // .debug_frame and/or .eh_frame, which avoids this issue, so it's not required to solve right
  // now. If in the future this class is needed for additional use cases spanning multiple
  // compressed sections this will need to be solved.

  auto found = debug::LargestLessOrEqual(
      decompressed_sections_.begin(), decompressed_sections_.end(), offset,
      [](const decltype(decompressed_sections_)::value_type& v, uint64_t a) { return v.first < a; },
      [](const decltype(decompressed_sections_)::value_type& v, uint64_t a) {
        return v.first == a;
      });

  const elflib::Elf64_Shdr* section_header = nullptr;
  auto headers = elflib_->GetSectionHeaders();

  for (const auto& shdr : headers) {
    if (shdr.sh_type & elflib::SHT_NOBITS) {
      // Skip NOBITS sections.
      continue;
    }

    if (shdr.sh_offset <= offset && shdr.sh_offset + shdr.sh_size > offset) {
      if ((shdr.sh_flags & elflib::SHF_COMPRESSED) != 0) {
        // The data we need is in a compressed section.
        section_header = &shdr;
        break;
      }

      // A non-compressed section here will always be the best choice, read from it directly.
      return ReadFromOffset(offset, size, dst);
    }
  }

  if (found != decompressed_sections_.end()) {
    // Check to see if the offset of the closest section found above matches the offset of the
    // section we found from the decompressed data.
    if (section_header != nullptr && found->first != section_header->sh_offset) {
      // These refer to two different sections. The decompressed data will be larger than the
      // section size reported in the corresponding section header. If the requested read falls
      // within the decompressed data then we will read from that. We already know that the
      // decompressed section offset is less than the offset passed to us from the user from
      // LargestLessOrEqual above so we don't need to check that here.
      if (found->first + found->second.size() <= offset + size) {
        return ReadFromData(found->second, offset - found->first, size, dst);
      }
      // else fall through to decompress the section corresponding to |section_header|.
    } else {
      // The cached section and the section_header match, so we can simply do the read.
      return ReadFromData(found->second, offset - found->first, size, dst);
    }
  }

  // If we don't have a section header that claims to contain the address/offset the user requested,
  // then we're done.
  if (section_header == nullptr) {
    return unwinder::Error("Section not found.");
  }

  // Need to decompress and load this section, then perform the read from the decompressed bytes.
  if (auto err = DecompressSection(*section_header); err.has_error()) {
    return ErrToUnwinderError(err);
  }

  // Try again, we should now have the decompressed data.
  found = debug::LargestLessOrEqual(
      decompressed_sections_.begin(), decompressed_sections_.end(), offset,
      [](const decltype(decompressed_sections_)::value_type& v, uint64_t a) { return v.first < a; },
      [](const decltype(decompressed_sections_)::value_type& v, uint64_t a) {
        return v.first == a;
      });

  if (found == decompressed_sections_.end()) {
    return unwinder::Error("Section not found.");
  }

  // Users who expect to be reading from an ELF file directly (i.e. without regard to compressed
  // sections) will pass us an absolute offset into the file, which will include the section offset.
  // We no longer need that since we have the raw section data and want to return the data at the
  // section relative offset, which is common in various DWARF data encoding schemes.
  return ReadFromData(found->second, offset - found->first, size, dst);
}

}  // namespace zxdb
