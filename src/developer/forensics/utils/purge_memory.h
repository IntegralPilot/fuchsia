// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_UTILS_PURGE_MEMORY_H_
#define SRC_DEVELOPER_FORENSICS_UTILS_PURGE_MEMORY_H_

#include <lib/async/dispatcher.h>
#include <lib/zx/time.h>

namespace forensics {

// Purges unused memory using M_PURGE_ALL.
void PurgeAllMemoryAfter(async_dispatcher_t* dispatcher, zx::duration delay);

}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_UTILS_PURGE_MEMORY_H_
