// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "context.h"

#include <algorithm>
#include <cstdint>
#include <string_view>
#include <vector>

#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/zxdb/client/breakpoint.h"
#include "src/developer/debug/zxdb/client/filter.h"
#include "src/developer/debug/zxdb/client/pretty_stack_manager.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/system.h"
#include "src/developer/debug/zxdb/client/system_observer.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_attach.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_breakpoint.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_continue.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_evaluate.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_function_breakpoint.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_launch.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_next.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_pause.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_process.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_scopes.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_stacktrace.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_step_in.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_step_out.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_terminate.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_threads.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_variables.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_zxdb_detach.h"
#include "src/developer/debug/zxdb/debug_adapter/server.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace zxdb {
namespace {

constexpr std::string_view kContentLengthHeader = "Content-Length:";

// Escapes newlines and carriage returns for clean single-line debug logging.
std::string EscapeNewlines(std::string_view sv) {
  size_t extra = 0;
  for (char c : sv) {
    if (c == '\n' || c == '\r') {
      extra++;
    }
  }
  std::string s;
  s.reserve(sv.size() + extra);
  for (char c : sv) {
    if (c == '\n') {
      s += "\\n";
    } else if (c == '\r') {
      s += "\\r";
    } else {
      s += c;
    }
  }
  return s;
}

}  // namespace

DebugAdapterContext::DebugAdapterContext(Console* console, debug::StreamBuffer* stream)
    : console_(console), dap_(dap::Session::create()), stream_(stream) {
  reader_ = std::make_shared<DebugAdapterReader>(stream);
  writer_ = std::make_shared<DebugAdapterWriter>(stream);

  session()->AddObserver(this);

  dap_->registerHandler(
      [this](const dap::InitializeRequest& req,
             std::function<void(dap::ResponseOrError<dap::InitializeResponse>)> send_resp) {
        DEBUG_LOG(DebugAdapter) << "InitializeRequest received";
        if (req.supportsInvalidatedEvent) {
          supports_invalidate_event_ = req.supportsInvalidatedEvent.value();
        }
        if (req.supportsRunInTerminalRequest) {
          supports_run_in_terminal_ = req.supportsRunInTerminalRequest.value();
        }
        send_initialize_response_ = send_resp;
        // If the session is connected or there's no pending connection, send the response
        // immediately. Otherwise, defer the response until the connection resolves.
        if (session()->IsConnected()) {
          DidResolveConnection(Err());
        } else if (!session()->HasPendingConnection()) {
          Err err = session()->last_connection_error();
          if (!err.has_error()) {
            err = Err("Debugger not connected to device");
          }
          DidResolveConnection(err);
        }
      });

  dap_->registerSentHandler([this](const dap::ResponseOrError<dap::InitializeResponse>& response) {
    DEBUG_LOG(DebugAdapter) << "InitializeResponse sent";
    // Set up events and handlers now. All messages should be sent only after Initialize response
    // is sent. Setting up earlier would lead to events and responses being sent before Initialize
    // request is processed.
    Init();
    dap_->send(dap::InitializedEvent());
  });

  dap_->onError([](const char* msg) { LOGS(Error) << "dap::Session error:" << msg; });

  dap_->connect(reader_, writer_);
}

DebugAdapterContext::~DebugAdapterContext() {
  if (init_done_) {
    session()->thread_observers().RemoveObserver(this);
    session()->process_observers().RemoveObserver(this);
    session()->breakpoint_observers().RemoveObserver(this);
    session()->system().RemoveObserver(this);
  }
  DeleteAllBreakpoints();
  DeleteAllFilters();
  session()->RemoveObserver(this);
}

void DebugAdapterContext::DidResolveConnection(const Err& err) {
  if (!send_initialize_response_) {
    return;
  }
  if (err.has_error()) {
    send_initialize_response_(dap::Error(err.msg()));
    return;
  }
  dap::InitializeResponse response;
  response.supportsFunctionBreakpoints = true;
  response.supportsConditionalBreakpoints = true;
  response.supportsConfigurationDoneRequest = true;
  response.supportsEvaluateForHovers = false;
  response.supportsTerminateRequest = true;
  send_initialize_response_(response);
}

