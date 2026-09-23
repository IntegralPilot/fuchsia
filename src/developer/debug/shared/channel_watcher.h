// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_SHARED_CHANNEL_WATCHER_H_
#define SRC_DEVELOPER_DEBUG_SHARED_CHANNEL_WATCHER_H_

#if !defined(__Fuchsia__)
#error channel_watcher.h can only be included on Fuchsia.
#endif

#include <zircon/types.h>

#include "src/developer/debug/shared/peered_object_watcher.h"

namespace debug {
class ChannelWatcher : public PeeredObjectWatcher {
 public:
  virtual void OnChannelReadable(zx_handle_t) = 0;

  virtual ~ChannelWatcher() = default;
};
}  // namespace debug
#endif  // SRC_DEVELOPER_DEBUG_SHARED_CHANNEL_WATCHER_H_
