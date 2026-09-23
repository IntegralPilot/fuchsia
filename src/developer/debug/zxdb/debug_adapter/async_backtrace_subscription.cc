// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/async_backtrace_subscription.h"

#include <lib/syslog/cpp/macros.h>

#include <algorithm>

#include "src/developer/debug/zxdb/client/async_task.h"
#include "src/developer/debug/zxdb/client/async_task_tree.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/source_file_provider_impl.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/common/string_util.h"
#include "src/developer/debug/zxdb/symbols/location.h"

namespace dap {

DAP_IMPLEMENT_STRUCT_TYPEINFO(AsyncTaskNode, "", DAP_FIELD(id, "id"), DAP_FIELD(name, "name"),
                              DAP_FIELD(file, "file"), DAP_FIELD(line, "line"),
                              DAP_FIELD(children, "children"))

DAP_IMPLEMENT_STRUCT_TYPEINFO(AsyncBacktraceUpdate, "zxdb.updateAsyncBacktrace",
                              DAP_FIELD(id, "id"), DAP_FIELD(name, "name"),
                              DAP_FIELD(processId, "processId"), DAP_FIELD(tasks, "tasks"))

}  // namespace dap

namespace zxdb {

namespace {

dap::integer GetProcessKoid(const Thread* thread) {
  FX_DCHECK(thread);
  FX_DCHECK(thread->GetProcess());
  return static_cast<dap::integer>(thread->GetProcess()->GetKoid());
}

bool AllThreadsStopped(const Process* process, const Thread* ignore_thread = nullptr) {
  if (!process) {
    return false;
  }
  const auto& threads = process->GetThreads();
  if (threads.empty()) {
    return false;
  }
  return !std::ranges::any_of(threads, [ignore_thread](const Thread* t) {
    return t != ignore_thread && !t->CurrentStopSupportsFrames();
  });
}

}  // namespace

AsyncBacktraceSubscription::AsyncBacktraceSubscription(fxl::WeakPtr<Session> session,
                                                       std::shared_ptr<dap::Session> dap)
    : session_(std::move(session)), dap_(std::move(dap)), weak_factory_(this) {
  if (session_) {
    session_->thread_observers().AddObserver(this);
  }
}

AsyncBacktraceSubscription::~AsyncBacktraceSubscription() {
  if (session_) {
    session_->thread_observers().RemoveObserver(this);
  }
}

void AsyncBacktraceSubscription::DidCreateThread(Thread* thread) {
  CancelPendingBacktrace(thread);

  // Notify DAP client that a new thread has been created.
  dap_->send(dap::AsyncBacktraceUpdate{
      .id = static_cast<int64_t>(thread->GetKoid()),
      .name = thread->GetName(),
      .processId = GetProcessKoid(thread),
      .tasks = dap::array<dap::AsyncTaskNode>(),
  });
}

void AsyncBacktraceSubscription::OnThreadStopped(Thread* thread, const StopInfo& /*info*/) {
  CancelPendingBacktrace(thread);

  // Syncing the async task tree is expensive, so skip syncing when no frames are available.
  if (!thread->CurrentStopSupportsFrames()) {
    dap_->send(dap::AsyncBacktraceUpdate{
        .id = static_cast<int64_t>(thread->GetKoid()),
        .name = thread->GetName(),
        .processId = GetProcessKoid(thread),
        .tasks = dap::array<dap::AsyncTaskNode>(),
    });
    return;
  }

  if (!AllThreadsStopped(thread->GetProcess())) {
    return;
  }

  for (Thread* t : thread->GetProcess()->GetThreads()) {
    CollectAndReportAsyncBacktrace(t);
  }
}

void AsyncBacktraceSubscription::OnProcessStopped(Process* process) {
  if (!AllThreadsStopped(process)) {
    return;
  }

  for (Thread* t : process->GetThreads()) {
    CollectAndReportAsyncBacktrace(t);
  }
}

void AsyncBacktraceSubscription::DidUpdateStackFrames(Thread* thread) {
  // Notify DAP client to reset this thread's async backtrace subtree, since it has resumed.
  if (!thread->CurrentStopSupportsFrames()) {
    if (Process* process = thread->GetProcess()) {
      for (Thread* t : process->GetThreads()) {
        CancelPendingBacktrace(t);
      }
    } else {
      CancelPendingBacktrace(thread);
    }

    dap_->send(dap::AsyncBacktraceUpdate{
        .id = static_cast<int64_t>(thread->GetKoid()),
        .name = thread->GetName(),
        .processId = GetProcessKoid(thread),
        .tasks = dap::array<dap::AsyncTaskNode>(),
    });
    return;
  }
}

void AsyncBacktraceSubscription::WillDestroyThread(Thread* thread) {
  Process* process = thread->GetProcess();
  CancelPendingBacktrace(thread);

  // Notify DAP client to remove this thread from the async backtrace view.
  dap_->send(dap::AsyncBacktraceUpdate{
      .id = static_cast<int64_t>(thread->GetKoid()),
      .name = thread->GetName(),
      .processId = GetProcessKoid(thread),
  });

  // If thread destruction leaves all remaining threads in the process stopped,
  // trigger async backtrace collection for the remaining threads.
  if (AllThreadsStopped(process, thread)) {
    for (Thread* t : process->GetThreads()) {
      if (t != thread) {
        CollectAndReportAsyncBacktrace(t);
      }
    }
  }
}

void AsyncBacktraceSubscription::CancelPendingBacktrace(const Thread* thread) {
  if (auto it = pending_backtraces_.find(thread->GetKoid()); it != pending_backtraces_.end()) {
    it->second.Cancel();
    pending_backtraces_.erase(it);
  }
}

void AsyncBacktraceSubscription::CollectAndReportAsyncBacktrace(Thread* thread) {
  // Enqueue into `pending_backtraces_` before collecting the async backtrace into an
  // `AsyncBacktraceUpdate` DAP event, allowing this backtrace to be cancelled if the thread state
  // happens to change in quick succession (e.g. thread resumes, or thread is destroyed).
  CancelPendingBacktrace(thread);
  auto [it, inserted] = pending_backtraces_.try_emplace(
      thread->GetKoid(),
      [weak_this = weak_factory_.GetWeakPtr(), weak_thread = thread->GetWeakPtr()](
          const Err& err, const Frame* /*frame*/) {
        if (!weak_this || !weak_thread) {
          return;
        }

        if (!AllThreadsStopped(weak_thread->GetProcess())) {
          return;
        }

        uint64_t thread_koid = weak_thread->GetKoid();

        // A common source of errors is when async tasks aren't found (e.g. thread is not running
        // async code). In these cases, we report an empty task tree to the DAP client for this
        // thread.
        if (err.has_error()) {
          FX_LOGS(DEBUG) << "Failed to collect async task tree for thread \"" << thread_koid
                         << "\": " << err.msg();
          weak_this->dap_->send(dap::AsyncBacktraceUpdate{
              .id = static_cast<int64_t>(thread_koid),
              .name = weak_thread->GetName(),
              .processId = GetProcessKoid(weak_thread.get()),
              .tasks = dap::array<dap::AsyncTaskNode>(),
          });
          return;
        }

        if (!weak_thread->GetProcess()->GetTarget()) {
          return;
        }

        auto file_provider =
            SourceFileProviderImpl(weak_thread->GetProcess()->GetTarget()->settings());
        weak_this->dap_->send(dap::AsyncBacktraceUpdate{
            .id = static_cast<int64_t>(thread_koid),
            .name = weak_thread->GetName(),
            .processId = GetProcessKoid(weak_thread.get()),
            .tasks = weak_thread->GetAsyncTaskTree().Map<dap::AsyncTaskNode>(
                [&file_provider](const zxdb::AsyncTask& task, dap::AsyncTaskNode* node) {
                  if (uint64_t task_id = task.GetId(); task_id != 0) {
                    node->id = zxdb::to_hex_string(task_id);
                  }
                  node->name = task.GetIdentifier().GetFullName();
                  const zxdb::Location& location = task.GetLocation();
                  if (location.file_line().is_valid()) {
                    auto file_metadata = file_provider.GetFileMetadata(
                        location.file_line().file(), location.file_line().comp_dir());
                    if (!file_metadata.has_error()) {
                      node->file = file_metadata.take_value().full_path;
                      node->line = location.file_line().line();
                    }
                  }
                },
                [](dap::AsyncTaskNode* node) { return &node->children; }),
        });
      });
  thread->GetAsyncTaskTree().Sync(thread->GetStack(), it->second.callback());
}

}  // namespace zxdb
