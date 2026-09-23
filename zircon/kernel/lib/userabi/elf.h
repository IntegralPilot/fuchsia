// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_USERABI_ELF_H_
#define ZIRCON_KERNEL_LIB_USERABI_ELF_H_

#include <lib/zx/result.h>

#include <ktl/optional.h>
#include <object/handle.h>
#include <object/vm_address_region_dispatcher.h>
#include <vm/handoff-end.h>

struct MappedElf {
  HandleOwner vmar;
  zx_vaddr_t vaddr_start;
  size_t vaddr_size;
  zx_vaddr_t entry;
  ktl::optional<size_t> stack_size;
};

// This creates a VMAR within the given parent VMAR, and maps the ELF file in.
// The new VMAR is returned, so it can be used to change protections for RELRO.
// The reference can just be dropped to ensure no more changes are possible.
zx::result<MappedElf> MapHandoffElf(HandoffEnd::Elf elf, VmAddressRegionDispatcher& parent_vmar);

#endif  // ZIRCON_KERNEL_LIB_USERABI_ELF_H_
