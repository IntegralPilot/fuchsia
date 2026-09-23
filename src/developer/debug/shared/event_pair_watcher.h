// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_SHARED_EVENT_PAIR_WATCHER_H_
#define SRC_DEVELOPER_DEBUG_SHARED_EVENT_PAIR_WATCHER_H_

#if !defined(__Fuchsia__)
#error event_pair_watcher.h can only be included on Fuchsia.
#endif

#include <zircon/types.h>

#include <src/developer/debug/shared/peered_object_watcher.h>

namespace debug {
// Currently the only use case we have for EventPairs is to monitor when they are closed, so there
// is no additional method for notifications of signals other than PeerClosed being sent.
class EventPairWatcher : public PeeredObjectWatcher {};
}  // namespace debug
#endif  // SRC_DEVELOPER_DEBUG_SHARED_EVENT_PAIR_WATCHER_H_
