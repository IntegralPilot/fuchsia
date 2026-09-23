// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_MINIDUMP_ITERATOR_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_MINIDUMP_ITERATOR_H_

#include <fidl/fuchsia.debugger/cpp/fidl.h>

#include "src/lib/fxl/memory/weak_ptr.h"

namespace debug_agent {

class DebugAgent;
class DebuggedProcess;

class MinidumpIterator : public fidl::Server<fuchsia_debugger::MinidumpIterator> {
 public:
  MinidumpIterator(fxl::WeakPtr<DebugAgent> debug_agent, std::vector<DebuggedProcess*> processes);

  // fidl::Server<MinidumpIterator> implementation.
  void GetNext(GetNextCompleter::Sync& completer) override;

  // Advance to the next process in |processes_|, and update |current_process_|. Once all processes
  // have been iterated, this returns false.
  bool Advance();

 private:
  friend class MinidumpIteratorTest;

  std::vector<DebuggedProcess*> processes_;
  size_t index_ = 0;
  DebuggedProcess* current_process_ = nullptr;
  fxl::WeakPtr<DebugAgent> debug_agent_;
};
}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_MINIDUMP_ITERATOR_H_
