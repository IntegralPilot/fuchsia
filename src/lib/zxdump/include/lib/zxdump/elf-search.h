// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ZXDUMP_INCLUDE_LIB_ZXDUMP_ELF_SEARCH_H_
#define SRC_LIB_ZXDUMP_INCLUDE_LIB_ZXDUMP_ELF_SEARCH_H_

#include <lib/elfldltl/layout.h>
#include <lib/fit/result.h>
#include <zircon/syscalls/object.h>

#include <ostream>
#include <span>
#include <utility>
#include <variant>

#include "buffer.h"
#include "dump.h"
#include "task.h"
#include "types.h"

namespace zxdump {

// Zircon core dumps are always in the 64-bit little-endian ELF format.
using Elf = elfldltl::Elf64<elfldltl::ElfData::k2Lsb>;

// However, ELF images in memory can be either 64-bit or 32-bit.
template <class... Elf>
using AllElfPhdrs = std::variant<std::span<const typename Elf::Phdr>...>;
using AnyElfPhdrs = elfldltl::AllNativeFormats<AllElfPhdrs>;

// AnyElfPhdrsBuffer is just a variant of zxdump::Buffer<Elf...::Phdr> types,
// but it also has the empty() method to avoid needing to use std::visit or
// other methods to access the buffer when which variant it is doesn't matter.
template <class... Elf>
using AllElfPhdrsBuffer = std::variant<Buffer<typename Elf::Phdr>...>;
class AnyElfPhdrsBuffer : public elfldltl::AllNativeFormats<AllElfPhdrsBuffer> {
 public:
  using elfldltl::AllNativeFormats<AllElfPhdrsBuffer>::variant;

  bool empty() const {
    return std::visit([](const auto& buffer) { return buffer->empty(); }, *this);
  }
};

// This can be used from a zxdump::SegmentCallback function to guess if it's
// likely worthwhile to probe this mapping for containing the beginning of an
// ELF image.  If this returns true, a zxdump::ProcessDump::FindBuildIdNote
// call on the segment is recommended.
bool IsLikelyElfMapping(const zx_info_maps_t& maps);

// These helpers are used by zxdump::ProcessDump::FindBuildIdNote, which is
// used in `prune_segment` callbacks for zxdump::ProcessDump::CollectProcess.
// But they can also be used separately.

// An error result from zxdump::DetectElf or zxdump::DetectElfIdentity stems
// from a failing zx::process::read_memory call.  The .error_value() type
// extends zxdump::Error with the vaddr of that failing read_memory attempt.
// This can be due to a low-level probably reading the segment (including just
// a race with mappings having changed in the process since the data was
// collected); or due to ELF metadata that otherwise looked plausibly valid
// pointing to relative addresses in the putative ELF load image.
struct ElfSearchError : public Error {
  constexpr ElfSearchError(Error error, zx_vaddr_t error_vaddr)
      : Error{error}, vaddr{error_vaddr} {}

  constexpr ElfSearchError(const ElfSearchError&) = default;

  constexpr ElfSearchError& operator=(const ElfSearchError&) = default;

  constexpr auto operator<=>(const ElfSearchError&) const = default;

  zx_vaddr_t vaddr;
};

// This prints "op at vaddr: status" with the status string.
std::ostream& operator<<(std::ostream& os, const ElfSearchError& error);

// Try to detect an ELF image in this segment.  If one is found and its phdrs
// are accessible, return a view into them from process.read_memory (which see
// about the lifetime of the buffer).  Its ELF header can be fetched with
// process.read_memory<Elf::Ehdr>(segment.base).  If none is found, this
// returns success with an empty Buffer.  It only returns error when
// process.read_memory gets an error, either because the segment memory can't
// be read or because the ELF header pointed to a Phdr range (not necessarily
// all in that segment) that can't be read.
fit::result<ElfSearchError, AnyElfPhdrsBuffer> DetectElf(  //
    Process& process, const zx_info_maps_t& segment);

// This is the return value of zxdump::DetectElfIdentity, below.  It contains
// absolute memory ranges in the process that provide identifying information.
struct ElfIdentity {
  static constexpr size_t kBuildIdOffset = sizeof(Elf::Nhdr) + sizeof("GNU");

  // This gives the vaddr and size of the ELF build ID note in memory.  The
  // size is zero if no build ID note was found.  The memory there can be read
  // out and the note header parsed to find the ID payload.  The note header
  // and name together have a fixed known size before the build ID bytes so
  // that can just be skipped to extract only the actual ID bytes.
  SegmentDisposition::Note build_id;

  // This gives the vaddr and size of the DT_SONAME string (without its NUL
  // terminator).  The size is zero if no valid DT_SONAME string was found.
  // (This reuses the vaddr, size pair type for the string's range in memory,
  // but note not a note.)
  SegmentDisposition::Note soname;
};

// Given a detected ELF image, examine its program headers and memory to find
// the build ID note and DT_SONAME string if possible.  The argument span
// doesn't have to be one returned by DetectElf (i.e. by process.read_memory)
// but it can be.
fit::result<ElfSearchError, ElfIdentity> DetectElfIdentity(  //
    Process& process, const zx_info_maps_t& segment, const AnyElfPhdrs& phdrs);

// This overload converts the variant buffer to the variant span.
inline fit::result<ElfSearchError, ElfIdentity> DetectElfIdentity(  //
    Process& process, const zx_info_maps_t& segment, const AnyElfPhdrsBuffer& phdrs_buffer) {
  return std::visit(
      [&](const auto& buffer) { return DetectElfIdentity(process, segment, *buffer); },
      phdrs_buffer);
}

// This represents the data from ZX_INFO_PROCESS_MAPS or ZX_INFO_VMAR_MAPS.
using MapsInfoSpan = std::span<const zx_info_maps_t>;

// This uses zxdump::DetectElf to search a range of the address space for an
// ELF image.  It returns subspan of the argument span that holds an ELF image.
// It returns success with an empty span if none is found.  It only returns
// error if zxdump::DetectElf or zxdump::DetectElfIdentity got an error.
fit::result<ElfSearchError, MapsInfoSpan> ElfSearch(Process& process, MapsInfoSpan maps_info);

}  // namespace zxdump

#endif  // SRC_LIB_ZXDUMP_INCLUDE_LIB_ZXDUMP_ELF_SEARCH_H_