void DebugAdapterContext::Init() {
  session()->analytics().ReportConsoleType(ConsoleType::Type::kDebugAdapter);

  // Register handlers with dap module.
  dap_->registerHandler([this](const dap::LaunchRequestZxdb& req) {
    DEBUG_LOG(DebugAdapter) << "RunBinaryRequest received";
    return OnRequestLaunch(this, req);
  });

  dap_->registerHandler([](const dap::SetExceptionBreakpointsRequest& req) {
    DEBUG_LOG(DebugAdapter) << "SetExceptionBreakpointsRequest received";
    dap::SetExceptionBreakpointsResponse response;
    return response;
  });

  dap_->registerHandler([this](const dap::SetBreakpointsRequest& req) {
    DEBUG_LOG(DebugAdapter) << "SetBreakpointsRequest received";
    return OnRequestBreakpoint(this, req);
  });

  dap_->registerHandler([this](const dap::SetFunctionBreakpointsRequest& req) {
    DEBUG_LOG(DebugAdapter) << "SetFunctionBreakpointsRequest received";
    return OnRequestFunctionBreakpoint(this, req);
  });

  dap_->registerHandler([](const dap::ConfigurationDoneRequest& req) {
    DEBUG_LOG(DebugAdapter) << "ConfigurationDoneRequest received";
    return dap::ConfigurationDoneResponse();
  });

  dap_->registerHandler([this](const dap::AttachRequestZxdb& req) {
    DEBUG_LOG(DebugAdapter) << "AttachRequest received";
    return OnRequestAttach(this, req);
  });

  // cppdap statically associates dap::ThreadsRequest with the standard
  // dap::ThreadsResponse when using the high-level registerHandler API.
  // We register using the low-level untyped callback so we can provide
  // the extended dap::ThreadsResponseZxdb type information to onSuccess
  // and onError, returning processId with each thread.
  dap_->registerHandler(
      dap::TypeOf<dap::ThreadsRequest>::type(),
      [this](const void* args, const dap::Session::RequestHandlerSuccessCallback& onSuccess,
             const dap::Session::RequestHandlerErrorCallback& onError) {
        DEBUG_LOG(DebugAdapter) << "ThreadsRequest received";
        auto res = OnRequestThreads(this, *static_cast<const dap::ThreadsRequest*>(args));
        if (res.error) {
          onError(dap::TypeOf<dap::ThreadsResponseZxdb>::type(), res.error);
        } else {
          onSuccess(dap::TypeOf<dap::ThreadsResponseZxdb>::type(), &res.response);
        }
      });

  dap_->registerHandler(
      [this](const dap::PauseRequestZxdb& req,
             std::function<void(dap::ResponseOrError<dap::PauseResponseZxdb>)> callback) {
        DEBUG_LOG(DebugAdapter) << "PauseRequest received";
        OnRequestPause(this, req, callback);
      });

  dap_->registerHandler([this](const dap::ContinueRequest& req) {
    DEBUG_LOG(DebugAdapter) << "ContinueRequest received";
    return OnRequestContinue(this, req);
  });

  dap_->registerHandler(
      [this](const dap::NextRequest& req,
             std::function<void(dap::ResponseOrError<dap::NextResponse>)> callback) {
        DEBUG_LOG(DebugAdapter) << "NextRequest received";
        OnRequestNext(this, req, callback);
      });

  dap_->registerHandler(
      [this](const dap::StepInRequest& req,
             std::function<void(dap::ResponseOrError<dap::StepInResponse>)> callback) {
        DEBUG_LOG(DebugAdapter) << "StepInRequest received";
        OnRequestStepIn(this, req, callback);
      });

  dap_->registerHandler(
      [this](const dap::StepOutRequest& req,
             std::function<void(dap::ResponseOrError<dap::StepOutResponse>)> callback) {
        DEBUG_LOG(DebugAdapter) << "StepOutRequest received";
        OnRequestStepOut(this, req, callback);
      });

  dap_->registerHandler(
      [this](const dap::StackTraceRequestZxdb& req,
             std::function<void(dap::ResponseOrError<dap::StackTraceResponse>)> callback) {
        DEBUG_LOG(DebugAdapter) << "StackTraceRequest received";
        OnRequestStackTrace(this, req, callback);
      });

  dap_->registerHandler([this](const dap::ScopesRequest& req) {
    DEBUG_LOG(DebugAdapter) << "ScopesRequest received";
    return OnRequestScopes(this, req);
  });

  dap_->registerHandler(
      [this](const dap::VariablesRequest& req,
             std::function<void(dap::ResponseOrError<dap::VariablesResponse>)> callback) {
        DEBUG_LOG(DebugAdapter) << "VariablesRequest received";
        OnRequestVariables(this, req, callback);
      });

  dap_->registerHandler(
      [this](const dap::EvaluateRequest& req,
             std::function<void(dap::ResponseOrError<dap::EvaluateResponse>)> callback) {
        DEBUG_LOG(DebugAdapter) << "EvaluateRequest received";
        OnRequestEvaluate(this, req, callback);
      });

  dap_->registerHandler(
      [this](const dap::ZxdbTerminateRequest& req,
             std::function<void(dap::ResponseOrError<dap::TerminateResponse>)> callback) {
        DEBUG_LOG(DebugAdapter) << "ZxdbTerminateRequest received";
        OnRequestZxdbTerminate(this, req, callback);
      });

  dap_->registerHandler(
      [this](const dap::ZxdbDetachRequest& req,
             std::function<void(dap::ResponseOrError<dap::ZxdbDetachResponse>)> callback) {
        DEBUG_LOG(DebugAdapter) << "ZxdbDetachRequest received";
        OnRequestZxdbDetach(this, req, callback);
      });

  dap_->registerHandler([this](const dap::ZxdbProcessRequest& req) {
    DEBUG_LOG(DebugAdapter) << "ZxdbProcessRequest received";
    return OnRequestZxdbProcess(this, req);
  });

  dap_->registerHandler([this](const dap::DisconnectRequest& req) {
    DEBUG_LOG(DebugAdapter) << "DisconnectRequest received";
    if (destroy_connection_cb_) {
      debug::MessageLoop::Current()->PostTask(
          FROM_HERE, [cb = std::move(destroy_connection_cb_)]() mutable { cb(); });
    }
    return dap::DisconnectResponse();
  });

  // Register to zxdb session events
  session()->thread_observers().AddObserver(this);
  session()->process_observers().AddObserver(this);
  session()->breakpoint_observers().AddObserver(this);
  session()->system().AddObserver(this);

  async_backtrace_subscription_.emplace(session()->GetWeakPtr(), dap_);

  init_done_ = true;
}

