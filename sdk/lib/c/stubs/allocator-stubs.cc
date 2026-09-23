// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zircon-internal/unique-backtrace.h>
#include <zircon/compiler.h>

// This file contains stubs for the allocator API entry points in the libc ABI.
// libc.so can be dynamically linked against a shared library that provides the
// actual allocator implementations. These need to be provided so there won't be
// any dangling references in libc at startup time. Weak definitions will be
// replaced with external definitions in the final phase of dynamic linking, so
// these functions won't be called at all.
#define TRAP_STUB(name) \
  __EXPORT __WEAK void name(void) { CRASH_WITH_UNIQUE_BACKTRACE(); }

extern "C" {

__WEAK __attribute__((visibility("hidden"))) void __libc_init_gwp_asan(void) {}

// Although this isn't a common allocator function, this is expected in the libc.ifs file.
void __scudo_print_stats(void) { CRASH_WITH_UNIQUE_BACKTRACE(); }

TRAP_STUB(aligned_alloc)
TRAP_STUB(calloc)
TRAP_STUB(malloc)
TRAP_STUB(realloc)
TRAP_STUB(reallocarray)
TRAP_STUB(memalign)
TRAP_STUB(posix_memalign)
TRAP_STUB(pvalloc)
TRAP_STUB(valloc)
TRAP_STUB(free)
TRAP_STUB(free_aligned_sized)
TRAP_STUB(free_sized)
TRAP_STUB(malloc_disable)
TRAP_STUB(malloc_disable_memory_tagging)
TRAP_STUB(malloc_enable)
TRAP_STUB(malloc_info)
TRAP_STUB(malloc_iterate)
TRAP_STUB(malloc_set_add_large_allocation_slack)
TRAP_STUB(malloc_set_pattern_fill_contents)
TRAP_STUB(malloc_set_track_allocation_stacks)
TRAP_STUB(malloc_set_zero_contents)
TRAP_STUB(malloc_usable_size)
TRAP_STUB(mallopt)
TRAP_STUB(mallinfo)
TRAP_STUB(mallinfo2)

}  // extern "C"
