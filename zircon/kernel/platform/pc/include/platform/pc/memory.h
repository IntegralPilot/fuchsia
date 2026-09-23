// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PLATFORM_PC_INCLUDE_PLATFORM_PC_MEMORY_H_
#define ZIRCON_KERNEL_PLATFORM_PC_INCLUDE_PLATFORM_PC_MEMORY_H_

#include <sys/types.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <ktl/span.h>

// Forward declaration; defined in <lib/memalloc/range.h>
namespace memalloc {
struct Range;
}

void pc_mem_init(ktl::span<const memalloc::Range> ranges);

#endif  // ZIRCON_KERNEL_PLATFORM_PC_INCLUDE_PLATFORM_PC_MEMORY_H_