bool DebugAdapterContext::HasCompleteMessage() {
  if (!stream_) {
    return false;
  }

  char peek_buf[1024];
  size_t peeked = stream_->Peek(peek_buf, sizeof(peek_buf));
  if (peeked == 0) {
    return false;
  }

  std::string_view view(peek_buf, peeked);

  size_t cl_pos = view.find("Content-Length:");
  if (cl_pos == std::string_view::npos) {
    DEBUG_LOG(DebugAdapter)
        << "Completeness check: 'Content-Length:' not found in peeked data (size=" << peeked
        << "): " << EscapeNewlines(view);
    return false;
  }

  size_t header_end = view.find("\r\n\r\n", cl_pos);
  if (header_end == std::string_view::npos) {
    DEBUG_LOG(DebugAdapter)
        << "Completeness check: '\\r\\n\\r\\n' not found after 'Content-Length:' (size=" << peeked
        << "): " << EscapeNewlines(view);
    return false;
  }

  size_t val_pos = cl_pos + kContentLengthHeader.size();
  while (val_pos < header_end && (view[val_pos] == ' ' || view[val_pos] == '\t')) {
    val_pos++;
  }

  size_t len = 0;
  while (val_pos < header_end && view[val_pos] >= '0' && view[val_pos] <= '9') {
    len = len * 10 + (view[val_pos] - '0');
    val_pos++;
  }

  if (len == 0) {
    DEBUG_LOG(DebugAdapter) << "Completeness check: Invalid or 0 Content-Length in: "
                            << EscapeNewlines(view);
    return false;
  }

  size_t header_len = header_end + 4;
  size_t total_len = header_len + len;

  bool complete = stream_->IsAvailable(total_len);

  DEBUG_LOG(DebugAdapter) << "Completeness check: required=" << total_len << ", peeked=" << peeked
                          << ", complete=" << complete;

  if (!complete) {
    DEBUG_LOG(DebugAdapter) << "Incomplete message data: " << EscapeNewlines(view);
  }

  return complete;
}

