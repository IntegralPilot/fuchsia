// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_ARM64_PERIPHMAP_H_
#define ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_ARM64_PERIPHMAP_H_

#include <sys/types.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

// translates peripheral physical address to virtual address in the big kernel map
vaddr_t periph_paddr_to_vaddr(paddr_t paddr);

// translates peripheral virtual address to physical address in the big kernel map
paddr_t periph_vaddr_to_paddr(vaddr_t vaddr);

#endif  // ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_ARM64_PERIPHMAP_H_
