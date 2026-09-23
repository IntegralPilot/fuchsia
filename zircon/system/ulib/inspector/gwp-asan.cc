// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon/system/ulib/inspector/gwp-asan.h"

#include <elf-search.h>

#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <vector>

#include "sdk/lib/c/scudo/gwp_asan_info.h"

namespace inspector {

fit::result<GwpAsanError> inspector_get_gwp_asan_info(const zx::process& process,
                                                      const zx_exception_report_t& exception_report,
                                                      GwpAsanInfo* info) {
  // The address of __libc_gwp_asan_info.
  uint64_t libc_gwp_asan_info_addr = 0;

  // If a page fault was caused because there is no memory available,
  // it's not a gwp-asan error.
  if (exception_report.header.type == ZX_EXCP_FATAL_PAGE_FAULT &&
      static_cast<zx_status_t>(exception_report.context.synth_code) == ZX_ERR_NO_MEMORY) {
    info->error_type = nullptr;
    return fit::ok();
  }

  // Find the GWP-ASan note.
  elf_search::ForEachModule(process, [&](const elf_search::ModuleInfo& info) {
    if (info.name != "libc.so") {
      return;
    }
    for (const auto& phdr : info.phdrs) {
      if (phdr.p_type != PT_NOTE) {
        continue;
      }
      // Read the whole segment.
      std::vector<std::byte> notes(phdr.p_memsz);
      size_t actual;
      if (process.read_memory(info.vaddr + phdr.p_vaddr, notes.data(), notes.size(), &actual) !=
              ZX_OK ||
          actual != notes.size()) {
        return;
      }
      uint64_t p = 0;
      while (p + sizeof(Elf64_Nhdr) <= notes.size()) {
        Elf64_Nhdr& nhdr = reinterpret_cast<Elf64_Nhdr&>(notes[p]);
        p += sizeof(Elf64_Nhdr);
        p += (nhdr.n_namesz + 3) & ~3;
        if (nhdr.n_type == GWP_ASAN_NOTE_TYPE) {
          if (nhdr.n_descsz != sizeof(uint64_t) || p + nhdr.n_descsz > notes.size()) {
            return;
          }
          libc_gwp_asan_info_addr =
              reinterpret_cast<uint64_t&>(notes[p]) + p + info.vaddr + phdr.p_vaddr;
          break;
        }
        p += (nhdr.n_descsz + 3) & ~3;
      }
    }
  });

  if (!libc_gwp_asan_info_addr) {
    return fit::error(GwpAsanInfoAddressNotFound{});
  }

  // Read the __libc_gwp_asan_info.
  gwp_asan::LibcGwpAsanInfo libc_gwp_asan_info;
  size_t actual;
  zx_status_t status = process.read_memory(libc_gwp_asan_info_addr, &libc_gwp_asan_info,
                                           sizeof(libc_gwp_asan_info), &actual);
  if (status != ZX_OK || actual != sizeof(libc_gwp_asan_info)) {
    return fit::error(LibcGwpAsanInfoReadFailed{
        .status = status,
        .libc_gwp_asan_info_addr = libc_gwp_asan_info_addr,
        .actual_size = actual,
    });
  }

  // Read the allocator state.
  gwp_asan::AllocatorState state;
  uintptr_t state_addr = reinterpret_cast<uintptr_t>(libc_gwp_asan_info.state);
  status = process.read_memory(state_addr, &state, sizeof(state), &actual);
  if (status != ZX_OK || actual != sizeof(state)) {
    return fit::error(AllocatorStateReadFailed{
        .status = status,
        .address = state_addr,
        .actual_size = actual,
    });
  }

  // Check the MaxSimultaneousAllocations, the magic and the version.
  // They are not set if GWP-ASan is not enabled.
  using gwp_asan::AllocatorVersionMagic;
  if (state.MaxSimultaneousAllocations == 0 ||
      memcmp(state.VersionMagic.Magic, AllocatorVersionMagic::kAllocatorVersionMagic, 4) != 0 ||
      state.VersionMagic.Version != AllocatorVersionMagic::kAllocatorVersion) {
    std::array<uint8_t, 4> magic;
    memcpy(magic.data(), state.VersionMagic.Magic, 4);
    return fit::error(ValidationFailed{
        .magic = magic,
        .version = state.VersionMagic.Version,
        .max_allocations = state.MaxSimultaneousAllocations,
    });
  }

  uint64_t faulting_addr = 0;
  if (exception_report.header.type == ZX_EXCP_FATAL_PAGE_FAULT) {
#if defined(__x86_64__)
    faulting_addr = exception_report.context.arch.u.x86_64.cr2;
#elif defined(__aarch64__)
    faulting_addr = exception_report.context.arch.u.arm_64.far;
#elif defined(__riscv)
    faulting_addr = exception_report.context.arch.u.riscv_64.tval;
#else
#error What arch?
#endif
  }

  if (!__gwp_asan_error_is_mine(&state, faulting_addr)) {
    info->error_type = nullptr;
    return fit::ok();
  }

  // Read the allocator metadata.
  using gwp_asan::AllocationMetadata;
  std::vector<AllocationMetadata> metadata_list(state.MaxSimultaneousAllocations);
  uintptr_t metadata_addr = reinterpret_cast<uintptr_t>(libc_gwp_asan_info.metadata);
  size_t expected_size = sizeof(AllocationMetadata) * metadata_list.size();
  status = process.read_memory(metadata_addr, metadata_list.data(), expected_size, &actual);
  if (status != ZX_OK || actual != expected_size) {
    return fit::error(MetadataReadFailed{
        .status = status,
        .address = metadata_addr,
        .expected_size = expected_size,
        .actual_size = actual,
    });
  }

  if (!faulting_addr) {
    faulting_addr =
        __gwp_asan_get_internal_crash_address(&state, state.internallyDetectedErrorFaultAddress());
  }

  info->faulting_addr = faulting_addr;
  info->error_type = gwp_asan::ErrorToString(
      __gwp_asan_diagnose_error(&state, metadata_list.data(), faulting_addr));

  const AllocationMetadata* metadata =
      __gwp_asan_get_metadata(&state, metadata_list.data(), faulting_addr);
  if (!metadata) {
    return fit::error(MetadataMappingFailed{
        .faulting_addr = faulting_addr,
    });
  }
  info->allocation_address = __gwp_asan_get_allocation_address(metadata);
  info->allocation_size = __gwp_asan_get_allocation_size(metadata);

  // TODO: Also include the thread id after gwp_asan::getThreadID() is supported on Fuchsia,
  //       which lives in //third_party/scudo/gwp_asan/platform_specific/common_fuchsia.cpp.
  info->allocation_trace.resize(AllocationMetadata::kMaxTraceLengthToCollect);
  size_t trace_size = __gwp_asan_get_allocation_trace(metadata, info->allocation_trace.data(),
                                                      AllocationMetadata::kMaxTraceLengthToCollect);
  info->allocation_trace.resize(trace_size);

  if (__gwp_asan_is_deallocated(metadata)) {
    info->deallocation_trace.resize(AllocationMetadata::kMaxTraceLengthToCollect);
    trace_size = __gwp_asan_get_deallocation_trace(metadata, info->deallocation_trace.data(),
                                                   AllocationMetadata::kMaxTraceLengthToCollect);
    info->deallocation_trace.resize(trace_size);
  } else {
    info->deallocation_trace.resize(0);
  }

  return fit::ok();
}

}  // namespace inspector
