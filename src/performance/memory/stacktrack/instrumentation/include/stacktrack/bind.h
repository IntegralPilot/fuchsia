// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_MEMORY_STACKTRACK_INSTRUMENTATION_INCLUDE_STACKTRACK_BIND_H_
#define SRC_PERFORMANCE_MEMORY_STACKTRACK_INSTRUMENTATION_INCLUDE_STACKTRACK_BIND_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// Binds the current process to the provided process registry.
//
// `registry_channel` must be the client end of a `fuchsia.starnix.stacktrack.Registry` channel
// connected to stacktrack's collector. Calling this function is necessary to make the current
// process visible to the collector.
//
// Since a process cannot be bound to multiple registries, this function can only be called at most
// once during the lifetime of a process.
//
// `registry_channel` must be a valid handle.
void stacktrack_bind_with_channel(zx_handle_t registry_channel);

// Binds the current process to the process registry, using `fdio_service_connect` to locate it.
//
// This function wraps `stacktrack_bind_with_channel` and implements the common case of using fdio
// to connect to the process registry.
void stacktrack_bind_with_fdio(void);

__END_CDECLS

#endif  // SRC_PERFORMANCE_MEMORY_STACKTRACK_INSTRUMENTATION_INCLUDE_STACKTRACK_BIND_H_
