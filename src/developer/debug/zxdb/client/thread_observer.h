// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_THREAD_OBSERVER_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_THREAD_OBSERVER_H_

#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/zxdb/client/stop_info.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace zxdb {

class Breakpoint;
class Thread;

class ThreadObserver {
 public:
  virtual void DidCreateThread(Thread* thread) {}
  virtual void WillDestroyThread(Thread* thread) {}

  // Notification that a thread has stopped. The thread and all breakpoint statistics will be
  // up-to-date.
  virtual void OnThreadStopped(Thread* thread, const StopInfo& info) {}

  // Notification that the frames on a thread have been updated in some way:
  //
  //  - They could be cleared (when the thread is resumed).
  //  - They could go from empty to being set (when the thread is stopped).
  //  - They could be added to (when the minimal stack is replaced with a full stack).
  //  - They could be changed arbitrarily (when symbols are loaded or when a stack refresh is
  //    requested and the thread has changed out from under us).
  virtual void DidUpdateStackFrames(Thread* thread) {}
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_THREAD_OBSERVER_H_
