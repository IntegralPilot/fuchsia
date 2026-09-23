// Copyright 2017 The Fuchsia Authors
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_PHYSMAP_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_PHYSMAP_H_

#include <assert.h>
#include <inttypes.h>
#include <stddef.h>
#include <stdint.h>

// The kernel physmap is a region of the kernel where all of useful physical
// memory is linearly mapped for cheap, ready access: in particular, excluding
// non-RAM subranges, the physical range [0, gPhysmapSize) is mapped to the
// virtual [gPhysmapBase, gPhysmapBase + gPhysmapSize), where gPhysmapSize is
// just large enough to capture all of physical RAM. The mapping is set up in
// physboot and then the variables giving its dimensions are set in
// PostHandoffBootstrap().

extern vaddr_t gPhysmapBase;
extern size_t gPhysmapSize;

// check to see if an address is in the physmap virtually and physically
inline bool is_physmap_addr(const void* addr) {
  return reinterpret_cast<uintptr_t>(addr) >= gPhysmapBase &&
         reinterpret_cast<uintptr_t>(addr) - gPhysmapBase < gPhysmapSize;
}

inline bool is_physmap_phys_addr(paddr_t pa) { return pa < gPhysmapSize; }

// physical to virtual, returning pointer in the big kernel map
inline void* paddr_to_physmap(paddr_t pa) {
  DEBUG_ASSERT_MSG(is_physmap_phys_addr(pa), "paddr %#" PRIxPTR "\n", pa);
  return reinterpret_cast<void*>(gPhysmapBase + pa);
}

// given a pointer into the physmap, reverse back to a physical address
inline paddr_t physmap_to_paddr(const void* addr) {
  DEBUG_ASSERT_MSG(is_physmap_addr(addr), "vaddr %p\n", addr);
  return reinterpret_cast<uintptr_t>(addr) - gPhysmapBase;
}

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_PHYSMAP_H_
