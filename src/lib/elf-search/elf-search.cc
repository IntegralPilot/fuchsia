// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <elf-search.h>
#include <elf.h>
#include <inttypes.h>
#include <lib/elfldltl/constants.h>
#include <lib/trace/event.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>

#include <algorithm>
#include <memory>

#include <fbl/alloc_checker.h>

namespace elf_search {
namespace {

// This is a reasonable upper limit on the number of program headers that are
// expected. 7 or 8 is more typical.
constexpr size_t kMaxProgramHeaders = 16;
// kWindowSize is a tuning parameter. It specifies how much memory should be
// read in by ProcessMemReader when a new read is needed. The goal is to
// optimize the trade-off between making too many system calls and reading in
// too much memory. The larger kWindowSize is the fewer system calls are made
// but the more bytes are copied over that don't need to be. The smaller it is
// the more system calls need to be made but the fewer superfluous bytes are
// copied.
// TODO(jakehehrlich): Tune kWindowSize rather than just guessing.
constexpr size_t kWindowSize = 0x400;
// This is an upper bound on the number of bytes that can be used in a build ID.
// md5 and sha1 are the most common hashes used for build ids and they use 20
// and 16 bytes respectively. This makes 32 a generous upper bound.
constexpr size_t kMaxBuildIDSize = 32;
// An upper limit on the length of the DT_SONAME.
constexpr size_t kMaxSonameSize = 256;
// The maximum length of the buffer used for the module name.
constexpr size_t kNameBufferSize = 512;

bool IsPossibleLoadedEhdr(const Elf64_Ehdr& ehdr) {
  // Do some basic sanity checks including checking the ELF identifier
  return ehdr.e_ident[EI_MAG0] == ELFMAG0 && ehdr.e_ident[EI_MAG1] == ELFMAG1 &&
         ehdr.e_ident[EI_MAG2] == ELFMAG2 && ehdr.e_ident[EI_MAG3] == ELFMAG3 &&
         ehdr.e_ident[EI_CLASS] == ELFCLASS64 && ehdr.e_ident[EI_DATA] == ELFDATA2LSB &&
         ehdr.e_ident[EI_VERSION] == EV_CURRENT && ehdr.e_type == ET_DYN &&
         ehdr.e_machine == static_cast<uint16_t>(elfldltl::ElfMachine::kNative) &&
         ehdr.e_version == EV_CURRENT && ehdr.e_ehsize == sizeof(Elf64_Ehdr) &&
         ehdr.e_phentsize == sizeof(Elf64_Phdr) && ehdr.e_phnum > 0 &&
         (ehdr.e_phoff % alignof(Elf64_Phdr) == 0);
}

bool IsPossibleLoadedEhdr(const Elf32_Ehdr& ehdr) {
  // Do some basic sanity checks including checking the ELF identifier
  return ehdr.e_ident[EI_MAG0] == ELFMAG0 && ehdr.e_ident[EI_MAG1] == ELFMAG1 &&
         ehdr.e_ident[EI_MAG2] == ELFMAG2 && ehdr.e_ident[EI_MAG3] == ELFMAG3 &&
         ehdr.e_ident[EI_CLASS] == ELFCLASS32 && ehdr.e_ident[EI_DATA] == ELFDATA2LSB &&
         ehdr.e_ident[EI_VERSION] == EV_CURRENT && ehdr.e_type == ET_DYN &&
         ehdr.e_machine == static_cast<uint16_t>(elfldltl::ElfMachine::kArm) &&
         ehdr.e_version == EV_CURRENT && ehdr.e_ehsize == sizeof(Elf32_Ehdr) &&
         ehdr.e_phentsize == sizeof(Elf32_Phdr) && ehdr.e_phnum > 0 &&
         (ehdr.e_phoff % alignof(Elf32_Phdr) == 0);
}

// TODO(jakehehrlich): Switch uses of uint8_t to std::byte where appropriate.

class ProcessMemReader {
 public:
  ProcessMemReader(const zx::process& proc) : process_(proc) {}

  // TODO(jakehehrlich): Make this interface zero-copy (by returning
  // a pointer rather than copying for instance). It's important that
  // the lifetime of the underlying storage is correctly managed.
  template <typename T>
  [[nodiscard]] zx_status_t Read(uintptr_t vaddr, T* x) {
    return ReadBytes(vaddr, reinterpret_cast<uint8_t*>(x), sizeof(T));
  }

  template <typename T>
  [[nodiscard]] zx_status_t ReadArray(uintptr_t vaddr, T* arr, size_t sz) {
    return ReadBytes(vaddr, reinterpret_cast<uint8_t*>(arr), sz * sizeof(T));
  }