void DebugAdapterContext::OnStreamReadable() {
  while (HasCompleteMessage()) {
    if (auto payload = dap_->getPayload()) {
      payload();
    } else {
      break;
    }
  }
}

void DebugAdapterContext::DidCreateThread(Thread* thread) {
  dap::ThreadEventZxdb event;
  event.reason = "started";
  event.threadId = thread->GetKoid();
  if (thread->GetProcess()) {
    event.processId = thread->GetProcess()->GetKoid();
  }
  dap_->send(event);
}

void DebugAdapterContext::WillDestroyThread(Thread* thread) {
  dap::ThreadEventZxdb event;
  event.reason = "exited";
  event.threadId = thread->GetKoid();
  if (thread->GetProcess()) {
    event.processId = thread->GetProcess()->GetKoid();
  }
  dap_->send(event);
}

void DebugAdapterContext::OnThreadStopped(Thread* thread, const StopInfo& info) {
  dap::StoppedEvent event;
  switch (info.exception_type) {
    case debug_ipc::ExceptionType::kSoftwareBreakpoint:
    case debug_ipc::ExceptionType::kHardwareBreakpoint:
      event.reason = "breakpoint";
      event.description = "Breakpoint hit";
      break;
    case debug_ipc::ExceptionType::kSingleStep:
      event.reason = "step";
      break;
    case debug_ipc::ExceptionType::kPolicyError:
      event.reason = "exception";
      event.description = "Policy error";
      break;
    case debug_ipc::ExceptionType::kPageFault:
      event.reason = "exception";
      event.description = "Page fault";
      break;
    case debug_ipc::ExceptionType::kUndefinedInstruction:
      event.reason = "exception";
      event.description = "Undefined Instruction";
      break;
    case debug_ipc::ExceptionType::kUnalignedAccess:
      event.reason = "exception";
      event.description = "Unaligned Access";
      break;
    default:
      event.reason = "unknown";
  }
  event.threadId = thread->GetKoid();

  // Check whether a process level breakpoint caused this event.
  // TODO(https://fxbug.dev/442573279): This approach doesn't not work for
  // `debug_ipc::ExceptionType::kSingleStep` since `StopInfo::hit_breakpoints` will be empty in that
  // case.
  for (auto& bp : info.hit_breakpoints) {
    if (bp) {
      if (!event.hitBreakpointIds.has_value()) {
        event.hitBreakpointIds = dap::array<dap::integer>();
      }
      event.hitBreakpointIds->push_back(IdForBreakpoint(bp.get()));
      if (bp->GetSettings().enabled &&
          (bp->GetSettings().stop_mode == BreakpointSettings::StopMode::kProcess ||
           bp->GetSettings().stop_mode == BreakpointSettings::StopMode::kAll)) {
        event.allThreadsStopped = true;
      }
    }
  }
  dap_->send(event);
}

void DebugAdapterContext::OnProcessPaused(Process* process) {
  if (async_backtrace_subscription_) {
    async_backtrace_subscription_->OnProcessStopped(process);
  }
}

