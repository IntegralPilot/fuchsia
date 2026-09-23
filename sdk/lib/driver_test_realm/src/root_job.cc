// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver_test_realm/src/root_job.h>
#include <lib/syslog/cpp/macros.h>

namespace driver_test_realm {

void RootJob::Get(GetCompleter::Sync& completer) {
  zx::job job;
  zx_status_t status = zx::job::default_job()->duplicate(ZX_RIGHT_SAME_RIGHTS, &job);
  if (status != ZX_OK) {
    FX_LOG_KV(ERROR, "Failed to duplicate job", FX_KV("status", status));
  }
  completer.Reply(std::move(job));
}

}  // namespace driver_test_realm
