// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_DEBUG_AGENT_OBSERVER_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_DEBUG_AGENT_OBSERVER_H_

#include "src/developer/debug/ipc/protocol.h"

class DebugAgentObserver {
 public:
// We have to overload this name for all notification types, since this will be called from a
// templated function (see DebugAgent::SendNotification). But templated virtual functions are not
// allowed. We don't need to worry about the debug_ipc versioning because this class and its
// descendants will all be built together with DebugAgent.
#define FN(notify_type) \
  virtual void OnNotification(const debug_ipc::notify_type& notify) {}
  FOR_EACH_NOTIFICATION_TYPE(FN)
#undef FN
};

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_DEBUG_AGENT_OBSERVER_H_
