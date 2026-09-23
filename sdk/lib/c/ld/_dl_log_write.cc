// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/processargs.h>

#include "dynlink.h"
#include "log.h"

using LIBC_NAMESPACE::gLog;

void _dl_log_write(const char* buffer, size_t len) { gLog({buffer, len}); }

// This is called by __dls3 for any PA_FD handle in the bootstrap message.
void _dl_log_write_init(zx_handle_t handle, uint32_t info) {
  zx::handle fd{handle};
  if (ld::IsProcessargsLogHandle(info)) {
    gLog.TakeLogHandle(std::move(fd));
  }
}

// This is called after the last chance to call _dl_log_write_init.
void _dl_log_write_init_fallback() {
  if (gLog) {
    return;
  }

  // TODO(https://fxbug.dev/42107086): For a long time this has always used a
  // kernel log channel when nothing else is passed in the bootstrap message.
  // This still needs to be replaced by a proper unprivileged logging scheme.
  zx::debuglog debuglog;
  zx::debuglog::create(zx::resource{}, 0, &debuglog);
  gLog.set_debuglog(std::move(debuglog));
}
