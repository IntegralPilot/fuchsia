// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_PROCESS_INFO_ITERATOR_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_PROCESS_INFO_ITERATOR_H_

#include <fidl/fuchsia.debugger/cpp/fidl.h>

#include "src/lib/fxl/memory/weak_ptr.h"

namespace debug_agent {

class DebugAgent;
class DebuggedProcess;
class DebuggedThread;

class ProcessInfoIterator : public fidl::Server<fuchsia_debugger::ProcessInfoIterator> {
 public:
  ProcessInfoIterator(fxl::WeakPtr<DebugAgent> debug_agent, std::vector<DebuggedProcess*> processes,
                      std::optional<fuchsia_debugger::ThreadDetailsInterest> interest);

  // fidl::Server<ProcessInfoIterator> implementation.
  void GetNext(GetNextCompleter::Sync& completer) override;

  // Advance to the next thread in |current_process_|. If there are no more threads, then also
  // advance to the next process. When there are no more processes and all threads have been
  // visited, this returns false.
  bool Advance();

  // Returns true if |interest_| is set and the client has set the corresponding interest to true.
  bool CaptureBacktrace() const;

 private:
  friend class ProcessInfoIteratorTest;

  fxl::WeakPtr<DebugAgent> debug_agent_;
  std::optional<fuchsia_debugger::ThreadDetailsInterest> interest_;

  // All pointers are non-owning.
  std::vector<DebuggedProcess*> processes_;
  // Current position in |processes_|.
  size_t process_index_ = 0;

  // The pointers are non-owning, but we need to keep this vector alive, since DebuggedProcess vends
  // us the vector from an internal map.
  std::vector<DebuggedThread*> current_process_threads_;
  // Current position in |current_process_threads_|.
  size_t thread_index_ = 0;

  std::string current_moniker_;
  DebuggedProcess* current_process_ = nullptr;
  DebuggedThread* current_thread_ = nullptr;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_PROCESS_INFO_ITERATOR_H_