void DebugAdapterContext::OnThreadPaused(Thread* thread) {
  if (async_backtrace_subscription_) {
    async_backtrace_subscription_->OnThreadStopped(thread, StopInfo());
  }
}

void DebugAdapterContext::DidUpdateStackFrames(Thread* thread) { DeleteFrameIdsForThread(thread); }

void DebugAdapterContext::DidCreateProcess(Process* process, uint64_t timestamp) {
  dap::ProcessEvent event;
  event.name = process->GetName();
  event.isLocalProcess = false;
  event.systemProcessId = process->GetKoid();

  switch (process->start_type()) {
    case Process::StartType::kAttach:
      event.startMethod = "attach";
      break;
    case Process::StartType::kLaunch:
      event.startMethod = "launch";
      break;
  }

  dap_->send(event);
}

void DebugAdapterContext::WillDestroyProcess(Process* process, DestroyReason reason, int exit_code,
                                             uint64_t timestamp) {
  dap::ExitedEvent exit_event;            // Sent when process exits.
  dap::TerminatedEvent terminated_event;  // Sent when process is detached.
  switch (reason) {
    case ProcessObserver::DestroyReason::kExit:
      exit_event.exitCode = exit_code;
      dap_->send(exit_event);
      break;
    case ProcessObserver::DestroyReason::kDetach:
      dap_->send(terminated_event);
      break;
    case ProcessObserver::DestroyReason::kKill:
      exit_event.exitCode = -1;
      dap_->send(exit_event);
      break;
  }
}

int64_t DebugAdapterContext::IdForBreakpoint(Breakpoint* breakpoint) {
  auto item = breakpoint_to_id_.find(breakpoint);
  if (item != breakpoint_to_id_.end()) {
    return item->second;
  }

  int64_t current_breakpoint_id = next_breakpoint_id_++;
  breakpoint_to_id_[breakpoint] = current_breakpoint_id;
  return current_breakpoint_id;
}

void DebugAdapterContext::OnBreakpointMatched(Breakpoint* breakpoint, bool user_requested) {
  BreakpointSettings settings = breakpoint->GetSettings();

  dap::Breakpoint bp;
  bp.verified = true;
  bp.id = IdForBreakpoint(breakpoint);

  dap::BreakpointEvent breakpoint_event;
  breakpoint_event.reason = "changed";
  breakpoint_event.breakpoint = bp;
  dap_->send(breakpoint_event);
}

Thread* DebugAdapterContext::GetThread(uint64_t koid) {
  Thread* match = nullptr;
  auto targets = session()->system().GetTargets();
  for (auto target : targets) {
    if (!target) {
      continue;
    }
    auto process = target->GetProcess();
    if (!process) {
      continue;
    }
    auto threads = process->GetThreads();
    for (auto thread : threads) {
      if (koid == thread->GetKoid()) {
        match = thread;
        break;
      }
    }
  }
  return match;
}

Err DebugAdapterContext::CheckStoppedThread(Thread* thread) {
  if (!thread) {
    return Err("Invalid thread.");
  }

  std::optional<debug_ipc::ThreadRecord::State> state_or = thread->GetState();
  if (!state_or) {
    return Err("Thread should be suspended but thread %llu is in an unknown state.",
               static_cast<unsigned long long>(thread->GetKoid()));
  }

  if (*state_or != debug_ipc::ThreadRecord::State::kBlocked &&
      *state_or != debug_ipc::ThreadRecord::State::kCoreDump &&
      *state_or != debug_ipc::ThreadRecord::State::kSuspended) {
    return Err("Thread should be suspended but thread %llu is %s.",
               static_cast<unsigned long long>(thread->GetKoid()),
               debug_ipc::ThreadRecord::StateToString(*state_or));
  }
  return Err();
}

