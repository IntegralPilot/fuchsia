// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/purge_memory.h"

#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include "src/performance/memory/scudo/mallopt.h"

namespace forensics {

void PurgeAllMemoryAfter(async_dispatcher_t* dispatcher, const zx::duration delay) {
  async::PostDelayedTask(
      dispatcher,
      [] {
        FX_LOGS(INFO) << "Purging memory";
        mallopt(M_PURGE_ALL, 0);
        FX_LOGS(INFO) << "Finished purging memory";
      },
      delay);
}

}  // namespace forensics
