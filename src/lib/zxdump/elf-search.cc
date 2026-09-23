// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/zxdump/elf-search.h"

#include <optional>

#include "core.h"

namespace zxdump {
namespace {

template <class Phdr>
struct LayoutFor;

template <>
struct LayoutFor<elfldltl::Elf64<>::Phdr> {
  using type = elfldltl::Elf64<>;
};

template <>
struct LayoutFor<elfldltl::Elf32<>::Phdr> {
  using type = elfldltl::Elf32<>;
};

using DetectElfResult = fit::result<ElfSearchError, AnyElfPhdrsBuffer>;
using DetectIdentityResult = fit::result<ElfSearchError, ElfIdentity>;

DetectElfResult NoElf() { return fit::ok(AnyElfPhdrsBuffer{}); }

template <typename T, class View = std::span<const T>>
fit::result<ElfSearchError, Buffer<T, View>> ReadMemory(  //
    Process& process, zx_vaddr_t vaddr, size_t count) {
  auto result = process.read_memory<T, View>(vaddr, count);
  if (result.is_ok()) [[likely]] {
    return fit::ok(*std::move(result));
  }
  return fit::error{ElfSearchError{result.error_value(), vaddr}};
}

}  // namespace

bool IsLikelyElfMapping(const zx_info_maps_t& maps) {
  const bool readonly =
      (maps.u.mapping.mmu_flags & (ZX_VM_PERM_READ | ZX_VM_PERM_WRITE)) == ZX_VM_PERM_READ;

  // It might be ELF if it...
  return maps.type == ZX_INFO_MAPS_TYPE_MAPPING &&
         maps.u.mapping.vmo_offset == 0 &&  // is the start of the file (VMO),
         readonly &&                        // maps it read-only,
         maps.size > sizeof(Elf::Ehdr);     // ... and has space for ELF,
}

// Try to detect an ELF image in this segment.  If one is found and its phdrs
// are accessible, return a view into them from process.read_memory.
DetectElfResult DetectElf(Process& process, const zx_info_maps_t& segment) {
  if (segment.type != ZX_INFO_MAPS_TYPE_MAPPING) {
    return NoElf();
  }

  union Ehdr {
    elfldltl::Elf64<>::Ehdr e64;
    elfldltl::Elf32<>::Ehdr e32;
  };
  auto result = ReadMemory<Ehdr>(process, segment.base, 1);
  if (result.is_error()) {
    return result.take_error();
  }

  auto try_ehdr = [&process, &segment]<class Ehdr>(  //
                      const Ehdr& ehdr, DetectElfResult& result) -> bool {
    using Phdr = typename Ehdr::ElfLayout::Phdr;
    if (ehdr.Valid() && ehdr.phentsize == sizeof(Phdr) && ehdr.phnum > 0 &&
        ehdr.phnum != Ehdr::kPnXnum && (ehdr.phoff % alignof(Phdr)) == 0 &&
        ehdr.phoff < segment.size && segment.size - ehdr.phoff >= ehdr.phnum * sizeof(Phdr)) {
      auto read = ReadMemory<Phdr>(process, segment.base + ehdr.phoff, ehdr.phnum);
      if (read.is_error() || !read->empty()) {
        result = std::move(read);
        return true;
      }
    }
    return false;
  };

  auto try_formats = [try_ehdr](const auto&... ehdr) {
    DetectElfResult result = NoElf();
    (try_ehdr(ehdr, result) || ...);
    return result;
  };
  return try_formats(result->front().e64, result->front().e32);
}

DetectIdentityResult DetectElfIdentity(Process& process, const zx_info_maps_t& segment,
                                       const AnyElfPhdrs& phdrs) {
  auto detect = [&process, &segment]<class Phdr>(
                    std::span<const Phdr> phdrs) -> fit::result<ElfSearchError, ElfIdentity> {
    using Elf = LayoutFor<Phdr>::type;
    using Dyn = Elf::Dyn;
    using Nhdr = Elf::Nhdr;

    // Find the first PT_LOAD segment.
    std::optional<uint64_t> first_load;
    for (const auto& phdr : phdrs) {
      if (phdr.type == elfldltl::ElfPhdrType::kLoad) {
        first_load = phdr.vaddr & -process.dump_page_size();
        break;
      }
    }
    if (!first_load) {
      // This is not really a valid ELF image, probably.
      return fit::ok(ElfIdentity{});
    }

    // Note the notes.
    std::vector<SegmentDisposition::Note> notes;
    std::optional<SegmentDisposition::Note> dynamic;
    for (const auto& phdr : phdrs) {
      if (phdr.type == elfldltl::ElfPhdrType::kNote && phdr.vaddr >= *first_load) {
        // It's an allocated note segment that might be within this segment.
        const size_t offset = phdr.vaddr - *first_load;
        if (offset < segment.size && segment.size - offset >= phdr.filesz) {
          // It actually fits inside the segment, so we can examine it.
          notes.push_back({phdr.vaddr, phdr.filesz});
        }
      } else if (phdr.type == elfldltl::ElfPhdrType::kDynamic && phdr.vaddr >= *first_load) {
        dynamic = {.vaddr = phdr.vaddr, .size = phdr.filesz};
      }
    }
    // The phdrs view is no longer used now, so it's safe to use read_memory.

    ElfIdentity id;

    // Detect the build ID.
    constexpr uint32_t kBuildIdNoteType{
        static_cast<uint32_t>(elfldltl::ElfNoteType::kGnuBuildId),
    };
    constexpr std::string_view kBuildIdNoteName{
        "GNU", sizeof("GNU"),  // NUL terminator included!
    };

    for (auto note : notes) {
      note.vaddr += segment.base - *first_load;
      while (note.size >= sizeof(Nhdr)) {
        Nhdr nhdr;
        if (auto result = ReadMemory<Nhdr>(process, note.vaddr, 1); result.is_ok()) {
          nhdr = result->front();
        } else {
          return result.take_error();
        }

        const size_t this_note_size =
            sizeof(nhdr) + NoteAlign(nhdr.namesz) + NoteAlign(nhdr.descsz);
        if (this_note_size > note.size) {
          break;
        }

        if (nhdr.type == kBuildIdNoteType && nhdr.descsz > 0 &&
            nhdr.namesz() == kBuildIdNoteName.size()) {
          auto result = ReadMemory<char, std::string_view>(process, note.vaddr + sizeof(Nhdr),
                                                           kBuildIdNoteName.size());
          if (result.is_error()) {
            return result.take_error();
          }

          ZX_DEBUG_ASSERT(result->size() == kBuildIdNoteName.size());
          if (**result == kBuildIdNoteName) {
            // We have a winner!
            note.size = this_note_size;
            id.build_id = note;
            break;
          }
        }

        note.vaddr += this_note_size;
        note.size -= this_note_size;
      }
    }

    // Detect the SONAME.  Ignore any memory errors since just returning
    // a build ID and no SONAME is better than returning no information at all.
    if (dynamic && dynamic->size > sizeof(Dyn)) {
      dynamic->vaddr += segment.base - *first_load;
      dynamic->size /= sizeof(Dyn);
      auto read = process.read_memory<Dyn>(dynamic->vaddr, dynamic->size);
      if (read.is_ok()) {
        std::optional<uint64_t> soname, strtab;
        for (const auto& dyn : **read) {
          switch (dyn.tag) {
            case elfldltl::ElfDynTag::kSoname:
              soname = dyn.val;
              break;
            case elfldltl::ElfDynTag::kStrTab:
              strtab = segment.base - *first_load + dyn.val;
              break;
            default:
              break;
          }
          if (soname && strtab) {
            uint64_t vaddr = *strtab + *soname;
            auto string = process.read_memory_string(vaddr);
            if (string.is_ok()) {
              id.soname = {.vaddr = vaddr, .size = string->size()};
            }
            break;
          }
        }
      }
    }

    return fit::ok(id);
  };

  return std::visit(detect, phdrs);
}

fit::result<ElfSearchError, MapsInfoSpan> ElfSearch(  //
    Process& process, MapsInfoSpan maps_info) {
  for (auto first = maps_info.begin(); first != maps_info.end(); ++first) {
    const auto& segment = *first;
    if (!IsLikelyElfMapping(segment)) {
      continue;
    }

    // If ELF headers of either ELFCLASS are detected, the phdrs will tell all.
    auto on_phdrs = [&](const auto& phdrs_buffer) -> MapsInfoSpan {
      if (phdrs_buffer->empty()) {
        return {};
      }

      std::optional<uint64_t> vaddr_base, vaddr_limit;
      const auto phdrs = *phdrs_buffer;
      const uint64_t page_size = process.dump_page_size();
      for (const auto& phdr : phdrs) {
        if (phdr.type == elfldltl::ElfPhdrType::kLoad) {
          if (!vaddr_base) {
            vaddr_base = phdr.vaddr & -page_size;
          }
          if (phdr.vaddr < *vaddr_base) {
            // It's invalid to have non-ascending segments.
            // Consider this not to be a real ELF image at all.
            vaddr_limit.reset();
            break;
          }

          // Wrap-around arithmetic is fine in calculating the runtime vaddr.
          const zx_vaddr_t abs_vaddr = phdr.vaddr - *vaddr_base + segment.base;

          // Overflow in calculating the runtime range is invalid.
          const uint64_t limit =  //
              (abs_vaddr + phdr.memsz + page_size - 1) & -page_size;
          if (limit < segment.base) {
            vaddr_limit.reset();
            break;
          }

          if (vaddr_limit && limit < *vaddr_limit) {
            // It's invalid to have overlapping segments.
            vaddr_limit.reset();
            break;
          }

          // The high-water mark for the vaddr_size now covers this segment.
          // TODO(https://fxbug.dev/416040479): There may be gaps big enough
          // that other mappings are allowed in between, and the caller will
          // now skip over them.
          vaddr_limit = limit;
        }
      }

      if (!vaddr_limit) {  // No useful phdrs found, so no image here.
        return {};
      }

      // Make sure the described ELF image would actually fit into the range of
      // segments.  If it would go off the end, it's cannot really be a valid
      // _loaded_ ELF image but might just be an ELF _file_ image--and we only
      // notice the difference when it's too high in the address space.  In
      // that case, the _next_ segment might be a real loaded ELF image, so
      // this segment won't be reported as an ELF image and following segments
      // won't be skipped in the scan for one.
      if (*vaddr_limit > maps_info.back().base + maps_info.back().size) {
        return {};
      }

      // We have an image.
      auto last = first;
      do {
        ++last;
      } while (last != maps_info.end() && last->base < *vaddr_limit);
      return {first, last};
    };

    // It's approximately ELF.  See if its segments can be gleaned.
    auto detected = DetectElf(process, segment);
    if (detected.is_error()) {
      return detected.take_error();
    }

    // If valid-looking ELF segments were found, then the job is done.
    // Otherwise iterate to check the next mapping.
    if (MapsInfoSpan found = std::visit(on_phdrs, *detected); !found.empty()) {
      return fit::ok(found);
    }
  }

  return fit::ok(MapsInfoSpan{});
}

}  // namespace zxdump