std::vector<PrettyStackManager::Match> DebugAdapterContext::GetElidedFrames(const Stack& stack) {
  std::vector<PrettyStackManager::Match> result(stack.size());

  Process* process = nullptr;
  if (stack[0]->GetThread()) {
    process = stack[0]->GetThread()->GetProcess();
  }

  // Elide against PrettyStackManager's default matchers first.
  for (const auto& frame : console()->context().pretty_stack_manager()->ProcessStack(stack)) {
    for (uint64_t i = 0; i < frame.frames.size(); i++) {
      result.at(frame.begin_index + i) = frame.match;
    }
  }

  // Next, elide against TestFailureStackMatcher's matchers. This runs second so the more relevant
  // "Test assertion implementation" grouping can override any generic matches near the top of the
  // stack during a test failure.
  auto test_impl_skipped_frames = console()->context().test_failure_stack_matcher()->Match(stack);
  for (uint64_t i = 0; i < test_impl_skipped_frames; i++) {
    result.at(i) =
        PrettyStackManager::Match(test_impl_skipped_frames, "Test assertion implementation");
  }

  if (test_impl_skipped_frames > 0) {
    // We think this process is a test, mark it as such.
    process->set_kind(Process::Kind::kTest);
  }

  return result;
}

int64_t DebugAdapterContext::IdForFrame(uint64_t thread_koid, int64_t stack_index) {
  FrameRecord record = {};
  record.thread_koid = thread_koid;
  record.stack_index = stack_index;

  for (auto const& it : id_to_frame_) {
    if (it.second.thread_koid == record.thread_koid && it.second.stack_index == stack_index) {
      return it.first;
    }
  }

  int64_t current_frame_id = next_frame_id_++;
  id_to_frame_[current_frame_id] = record;
  return current_frame_id;
}

Frame* DebugAdapterContext::FrameforId(int64_t id) {
  // id - 0 is invalid
  if (!id) {
    return nullptr;
  }

  if (auto it = id_to_frame_.find(id); it != id_to_frame_.end()) {
    Thread* thread = GetThread(it->second.thread_koid);
    if (!thread) {
      return nullptr;
    }
    if (thread->GetStack().size() <= static_cast<size_t>(it->second.stack_index)) {
      return nullptr;
    }
    return thread->GetStack()[it->second.stack_index];
  }
  // Not found
  return nullptr;
}

void DebugAdapterContext::DeleteFrameIdsForThread(Thread* thread) {
  auto thread_koid = thread->GetKoid();
  for (auto it = id_to_frame_.begin(); it != id_to_frame_.end();) {
    // We don't really know what changed. We don't want to invalidate the frame ID every time
    // since one of the update cases is that the frames have been appended to (so existing indices
    // are still valid) or that symbols are loaded (normally this means that the frames are
    // unchanged, though inline frames can get expanded in some cases).
    //
    // As a result, keep the ID unchanged unless it's now out-of-bounds. This avoids resetting any
    // state in the more common cases.
    if ((it->second.thread_koid == thread_koid) &&
        (static_cast<size_t>(it->second.stack_index) >= thread->GetStack().size())) {
      if (supports_invalidate_event_) {
        dap::InvalidatedEvent event;
        event.stackFrameId = IdForFrame(thread_koid, it->second.stack_index);
        dap_->send(event);
      }
      DeleteVariablesIdsForFrameId(it->first);
      it = id_to_frame_.erase(it);
    } else {
      it++;
    }
  }
}

int64_t DebugAdapterContext::IdForVariables(int64_t frame_id, VariablesType type,
                                            std::unique_ptr<FormatNode> parent,
                                            fxl::WeakPtr<FormatNode> child) {
  // Check if an entry exists already, except for kChildVariable records, as those are always
  // created newly.
  if (type != VariablesType::kChildVariable) {
    for (auto const& it : id_to_variables_) {
      if (it.second.frame_id == frame_id && it.second.type == type) {
        return it.first;
      }
    }
  }

  VariablesRecord record;
  record.frame_id = frame_id;
  record.type = type;
  record.parent = std::move(parent);
  record.child = std::move(child);

  int current_variables_id = next_variables_id_++;
  id_to_variables_[current_variables_id] = std::move(record);
  return current_variables_id;
}

