// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_MEMORY_SCUDO_MALLOPT_H_
#define SRC_PERFORMANCE_MEMORY_SCUDO_MALLOPT_H_

extern "C" {
// mallopt() adjust parameters or performs specific maintenance operations on the memory
// allocator. Returns 1 on success, 0 on error.
int mallopt(int /*param*/, int /*value*/);
}

#ifndef M_DECAY_TIME
// mallopt() option to set the minimum time between attempts to release unused pages to
// to the operating system in milliseconds. The value is clamped on allocator config
// `MinReleaseToOsIntervalMs` and `MinReleaseToOsIntervalMs`.
#define M_DECAY_TIME (-100)
#endif

#ifndef M_PURGE
// mallopt() option to immediately purge any memory not in use. This will release the memory back
// to the kernel. The value is ignored.
#define M_PURGE (-101)
#endif

#ifndef M_PURGE_ALL
// mallopt() option to immediately purge all possible memory back to the kernel. This call can take
// longer than a normal purge since it examines everything. In some cases, it can take more than
// twice the time of a M_PURGE call. The value is ignored.
#define M_PURGE_ALL (-104)
#endif

#endif  // SRC_PERFORMANCE_MEMORY_SCUDO_MALLOPT_H_
