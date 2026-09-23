// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/monitor/imminent_oom_observer.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

namespace monitor {

ImminentOomEventObserver::ImminentOomEventObserver(zx_handle_t imminent_oom_event_handle)
    : imminent_oom_event_handle_(imminent_oom_event_handle) {}

bool ImminentOomEventObserver::IsImminentOom() {
  zx_signals_t observed;
  zx_status_t status = zx_object_wait_one(imminent_oom_event_handle_, ZX_EVENT_SIGNALED,
                                          ZX_TIME_INFINITE_PAST, &observed);
  // With a deadline of ZX_TIME_INFINITE_PAST, it is expected that this call times out.
  if (status != ZX_OK && status != ZX_ERR_TIMED_OUT) {
    FX_LOGS(ERROR) << "zx_object_wait_one returned " << zx_status_get_string(status);
    return false;
  }

  return observed & ZX_EVENT_SIGNALED;
}

}  // namespace monitor