VariablesRecord* DebugAdapterContext::VariablesRecordForID(int64_t id) {
  // id - 0 is invalid
  if (!id) {
    return nullptr;
  }

  if (auto it = id_to_variables_.find(id); it != id_to_variables_.end()) {
    return &it->second;
  }
  // Not found
  return nullptr;
}

void DebugAdapterContext::DeleteVariablesIdsForFrameId(int64_t id) {
  for (auto it = id_to_variables_.begin(); it != id_to_variables_.end();) {
    if (it->second.frame_id == id) {
      it = id_to_variables_.erase(it);
    } else {
      it++;
    }
  }
}

void DebugAdapterContext::StoreBreakpointForSource(const std::filesystem::path& source,
                                                   Breakpoint* bp) {
  FX_DCHECK(bp);
  source_to_bp_[source].push_back(bp->GetWeakPtr());
}

std::vector<fxl::WeakPtr<Breakpoint>>* DebugAdapterContext::GetBreakpointsForSource(
    const std::filesystem::path& source) {
  if (auto it = source_to_bp_.find(source); it != source_to_bp_.end()) {
    return &it->second;
  }
  // Not found
  return nullptr;
}

void DebugAdapterContext::DeleteBreakpointsForSource(const std::filesystem::path& source) {
  auto it = source_to_bp_.find(source);
  if (it == source_to_bp_.end()) {
    return;
  }

  for (auto& bp : it->second) {
    if (bp) {
      breakpoint_to_id_.erase(bp.get());
      session()->system().DeleteBreakpoint(bp.get());
    }
  }
  source_to_bp_.erase(it);
}

void DebugAdapterContext::StoreFunctionBreakpoint(Breakpoint* bp) {
  FX_DCHECK(bp);
  function_bps_.push_back(bp->GetWeakPtr());
}

void DebugAdapterContext::DeleteAllFunctionBreakpoints() {
  for (auto& bp : function_bps_) {
    if (bp) {
      breakpoint_to_id_.erase(bp.get());
      session()->system().DeleteBreakpoint(bp.get());
    }
  }
  function_bps_.clear();
}

void DebugAdapterContext::DeleteAllBreakpoints() {
  DeleteAllFunctionBreakpoints();

  for (auto& it : source_to_bp_) {
    for (auto& bp : it.second) {
      if (bp) {
        session()->system().DeleteBreakpoint(bp.get());
      }
    }
  }
  breakpoint_to_id_.clear();
  source_to_bp_.clear();
}

void DebugAdapterContext::StoreFilter(Filter* filter) {
  FX_DCHECK(filter);
  if (std::find(filters_.begin(), filters_.end(), filter) == filters_.end()) {
    filters_.push_back(filter);
  }
}

// Deletes all filters created dynamically by this DAP connection context.
//
// We manage and clean up our own filters locally via `filters_` instead of calling the global
// `session()->system().GetFilters()` because the debugger process and the core Session survive
// DAP disconnections. Deleting all global filters on connection teardown would destructively
// wipe out the developer's pre-configured startup filters loaded from `~/.fuchsia/debug/zxdbrc`
// (e.g., `attach cobalt.cm`).
void DebugAdapterContext::DeleteAllFilters() {
  // Safeguard against null session during final context destruction
  if (!session()) {
    return;
  }

  // system().DeleteFilter calls WillDestroyFilter which will erase the iterator while we are
  // iterating. Iterate through the copied filters instead.
  auto filters_to_delete = filters_;
  for (auto* filter : filters_to_delete) {
    session()->system().DeleteFilter(filter);
  }
  filters_.clear();
}

void DebugAdapterContext::WillDestroyFilter(Filter* filter) {
  // Remove the filter from our local tracking vector if it is destroyed externally.
  auto it = std::find(filters_.begin(), filters_.end(), filter);
  if (it != filters_.end()) {
    filters_.erase(it);
  }
}

}  // namespace zxdb
