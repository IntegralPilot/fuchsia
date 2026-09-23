// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdlib.h>
#include <openthread/platform/memory.h>
/**
 * Dynamically allocates new memory. On platforms that support it, should just redirect to calloc. For
 * those that don't support calloc, should support the same functionality:
 *
 *   "The calloc() function contiguously allocates enough space for count objects that are size bytes of
 *   memory each and returns a pointer to the allocated memory. The allocated memory is filled with bytes
 *   of value zero."
 *
 * This function is required for OPENTHREAD_CONFIG_HEAP_EXTERNAL_ENABLE.
 *
 * @param[in] aNum   The number of blocks to allocate
 * @param[in] aSize  The size of each block to allocate
 *
 * @retval void*  The pointer to the front of the memory allocated
 * @retval NULL   Failed to allocate the memory requested.
 */
void *otPlatCAlloc(size_t aNum, size_t aSize) {
    return calloc(aNum, aSize);
}

/**
 * Frees memory that was dynamically allocated.
 *
 * This function is required for OPENTHREAD_CONFIG_HEAP_EXTERNAL_ENABLE.
 *
 * @param[in] aPtr  A pointer the memory blocks to free. The pointer may be NULL.
 */
void otPlatFree(void *aPtr) {
    free(aPtr);
}
