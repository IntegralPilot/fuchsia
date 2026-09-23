// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_DL_PHDR_INFO_H_
#define LIB_LD_DL_PHDR_INFO_H_

#include <lib/elfldltl/layout.h>
#include <link.h>

#include "abi.h"
#include "module.h"

namespace ld {

// The `dlpi_adds` and `dlpi_subs` members of `struct dl_phdr_info` represent a
// sample of the current state of history.  The current values are passed in
// every callback that dl_iterate_phdr() makes, unrelated to the specific
// module whose information is being reported.  Callbacks can compare these to
// previous runs to tell if any modules have newly been loaded or unloaded, so
// as to decide to short-circuit a dl_iterate_phdr() run when nothing changed.
struct DlPhdrInfoCounts {
  constexpr auto operator<=>(const DlPhdrInfoCounts&) const = default;

  uint64_t adds = 0;
  uint64_t subs = 0;
};

// This yields the current counts as of program startup, from ld::abi::_ld_abi.
template <class Elf, class AbiTraits>
constexpr DlPhdrInfoCounts DlPhdrInfoInitialExecCounts(const abi::Abi<Elf, AbiTraits>& abi) {
  return {.adds = abi.loaded_modules_count};
}

// Form the dl_iterate_phdr API's data structure from passive ABI module data.
//
// The caller must compute the TLS data pointer.  For the initial-exec set
// described in the passive ABI, see <lib/ld/tls.h> ld::TlsInitialExecData.
//
// The counts can be an instantaneous sample for this call, or can be a single
// sample taken at the beginning of the iteration and passed to all callbacks.
// For the initial-exec set only, use ld::DlPhdrInfoInitialExecCounts (above).
inline dl_phdr_info MakeDlPhdrInfo(const abi::Abi<>::Module& module, void* tls_data,
                                   DlPhdrInfoCounts counts) {
  constexpr auto safe_phnum = [](size_t count) -> uint16_t {
    if (count < 0xffff) [[likely]] {
      return static_cast<uint16_t>(count);
    }
    // A special value marks a larger count.  The dl_phdr_info has no way to
    // communicate the actual count, which is stored in a Shdr in the file.
    return elfldltl::Elf<>::Ehdr::kPnXnum;
  };
  return {
      .dlpi_addr = module.link_map.addr,
      .dlpi_name = module.link_map.name.get(),
      .dlpi_phdr = reinterpret_cast<const ElfW(Phdr)*>(module.phdrs.data()),
      .dlpi_phnum = safe_phnum(module.phdrs.size()),
      .dlpi_adds = counts.adds,
      .dlpi_subs = counts.subs,
      .dlpi_tls_modid = module.tls_modid,
      .dlpi_tls_data = tls_data,
  };
}

decltype(auto) operator<<(auto& os, DlPhdrInfoCounts counts) {
  return os << "{adds=" << counts.adds << ", subs=" << counts.subs << "}";
}

}  // namespace ld

#endif  // LIB_LD_DL_PHDR_INFO_H_
