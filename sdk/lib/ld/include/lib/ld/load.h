// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_LOAD_H_
#define LIB_LD_LOAD_H_

#include <lib/elfldltl/dynamic.h>
#include <lib/elfldltl/note.h>
#include <lib/elfldltl/phdr.h>
#include <lib/fit/result.h>

#include <tuple>

#include "memory.h"
#include "module.h"

namespace ld {

// Several functions here fill in the passive ABI Module data structure,
// either after or while setting up LoadInfo and Memory objects.

// Shorthand.
template <class Elf = elfldltl::Elf<>>
using AbiModule = abi::Abi<Elf>::Module;

// Set the module Phdrs, which is read directly from the load image, as opposed
// to from a buffer read from the file.
template <class Elf = elfldltl::Elf<>, class LoadInfo, class Memory>
constexpr void SetModulePhdrs(AbiModule<Elf>& module, const typename Elf::Ehdr& ehdr,
                              const LoadInfo& load_info, Memory& memory) {
  // Find the segment that covers the range of the file occupied by the phdrs:
  // [phoff, phoff + phnum * sizeof(Phdr)), if there is one.  Note this is
  // doing a linear search, but in canonical ELF layouts the first segment in
  // the list is always the correct one so usually this will in fact be
  // optimal.  In general, however, there is no requirement that it be first,
  // or that any segment qualify: the segments are ordered by vaddr; while
  // usually offset increases as vaddr increases, that is not mandated by the
  // ELF format rules, so there's no way to avoid checking all the segments
  // until a qualifying one is found.
  load_info.VisitSegments([&](const auto& segment) {
    using Phdr = Elf::Phdr;
    if (segment.offset() <= ehdr.phoff && ehdr.phoff - segment.offset() < segment.filesz() &&
        (segment.filesz() - (ehdr.phoff - segment.offset())) / sizeof(Phdr) >= ehdr.phnum) {
      if (auto read_phdrs = memory.template ReadArray<Phdr>(
              ehdr.phoff - segment.offset() + segment.vaddr(), ehdr.phnum)) {
        module.phdrs = *read_phdrs;
      }
      return false;  // Found the segment of interest; stop looking.
    }
    return true;  // Keep looking at the next segment.
  });
}

// Set the module vaddr bounds, calculated from the runtime load bias.
template <class Elf = elfldltl::Elf<>, class LoadInfo>
constexpr void SetModuleVaddrBounds(AbiModule<Elf>& module, const LoadInfo& load_info,
                                    typename Elf::size_type load_bias) {
  module.link_map.addr = load_bias;
  module.vaddr_start = load_info.vaddr_start() + load_bias;
  module.vaddr_end = module.vaddr_start + load_info.vaddr_size();
}

template <class Elf = elfldltl::Elf<>>
struct ModulePhdrInfo {
  std::optional<typename Elf::Phdr> dyn_phdr;
  std::optional<typename Elf::Phdr> tls_phdr;
  std::optional<typename Elf::Phdr> relro_phdr;
  std::optional<typename Elf::size_type> stack_size;
};

// This uses elfldltl::DecodePhdrs for the things needed by all dynamic linking
// cases, but that aren't stored directly in abi::Abi<>::Module, so they are
// just returned in ModulePhdrInfo.  Additional phdr observers can be passed to
// combine these into a single phdr scan.  This might be used in a single pass
// with the elfldltl::LoadInfo<...>::GetPhdrObserver() observer, when the
// entire ELF file image is already accessible in memory and things like notes
// can be examined in the same one pass using elfldltl::PhdrFileNoteObserver or
// similar things that find data via p_offset.  Or it might be used in a second
// pass after the LoadInfo has been filled in a first pass and the segments
// have been mapped; then elfldltl::PhdrMemoryNoteObserver can be used, or
// similar things that find data via p_vaddr.
template <class Elf = elfldltl::Elf<>, class Diagnostics, typename... PhdrObservers>
constexpr std::optional<ModulePhdrInfo<Elf>> DecodeModulePhdrs(
    Diagnostics& diag, std::span<const typename Elf::Phdr> phdrs,
    PhdrObservers&&... phdr_observers) {
  ModulePhdrInfo<Elf> result;
  if (!elfldltl::DecodePhdrs(diag, phdrs, elfldltl::PhdrDynamicObserver<Elf>(result.dyn_phdr),
                             elfldltl::PhdrRelroObserver<Elf>(result.relro_phdr),
                             elfldltl::PhdrTlsObserver<Elf>(result.tls_phdr),
                             elfldltl::PhdrStackObserver<Elf>(result.stack_size),
                             std::forward<PhdrObservers>(phdr_observers)...)) [[unlikely]] {
    return std::nullopt;
  }
  return result;
}

// This uses elfldltl::DecodeDynamic to fill the Module fields.  Additional
// dynamic section observers can be passed to include in the same scan.
template <class Elf = elfldltl::Elf<>, class Diagnostics, class Memory,
          typename... DynamicObservers>
constexpr fit::result<bool, std::span<const typename Elf::Dyn>> DecodeModuleDynamic(
    AbiModule<Elf>& module, Diagnostics& diag, Memory& memory,
    const std::optional<typename Elf::Phdr>& dyn_phdr, DynamicObservers&&... dynamic_observers) {
  using Dyn = const Elf::Dyn;

  if (!dyn_phdr) [[unlikely]] {
    return fit::error{diag.FormatError("no PT_DYNAMIC program header found")};
  }

  const size_t count = dyn_phdr->filesz / sizeof(Dyn);
  if (count <= 1) [[unlikely]] {
    return fit::error{diag.FormatError("PT_DYNAMIC p_filesz ", dyn_phdr->filesz(),
                                       " too small for any ", sizeof(Dyn),
                                       "-byte entries before DT_NULL entry")};
  }

  auto read_dyn = memory.template ReadArray<Dyn>(dyn_phdr->vaddr, count);
  if (!read_dyn) [[unlikely]] {
    return fit::error{
        diag.FormatError("cannot read", count, " entries from PT_DYNAMIC",
                         elfldltl::FileAddress{dyn_phdr->vaddr}),
    };
  }
  std::span<const Dyn> dyn = *read_dyn;

  module.link_map.ld = dyn.data();

  if (!elfldltl::DecodeDynamic(
          diag, memory, dyn, elfldltl::DynamicSymbolInfoObserver(module.symbols),
          elfldltl::DynamicInitObserver(module.init), elfldltl::DynamicFiniObserver(module.fini),
          std::forward<DynamicObservers>(dynamic_observers)...)) [[unlikely]] {
    return fit::error{false};
  }

  module.soname = module.symbols.soname();

  return fit::ok(dyn);
}

// This returns a callback that fills in the ABI module's build ID.  Use it
// with elfldltl::PhdrFileNoteObserver or elfldltl::PhdrMemoryNoteObserver.
// The module.build_id member must be default-initialized before this.
//
// The callback always return success so that the NoteObserver will never
// short-circuit the phdrs scan it's part of, only short-circuit unnecessary
// note scanning.
template <class Elf = elfldltl::Elf<>>
constexpr auto ObserveBuildIdNote(AbiModule<Elf>& module) {
  assert(module.build_id.empty());
  return [&module](const auto& note) -> fit::result<fit::failed, bool> {
    if (!note.IsBuildId()) {
      // This is a different note, so keep looking.
      return fit::ok(true);
    }

    // This is the build ID note.  After the first time through this path,
    // more callbacks should have been short-circuited by the return below.
    assert(module.build_id.empty());

    module.build_id = note.desc;

    // Tell the caller not to call again for another note.
    return fit::ok(false);
  };
}

// These are convenience wrappers around ObserveBuildIdNote when no other notes
// are of interest in a phdrs scan.  They return phdr note observers reading
// from the file or memory, respectively.

template <class Elf = elfldltl::Elf<>, class File, typename Allocator>
constexpr auto PhdrFileBuildIdObserver(File&& file, Allocator&& allocator, AbiModule<Elf>& module) {
  return elfldltl::PhdrFileNoteObserver(Elf{}, std::forward<File>(file),
                                        std::forward<Allocator>(allocator),
                                        ObserveBuildIdNote<Elf>(module));
}

template <class Elf = elfldltl::Elf<>, class Memory>
constexpr auto PhdrMemoryBuildIdObserver(Memory&& memory, AbiModule<Elf>& module) {
  return elfldltl::PhdrMemoryNoteObserver(Elf{}, std::forward<Memory>(memory),
                                          ObserveBuildIdNote<Elf>(module));
}

}  // namespace ld

#endif  // LIB_LD_LOAD_H_
