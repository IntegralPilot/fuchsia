// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_MEMORY_HEAPDUMP_INSTRUMENTATION_INCLUDE_HEAPDUMP_BIND_H_
#define SRC_PERFORMANCE_MEMORY_HEAPDUMP_INSTRUMENTATION_INCLUDE_HEAPDUMP_BIND_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// Binds the current process to the provided process registry.
//
// `registry_channel` must be the client end of a `fuchsia.memory.heapdump.process.Registry` channel
// connected to heapdump's collector. Calling this function is necessary to make the current process
// visible to the collector and, in turn, to its `ffx profile heapdump` client commands.
//
// Since a process cannot be bound to multiple registries, this function can only be called at most
// once during the lifetime of a process. This is enforced by the function itself, which will print
// an error and abort if called twice.
//
// Note: Allocations and snapshots captured before calling this function are internally buffered.
// Therefore, while it is expected that programs will call this function as early as possible, no
// data will be lost if they start to allocate before doing so.
//
// Passing ZX_HANDLE_INVALID will release all the resources that were used to internally buffer
// previous allocations and snapshots and disable tracking of future ones.
void heapdump_bind_with_channel(zx_handle_t registry_channel);

// Binds the current process to the process registry, using `fdio_service_connect` to locate it.
//
// This function wraps `heapdump_bind_with_channel` and implements the common case of using fdio to
// connect to the process registry.
//
// If the relevant capability is not in the namespace, it becomes equivalent to
// `heapdump_bind_with_channel(ZX_HANDLE_INVALID)`.
void heapdump_bind_with_fdio(void);

__END_CDECLS

#endif  // SRC_PERFORMANCE_MEMORY_HEAPDUMP_INSTRUMENTATION_INCLUDE_HEAPDUMP_BIND_H_
