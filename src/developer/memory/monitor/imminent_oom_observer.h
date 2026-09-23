// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_MEMORY_MONITOR_IMMINENT_OOM_OBSERVER_H_
#define SRC_DEVELOPER_MEMORY_MONITOR_IMMINENT_OOM_OBSERVER_H_

#include <zircon/types.h>

namespace monitor {

class ImminentOomObserver {
 public:
  virtual ~ImminentOomObserver() = default;

  virtual bool IsImminentOom() = 0;
};

class ImminentOomEventObserver : public ImminentOomObserver {
 public:
  explicit ImminentOomEventObserver(zx_handle_t imminent_oom_event_handle);
  bool IsImminentOom() override;

 private:
  zx_handle_t imminent_oom_event_handle_;
};

}  // namespace monitor

#endif  // SRC_DEVELOPER_MEMORY_MONITOR_IMMINENT_OOM_OBSERVER_H_
