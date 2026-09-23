// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_MEMORY_HEAPDUMP_INSTRUMENTATION_INCLUDE_HEAPDUMP_STATS_H_
#define SRC_PERFORMANCE_MEMORY_HEAPDUMP_INSTRUMENTATION_INCLUDE_HEAPDUMP_STATS_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

struct heapdump_global_stats {
  uint64_t total_allocated_bytes;
  uint64_t total_deallocated_bytes;
};

struct heapdump_thread_local_stats {
  uint64_t total_allocated_bytes;
  uint64_t total_deallocated_bytes;
};

// Obtains stats about past allocations.
//
// `global` will be filled with data about all allocations by the current process.
// `local` will be filled with data about allocations by calling thread only.
//
// It is also possible to pass NULL if the caller does not need the corresponding data.
void heapdump_get_stats(struct heapdump_global_stats *global,
                        struct heapdump_thread_local_stats *local);

__END_CDECLS

#endif  // SRC_PERFORMANCE_MEMORY_HEAPDUMP_INSTRUMENTATION_INCLUDE_HEAPDUMP_STATS_H_
