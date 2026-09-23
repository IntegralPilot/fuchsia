// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FZL_MEMORY_PROBE_H_
#define LIB_FZL_MEMORY_PROBE_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// Returns ZX_OK if the byte at the given address can be read or written, or the exceptions's
// error code if it generates a page fault. Any other type of error results in a test termination
// due to assert failure.
//
// These functions are designed for testing purposes. They are very heavyweight since they spin up a
// thread to attempt the memory access.
//
// The write probe is implemented as a non-atomic read/write of the same value. If the address could
// be modified by a different thread, or if it falls in the region of the stack used by the
// implementation of probe_for_write itself (since the probe is asynchronous), the non-atomic
// read/write can corrupt the data.
zx_status_t probe_for_read(const void* addr);
zx_status_t probe_for_write(void* addr);

__END_CDECLS

#endif  // LIB_FZL_MEMORY_PROBE_H_
