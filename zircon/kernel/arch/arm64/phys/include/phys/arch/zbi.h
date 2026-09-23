// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_ZBI_H_
#define ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_ZBI_H_

#ifdef __ASSEMBLER__
#include <zbi/asm/zbi.h>
#else
#include <lib/zbi-format/zbi.h>
#endif

#define ARCH_ZBI_KERNEL_TYPE (ZBI_TYPE_KERNEL_ARM64)

// Alignment required for an arm64 kernel ZBI.
#define ARCH_ZBI_KERNEL_ALIGNMENT (1 << 16)

// Alignment required for an arm64 data ZBI.
#define ARCH_ZBI_DATA_ALIGNMENT (1 << 12)

#endif  // ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_ZBI_H_