  [[nodiscard]] zx_status_t ReadString(uintptr_t vaddr, char* str, size_t limit) {
    char ch;
    size_t i = 0;
    do {
      if (i >= limit) {
        str[i - 1] = '\0';
        break;
      }
      zx_status_t status = Read(vaddr + i, &ch);
      if (status != ZX_OK) {
        return status;
      }
      str[i] = ch;
      i++;
    } while (ch != '\0');
    return ZX_OK;
  }

 private:
  const zx::process& process_;
  uint8_t window_[kWindowSize];
  uintptr_t window_start_ = 0;
  size_t window_size_ = 0;

  zx_status_t ReadBytes(uintptr_t vaddr, uint8_t* mem, size_t size) {
    if (vaddr >= window_start_ && vaddr - window_start_ < window_size_) {
      size_t from_window_size = std::min(size, window_size_ - (vaddr - window_start_));
      memcpy(mem, window_ + (vaddr - window_start_), from_window_size);
      vaddr += from_window_size;
      mem += from_window_size;
      size -= from_window_size;
    }
    while (size > 0) {
      // TODO(jakehehrlich): Only read into window on the last iteration of this loop.
      size_t actual;
      zx_status_t status = process_.read_memory(vaddr, window_, kWindowSize, &actual);
      if (status != ZX_OK) {
        return status;
      }
      window_start_ = vaddr;
      window_size_ = actual;
      size_t bytes_read = std::min(actual, size);
      memcpy(mem, window_, bytes_read);
      vaddr += bytes_read;
      mem += bytes_read;
      size -= bytes_read;
    }
    return ZX_OK;
  }
};

[[nodiscard]] zx_status_t GetBuildID(ProcessMemReader* reader, uintptr_t base,
                                     const Elf64_Phdr& notes, uint8_t* buildID,
                                     size_t* buildIDSize) {
  TRACE_DURATION("elf-search", __func__);
  auto NoteAlign = [](uint32_t x) { return (x + 3) & -4; };
  // TODO(jakehehrlich): Sanity check here that notes.p_vaddr falls in the
  // [p_vaddr,p_vaddr+p_filesz) range of some RO PT_LOAD.
  // TODO(jakehehrlich): Check that base is actually the bias and do something to alert the user to
  // base not being the bias.
  uintptr_t vaddr = base + notes.p_vaddr;
  uintptr_t end = vaddr + notes.p_filesz;
  // If the virtual address we found is not aligned or the ending overflowed return early.
  if ((vaddr & 3) || end < vaddr) {
    return ZX_ERR_NOT_FOUND;
  }
  while (end - vaddr > sizeof(Elf64_Nhdr)) {
    Elf64_Nhdr nhdr;
    zx_status_t status = reader->Read(vaddr, &nhdr);
    if (status != ZX_OK) {
      return status;
    }
    vaddr += sizeof(Elf64_Nhdr);
    if (end - vaddr < NoteAlign(nhdr.n_namesz)) {
      break;
    }
    uintptr_t nameAddr = vaddr;
    vaddr += NoteAlign(nhdr.n_namesz);
    if (end - vaddr < NoteAlign(nhdr.n_descsz)) {
      break;
    }
    uintptr_t descAddr = vaddr;
    vaddr += NoteAlign(nhdr.n_descsz);
    // TODO(jakehehrlich): If descsz is larger than kMaxBuildIDSize but
    // the type and name indicate that this note entry is a build ID we
    // should log a warning to the user.
    if (nhdr.n_type == NT_GNU_BUILD_ID && nhdr.n_namesz == sizeof(ELF_NOTE_GNU) &&
        nhdr.n_descsz <= kMaxBuildIDSize) {
      char name[sizeof(ELF_NOTE_GNU)];
      status = reader->ReadArray(nameAddr, name, nhdr.n_namesz);
      if (status != ZX_OK) {
        return status;
      }
      if (memcmp(name, ELF_NOTE_GNU, nhdr.n_namesz) == 0) {
        status = reader->ReadArray(descAddr, buildID, nhdr.n_descsz);
        if (status != ZX_OK) {
          return status;
        }
        *buildIDSize = nhdr.n_descsz;
        return ZX_OK;
      }
    }
  }
  return ZX_ERR_NOT_FOUND;
}

Elf64_Ehdr UpcastElf32Ehdr(const Elf32_Ehdr& to_convert) {
  Elf64_Ehdr converted;
  converted.e_type = to_convert.e_type;
  converted.e_machine = to_convert.e_machine;
  converted.e_version = to_convert.e_version;
  converted.e_entry = to_convert.e_entry;
  converted.e_phoff = to_convert.e_phoff;
  converted.e_shoff = to_convert.e_shoff;
  converted.e_flags = to_convert.e_flags;
  converted.e_ehsize = to_convert.e_ehsize;
  converted.e_phentsize = to_convert.e_phentsize;
  converted.e_phnum = to_convert.e_phnum;
  converted.e_shentsize = to_convert.e_shentsize;
  converted.e_shnum = to_convert.e_shnum;
  converted.e_shstrndx = to_convert.e_shstrndx;
  return converted;
}

Elf64_Phdr UpcastElf32Phdr(const Elf32_Phdr& to_convert) {
  Elf64_Phdr converted;
  converted.p_type = to_convert.p_type;
  converted.p_offset = to_convert.p_offset;
  converted.p_vaddr = to_convert.p_vaddr;
  converted.p_paddr = to_convert.p_paddr;
  converted.p_filesz = to_convert.p_filesz;
  converted.p_memsz = to_convert.p_memsz;
  converted.p_flags = to_convert.p_flags;
  converted.p_align = to_convert.p_align;
  return converted;
}

Elf64_Dyn UpcastElf32Dyn(const Elf32_Dyn& to_convert) {
  Elf64_Dyn converted;
  converted.d_tag = to_convert.d_tag;
  memcpy(&converted.d_un, &to_convert.d_un, sizeof(to_convert.d_un));
  return converted;
}

void DoActionForModule(ProcessMemReader& reader, const zx_info_maps_t& current_map,
                       const cpp20::span<zx_info_maps_t> next_maps, zx_vaddr_t& end_of_last_module,
                       const ModuleAction& action) {
  zx_status_t status;

  // First probe the IDENT header to see if this is a 64 or 32 bit module.
  uint8_t e_ident[EI_NIDENT];
  status = reader.ReadArray(current_map.base, e_ident, sizeof(e_ident));
  if (status != ZX_OK) {
    return;
  }

  const bool is_64bit = e_ident[EI_CLASS] == ELFCLASS64;

  // Read in what might be an ELF header.
  Elf64_Ehdr ehdr;
  if (is_64bit) {
    status = reader.Read(current_map.base, &ehdr);
    if (status != ZX_OK) {
      return;
    }

    // Do some basic checks to see if this could ever be an ELF file.
    if (!IsPossibleLoadedEhdr(ehdr)) {
      return;
    }
  } else {
    Elf32_Ehdr ehdr32;
    status = reader.Read(current_map.base, &ehdr32);
    if (status != ZX_OK) {
      return;
    }

    // Do some basic checks to see if this could ever be an ELF file. Make sure to verify against
    // the expected ELF 32 bit header before upcasting to the 64 bit header.
    if (!IsPossibleLoadedEhdr(ehdr32)) {
      return;
    }

    ehdr = UpcastElf32Ehdr(ehdr32);
  }

  // We only support ELF files with <= 16 program headers.
  // TODO(jakehehrlich): Log this because with the exception of core dumps
  // almost nothing should get here *and* have such a large number of phdrs.
  // This might indicate a larger issue.
  if (ehdr.e_phnum > kMaxProgramHeaders) {
    return;
  }
  Elf64_Phdr phdrs_buf[kMaxProgramHeaders];
  auto phdrs = cpp20::span<const Elf64_Phdr>{phdrs_buf, ehdr.e_phnum};

  if (is_64bit) {
    status = reader.ReadArray(current_map.base + ehdr.e_phoff, phdrs_buf, ehdr.e_phnum);
    if (status != ZX_OK) {
      return;
    }
  } else {
    Elf32_Phdr phdrs32_buf[kMaxProgramHeaders];
    auto phdrs32 = std::span<const Elf32_Phdr>{phdrs32_buf, ehdr.e_phnum};

    status = reader.ReadArray(current_map.base + ehdr.e_phoff, phdrs32_buf, ehdr.e_phnum);
    if (status != ZX_OK) {
      return;
    }

    for (size_t i = 0; i < phdrs32.size(); i++) {
      phdrs_buf[i] = UpcastElf32Phdr(phdrs32[i]);
    }
  }

  // Read the PT_DYNAMIC.
  uintptr_t dynamic = 0;
  size_t dynamic_count = 0;
  uintptr_t vaddr_start = -1ul;
  size_t next_map_index = 0;
  const size_t page_size = zx_system_get_page_size();
  const size_t size_of_dyn = is_64bit ? sizeof(Elf64_Dyn) : sizeof(Elf32_Dyn);
  for (size_t i = 0; i < phdrs.size(); i++) {
    const auto& phdr = phdrs[i];
    if (phdr.p_type == PT_DYNAMIC) {
      dynamic = current_map.base + phdr.p_vaddr;
      dynamic_count = phdr.p_filesz / size_of_dyn;
      break;
    }
    // Update end_of_last_module.
    if (phdr.p_type == PT_LOAD) {
      if (vaddr_start == -1ul) {
        // The first p_vaddr may not be 0.
        vaddr_start = phdr.p_vaddr & -page_size;
      }

      zx_vaddr_t phdr_start = current_map.base - vaddr_start + phdr.p_vaddr;
      phdr_start = phdr_start & -page_size;

      // Inspect the next mappings after the current one to check that the starting addrs match.
      // This is important to distinguish when a single module has been broken up to discontiguous
      // mappings so that |end_of_last_module| doesn't cause us to ignore any modules that have been
      // placed in the intermediate mappings.
      //
      // An example of this looks something like this:
      //      Start              End  Prot   Size   Koid       Offset  Cmt.Pgs  Name
      // 0xdfe52000       0xdfe6e000  r-x    112K  82411          0x0        0  blob-7736e79f
      // 0xdfe78000       0xdfe7a000  rw-      8K  83515          0x0        1  starnix-anon
      // 0xdfe7a000       0xdfe7c000  rw-      8K  83473          0x0        2  starnix-anon
      // 0xdfe7c000       0xdfe7d000  r--      4K   1093          0x0        0  time_values
      // 0xdfe7d000       0xdfe7e000  r--      4K  81533          0x0        1  starnix:vsdo
      // 0xdfe7e000       0xdfe80000  r-x      8K  83468          0x0        0  blob-814b69fa
      // 0xdfe80000       0xdfe82000  r--      8K  83467          0x0        2  data:blob-7736e79f
      // 0xdfe82000       0xdfe83000  rw-      4K  83467       0x2000        1  data:blob-7736e79f
      //
      // Notice how blob-7736e79f has an interleaved module, namely blob-814b69fa. We want to make
      // sure the end_of_last_module stops at 0xdfe6e000 in this example, rather than purely looking
      // at phdrs to see the actual end of this module at 0xdfe83000.
      if (i > 0 && end_of_last_module != 0 && next_map_index < next_maps.size()) {
        const auto& next_map = next_maps[next_map_index++];
        if (next_map.base != phdr_start) {
          break;
        }
      }

      end_of_last_module = current_map.base - vaddr_start + phdr.p_vaddr + phdr.p_memsz;
      // Round up to pages.
      end_of_last_module = (end_of_last_module + page_size - 1) & -page_size;
    }
  }

  // If we didn't find a |PT_DYNAMIC| above, make sure we iterate over everything now.
  if (dynamic == 0) {
    for (const auto& phdr : phdrs) {
      if (phdr.p_type == PT_DYNAMIC) {
        dynamic = current_map.base + phdr.p_vaddr;
        dynamic_count = phdr.p_filesz / size_of_dyn;
        break;
      }
    }
  }

  uintptr_t strtab = 0;
  Elf64_Xword soname_offset = 0;
  if (dynamic != 0) {
    for (size_t i = 0; i < dynamic_count; i++) {
      Elf64_Dyn dyn;
      if (is_64bit) {
        status = reader.Read(dynamic + i * size_of_dyn, &dyn);
      } else {
        Elf32_Dyn dyn32;
        status = reader.Read(dynamic + i * size_of_dyn, &dyn32);
        dyn = UpcastElf32Dyn(dyn32);
      }
      if (status != ZX_OK) {
        break;
      }
      if (dyn.d_tag == DT_STRTAB) {
        // Glibc will relocate the entries in the dynamic table if it's not readonly. Other libc's
        // such as bionic or musl won't. Use a heuristic here to detect: if the value is larger
        // than map.base, it's considered an address. Otherwise it's an offset.
        if (dyn.d_un.d_val >= current_map.base) {
          strtab = dyn.d_un.d_val;
        } else {
          strtab = current_map.base + dyn.d_un.d_val;
        }
      } else if (dyn.d_tag == DT_SONAME) {
        soname_offset = dyn.d_un.d_val;
      } else if (dyn.d_tag == DT_NULL) {
        break;
      }
    }
  }

  // Look for a DT_SONAME.
  char soname[kMaxSonameSize] = "";
  if (strtab != 0 && soname_offset != 0) {
    status = reader.ReadString(strtab + soname_offset, soname, sizeof(soname));
    // Ignore status, if it fails we get an empty soname which falls back to the VMO name below.
    // TODO(tbodt): log when this happens.
  }

  // Loop though program headers looking for a build ID.
  uint8_t build_id_buf[kMaxBuildIDSize];
  cpp20::span<const uint8_t> build_id;
  for (const auto& phdr : phdrs) {
    if (phdr.p_type == PT_NOTE) {
      size_t size;
      status = GetBuildID(&reader, current_map.base, phdr, build_id_buf, &size);
      if (status == ZX_OK && size != 0) {
        build_id = cpp20::span<const uint8_t>(build_id_buf, size);
        break;
      }
    }
  }
  // We're not considering otherwise valid files with no build id here.
  // TODO(jakehehrlich): Consider reporting loaded modules with no build ID.
  if (build_id.empty()) {
    return;
  }

  char name[kNameBufferSize];
  if (soname[0] != '\0') {
    snprintf(name, sizeof(name), "%s", soname);
  } else if (current_map.name[0] != '\0') {
    snprintf(name, sizeof(name), "<VMO#%" PRIu64 "=%s>", current_map.u.mapping.vmo_koid,
             current_map.name);
  } else {
    snprintf(name, sizeof(name), "<VMO#%" PRIu64 ">", current_map.u.mapping.vmo_koid);
  }

  // All checks have passed so we can give the user a module.
  action(ModuleInfo{
      .name = name,
      .vaddr = current_map.base,
      .build_id = build_id,
      .ehdr = ehdr,
      .phdrs = phdrs,
  });
}

}  // anonymous namespace

zx_status_t ForEachModule(const zx::process& process, ModuleAction action) {
  TRACE_DURATION("elf-search", __PRETTY_FUNCTION__);
  Searcher searcher;
  return searcher.ForEachModule(process, std::move(action));
}

zx_status_t Searcher::Reserve(size_t target_size) {
  TRACE_DURATION("elf-search", "AllocateBuffer");
  if (target_size > capacity_) {
    fbl::AllocChecker ac;
    zx_info_maps* new_buffer = new (&ac) zx_info_maps[target_size];
    if (!ac.check()) {
      return ZX_ERR_NO_MEMORY;
    }
    maps_.reset(new_buffer);
    capacity_ = target_size;
  }
  return ZX_OK;
}

zx_status_t Searcher::ForEachModule(const zx::process& process, ModuleAction action) {
  TRACE_DURATION("elf-search", __PRETTY_FUNCTION__);
  ProcessMemReader reader(process);

  zx_status_t status;
  size_t actual, avail = 0;
  do {
    // On the first pass of this loop with a freshly constructed Searcher, this will be a no-op.
    status = Reserve(avail);
    if (status != ZX_OK) {
      return status;
    }

    TRACE_DURATION("elf-search", "ReadProcessMaps");
    status = process.get_info(ZX_INFO_PROCESS_MAPS, maps_.get(), capacity_ * sizeof(zx_info_maps),
                              &actual, &avail);
    if (status != ZX_OK) {
      return status;
    }
  } while (avail > actual);
  // TODO(jakehehrlich): Check permissions of program headers to make sure they agree with mappings.
  // 'maps' should be sorted in ascending order of base address so we should be able to use that to
  // quickly find the mapping associated with any given PT_LOAD.

  // When `-z noseparate-code` is enabled, multiple ELF segments could live on the same page and
  // the same ELF header gets mapped multiple times with different flags.  end_of_last_module tracks
  // the end of last module and regions overlapping with the last module will be skipped.
  zx_vaddr_t end_of_last_module = 0;

  for (size_t i = 0; i < actual; ++i) {
    TRACE_DURATION("elf-search", "IterateMaps");
    const auto& map = maps_[i];

    // Skip regions overlapping with the last module to avoid parsing the same ELF header twice.
    if (map.base < end_of_last_module) {
      continue;
    }

    // Skip any writable maps since the RODATA segment containing the
    // headers will not be writable.
    if (map.type != ZX_INFO_MAPS_TYPE_MAPPING) {
      continue;
    }
    if ((map.u.mapping.mmu_flags & ZX_VM_PERM_WRITE) != 0) {
      continue;
    }
    // Skip any mapping that doesn't start at the beginning of a VMO.
    // We assume that the VMO represents the ELF file. ELF headers
    // always start at the beginning of the file so if our assumption
    // holds then we can't be looking at the start of an ELF header if
    // the offset into the VMO isn't also zero.
    if (map.u.mapping.vmo_offset != 0) {
      continue;
    }

    auto next_maps = std::span<zx_info_maps_t>(maps_.get() + i + 1, actual - (i + 1));
    DoActionForModule(reader, map, next_maps, end_of_last_module, action);
  }

  return ZX_OK;
}

}  // namespace elf_search
