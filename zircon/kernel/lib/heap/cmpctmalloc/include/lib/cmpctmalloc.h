// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2015 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_HEAP_CMPCTMALLOC_INCLUDE_LIB_CMPCTMALLOC_H_
#define ZIRCON_KERNEL_LIB_HEAP_CMPCTMALLOC_INCLUDE_LIB_CMPCTMALLOC_H_

#include <lib/zircon-internal/thread_annotations.h>
#include <stddef.h>

#include <fbl/enum_bits.h>

#ifdef _KERNEL
#include <kernel/mutex.h>

// This is a popular lock.  Critical sections protected by this lock are
// *usually* short-lived, so we'd like to minimize lock thrash by disabling
// preemption while holding the lock.  However, in degenerate cases (e.g. with
// excessive PMM fragmentation), the lock can be held for long periods of time.
// In those case, we don't want to overly delay the scheduling of a higher
// priority task.  To strike a balance between these competing concerns, we use
// |CriticalMutex| which employs "time-limited preemption deferral".
//
// For more information, see https://fxbug.dev/399694236.
DECLARE_SINGLETON_CRITICAL_MUTEX(TheHeapLock);
#else
#include <mutex>

struct TheHeapLock {
  static std::mutex* Get() {
    static std::mutex m;
    return &m;
  }
};
#endif

enum class CmpctDumpOptions : uint32_t {
  None = 0x0,
  PanicTime = 0x1,
  Verbose = 0x2,
};
FBL_ENABLE_ENUM_BITS(CmpctDumpOptions)

// The maximum size that |cmpct_alloc| can allocate. Any larger of a size would
// yield a nullptr.
extern const size_t kHeapMaxAllocSize;

void* cmpct_alloc(size_t) TA_EXCL(TheHeapLock::Get());
void cmpct_free(void*) TA_EXCL(TheHeapLock::Get());
void cmpct_sized_free(void*, size_t) TA_EXCL(TheHeapLock::Get());
void* cmpct_memalign(size_t alignment, size_t size) TA_EXCL(TheHeapLock::Get());

// Zero-fill allocations smaller than |size|
void cmpct_set_fill_on_alloc_threshold(size_t size);
void cmpct_init(void) TA_EXCL(TheHeapLock::Get());
void cmpct_dump(CmpctDumpOptions options) TA_EXCL(TheHeapLock::Get());
void cmpct_get_info(size_t* used_bytes, size_t* free_bytes, size_t* cached_bytes)
    TA_EXCL(TheHeapLock::Get());
void cmpct_test(void) TA_EXCL(TheHeapLock::Get());

// Get and set a user defined cookie against an allocation returned from cmpct_alloc
// or cmpct_memalign. |ptr| must be non-null and these methods are not thread
// safe.
void cmpct_set_cookie(void* ptr, uint32_t cookie);
uint32_t cmpct_get_cookie(void* ptr);
// Returns the allocation size rounded up to the allocator bucket size.
uint32_t cmpct_get_size(void* ptr);

#endif  // ZIRCON_KERNEL_LIB_HEAP_CMPCTMALLOC_INCLUDE_LIB_CMPCTMALLOC_H_
