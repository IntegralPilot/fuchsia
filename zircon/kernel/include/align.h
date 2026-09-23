// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_ALIGN_H_
#define ZIRCON_KERNEL_INCLUDE_ALIGN_H_

#include <arch/defines.h>

#define ROUNDUP(a, b) (((a) + ((b) - 1)) & ~((b) - 1))
#define ROUNDDOWN(a, b) ((a) & ~((b) - 1))

#define IS_ROUNDED(a, b) (!(((uintptr_t)(a)) & (((uintptr_t)(b)) - 1)))

// Use to align structures on cache lines to avoid cpu aliasing.
#define __CPU_ALIGN __ALIGNED(MAX_CACHE_LINE)

// Similar to above, but put in a special section to make sure no other
// variable shares the same cache line.
#define __CPU_ALIGN_EXCLUSIVE __CPU_ALIGN __SECTION(".data.cpu_align_exclusive")

#endif  // ZIRCON_KERNEL_INCLUDE_ALIGN_H_
