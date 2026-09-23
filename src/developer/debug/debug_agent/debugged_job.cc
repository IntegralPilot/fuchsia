// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/debugged_job.h"

#include "lib/syslog/cpp/macros.h"
#include "src/developer/debug/debug_agent/debug_agent.h"
#include "src/developer/debug/debug_agent/debugged_process.h"

namespace debug_agent {

DebuggedJobCreateInfo::DebuggedJobCreateInfo(std::unique_ptr<JobHandle> handle)
    : handle(std::move(handle)) {}

DebuggedJob::DebuggedJob(DebugAgent* debug_agent) : debug_agent_(debug_agent) {}

debug::Status DebuggedJob::Init(DebuggedJobCreateInfo&& info) {
  if (info.handle == nullptr) {
    return debug::Status("Cannot initialize DebuggedJob with an invalid JobHandle.");
  }

  job_handle_ = std::move(info.handle);

  JobHandle::AttachConfig config;
  switch (info.priority) {
    case debug_ipc::AttachConfig::Priority::kMinimal:
      config.exception_channel_type = JobExceptionChannelType::kNone;
      break;
    case debug_ipc::AttachConfig::Priority::kWeak:
      config.exception_channel_type = JobExceptionChannelType::kDebugger;
      break;
    case debug_ipc::AttachConfig::Priority::kStrong:
      config.exception_channel_type = JobExceptionChannelType::kException;
      break;
  }

  if (auto status = job_handle_->Attach(this, config); status.has_error()) {
    return status;
  }

  type_ = config.exception_channel_type;
  return debug::Status();
}

void DebuggedJob::OnProcessStarting(std::unique_ptr<ProcessHandle> process,
                                    std::unique_ptr<ExceptionHandle> exception) {
  FX_DCHECK(debug_agent_);

  debug_agent_->OnProcessChanged(DebugAgent::ProcessChangedHow::kStarting, std::move(process));
}

void DebuggedJob::OnProcessNameChanged(std::unique_ptr<ProcessHandle> process_handle,
                                       std::unique_ptr<ExceptionHandle> exception) {
  FX_DCHECK(debug_agent_);

  debug_agent_->OnProcessChanged(DebugAgent::ProcessChangedHow::kNameChanged,
                                 std::move(process_handle));
}

void DebuggedJob::OnUnhandledException(std::unique_ptr<ExceptionHandle> exception_handle) {
  FX_DCHECK(debug_agent_);
  // This notification means that we were attached to a job and none of the processes within this
  // job handled the exception. By virtue of being attached to the job only, we are not actually
  // interactively debugging the exception, we just collect some information and report it to all
  // clients.
  auto process = debug_agent_->GetDebuggedProcess(exception_handle->GetPid());

  // We are not guaranteed to be paying attention to ProcessStarting events from this job, therefore
  // we might not have created a DebuggedProcess object. If we don't, make one now. It's also
  // entirely possible for something else in the system to have issued a zx_task_kill on this
  // process, so there is no guarantee that we can get a handle to the process object.
  if (!process) {
    auto process_handle = job_handle_->FindProcess(exception_handle->GetPid());
    if (process_handle) {
      debug::Status status = debug_agent_->TrackProcess(std::move(process_handle), &process);
      if (status.has_error() || !process) {
        LOGS(Warn) << "Failed to attach on-demand to process pid " << exception_handle->GetPid()
                   << ": " << status.message();
        return;
      }
    } else {
      LOGS(Warn) << "Got exception for unknown process pid: " << exception_handle->GetPid();
      return;
    }
  }

  // Make sure the DebuggedProcess knows about all of the threads. This can be the case if we have
  // already created a DebuggedProcess but not claimed its exception channel at all. This method is
  // idempotent, it does not remove any threads, any currently pending thread starting events will
  // be instantly acknowledged. See DebuggedProcess::OnThreadStarting.
  process->PopulateCurrentThreads();

  auto thread = process->GetThread(exception_handle->GetTid());

  // Similarly to the above with processes, the thread might already have been destroyed by the
  // kernel before we can do anything with it. We cannot assert the validity of the thread.
  if (!thread) {
    debug_ipc::ThreadRecord::State thread_state = debug_ipc::ThreadRecord::State::kLast;
    auto thread_handle = exception_handle->GetThreadHandle();
    if (thread_handle) {
      thread_state = thread_handle->GetState().state;
    }
    LOGS(Warn) << "Got exception for unknown thread tid: " << exception_handle->GetTid()
               << "(state = " << debug_ipc::ThreadRecord::StateToString(thread_state) << ")";
    return;
  }

  debug_ipc::NotifyException notify;
  notify.thread = thread->GetThreadRecord(debug_ipc::ThreadRecord::StackAmount::kFull);
  notify.type = exception_handle->GetType(thread->thread_handle());
  notify.exception = exception_handle->GetRecord();
  notify.timestamp = GetNowTimestamp();
  // Make sure to tell the client that we aren't actually holding onto the exception after the
  // notification has been sent.
  notify.job_only = true;

  debug_agent_->SendNotification(notify);

  // Stop watching this particular process, but keep the job. The job is what the user is attached
  // to and could have other processes running.
  debug_agent_->RemoveDebuggedProcess(process->koid());
  exception_handle.reset();
}

}  // namespace debug_agent
