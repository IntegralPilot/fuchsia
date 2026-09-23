// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_SHARED_PEERED_OBJECT_WATCHER_H_
#define SRC_DEVELOPER_DEBUG_SHARED_PEERED_OBJECT_WATCHER_H_

#if !defined(__Fuchsia__)
#error peered_object_watcher.h can only be included on Fuchsia.
#endif

#include <zircon/types.h>

namespace debug {

// A base class for all Zircon peered objects. See
// https://fuchsia.dev/fuchsia-src/reference/kernel_objects/objects for the canonical definition.
// This base class is inherited by all "Watcher" classes that receive events about zircon peered
// objects from the message loop.
class PeeredObjectWatcher {
 public:
  virtual void OnPeerClosed(zx_handle_t) = 0;
};

}  // namespace debug
#endif  // SRC_DEVELOPER_DEBUG_SHARED_PEERED_OBJECT_WATCHER_H_
