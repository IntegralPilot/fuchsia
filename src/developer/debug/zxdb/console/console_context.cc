// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/console_context.h"

#include <inttypes.h>
#include <lib/syslog/cpp/macros.h>

#include <atomic>
#include <chrono>
#include <thread>

#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/zxdb/client/breakpoint.h"
#include "src/developer/debug/zxdb/client/breakpoint_location.h"
#include "src/developer/debug/zxdb/client/filter.h"
#include "src/developer/debug/zxdb/client/frame.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/setting_schema_definition.h"
#include "src/developer/debug/zxdb/client/source_file_provider_impl.h"
#include "src/developer/debug/zxdb/client/symbol_server.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/console/command.h"
#include "src/developer/debug/zxdb/console/command_utils.h"
#include "src/developer/debug/zxdb/console/console.h"
#include "src/developer/debug/zxdb/console/format_context.h"
#include "src/developer/debug/zxdb/console/format_exception.h"
#include "src/developer/debug/zxdb/console/format_location.h"
#include "src/developer/debug/zxdb/console/format_target.h"
#include "src/developer/debug/zxdb/console/format_thread.h"
#include "src/developer/debug/zxdb/format/format.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"
#include "src/developer/debug/zxdb/symbols/function.h"
#include "src/developer/debug/zxdb/symbols/loaded_module_symbols.h"
#include "src/developer/debug/zxdb/symbols/location.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace zxdb {

namespace {

// How long to wait before printing another "." in the console while loading large amounts of
// symbols, in seconds.
constexpr auto kSymbolLoadingPrintInterval = std::chrono::seconds(2);

}  // namespace

ConsoleContext::ConsoleContext(Session* session) : session_(session) {
  session->AddObserver(this);
  session->AddDownloadObserver(this);
  session->AddBreakpointObserver(this);

  session->target_observers().AddObserver(this);
  session->process_observers().AddObserver(this);
  session->thread_observers().AddObserver(this);
  session->component_observers().AddObserver(this);

  session->system().AddObserver(this);
  session->system().settings().AddObserver(ClientSettings::System::kConsoleMode, this);

  // Pick up any previously created targets. This will normally just be the
  // default one.
  for (Target* target : session->system().GetTargets())
    DidCreateTarget(target);

  for (SymbolServer* symbol_server : session->system().GetSymbolServers())
    DidCreateSymbolServer(symbol_server);

  test_failure_stack_matcher_ = fxl::MakeRefCounted<TestFailureStackMatcher>();
  pretty_stack_manager_ = fxl::MakeRefCounted<PrettyStackManager>();
  // TODO(bug 43549) this should be loaded from a configuration file somehow associated with the
  // user's build instead of being hardcoded. This call can then be deleted.
  pretty_stack_manager_->LoadDefaultMatchers();
}

ConsoleContext::~ConsoleContext() {
  // Unregister for all observers.
  session_->system().settings().RemoveObserver(ClientSettings::System::kConsoleMode, this);
  session_->system().RemoveObserver(this);
  session_->component_observers().RemoveObserver(this);
  session_->target_observers().RemoveObserver(this);
  session_->process_observers().RemoveObserver(this);
  session_->thread_observers().RemoveObserver(this);
  session_->RemoveBreakpointObserver(this);
  session_->RemoveDownloadObserver(this);
  session_->RemoveObserver(this);
}

int ConsoleContext::IdForTarget(const Target* target) const {
  const auto& found = target_to_id_.find(target);
  if (found == target_to_id_.end()) {
    FX_NOTREACHED();
    return 0;
  }
  return found->second;
}

int ConsoleContext::IdForThread(const Thread* thread) const {
  const TargetRecord* record =
      const_cast<ConsoleContext*>(this)->GetTargetRecord(thread->GetProcess()->GetTarget());
  if (!record)
    return 0;

  auto found = record->thread_to_id.find(thread);
  if (found == record->thread_to_id.end()) {
    FX_NOTREACHED();
    return 0;
  }
  return found->second;
}

int ConsoleContext::IdForFrame(const Frame* frame) const {
  // Find the frame in the thread's backtrace. We don't have to worry about
  // whether the frames have been synced, since if there is a frame here,
  // we know it's present in the thread's list.
  Thread* thread = frame->GetThread();
  const Stack& stack = thread->GetStack();
  for (size_t i = 0; i < stack.size(); i++) {
    if (stack[i] == frame)
      return static_cast<int>(i);
  }
  FX_NOTREACHED();  // Should have found the frame.
  return 0;
}

int ConsoleContext::IdForSymbolServer(const SymbolServer* symbol_server) const {
  const auto& found = symbol_server_to_id_.find(symbol_server);
  if (found == symbol_server_to_id_.end()) {
    FX_NOTREACHED();
    return 0;
  }
  return found->second;
}

int ConsoleContext::IdForBreakpoint(const Breakpoint* breakpoint) const {
  FX_DCHECK(!breakpoint->IsInternal())
      << "Should not be trying to get the ID of internal breakpoints. The "
         "client layer should filter these out.";

  auto found = breakpoint_to_id_.find(breakpoint);
  if (found == breakpoint_to_id_.end()) {
    FX_NOTREACHED();
    return 0;
  }
  return found->second;
}

int ConsoleContext::IdForFilter(const Filter* filter) const {
  auto found = filter_to_id_.find(filter);
  if (found == filter_to_id_.end()) {
    FX_NOTREACHED();
    return 0;
  }
  return found->second;
}

void ConsoleContext::SetActiveTarget(const Target* target) {
  auto found = target_to_id_.find(target);
  if (found == target_to_id_.end()) {
    FX_NOTREACHED();
    return;
  }
  active_target_id_ = found->second;
}

int ConsoleContext::GetActiveTargetId() const { return active_target_id_; }

Target* ConsoleContext::GetActiveTarget() const {
  auto found = id_to_target_.find(active_target_id_);
  if (found == id_to_target_.end())
    return nullptr;
  return found->second.target;
}

void ConsoleContext::SetActiveSymbolServer(const SymbolServer* symbol_server) {
  auto found = symbol_server_to_id_.find(symbol_server);
  if (found == symbol_server_to_id_.end()) {
    FX_NOTREACHED();
    return;
  }
  active_symbol_server_id_ = found->second;
}

int ConsoleContext::GetActiveSymbolServerId() const { return active_symbol_server_id_; }

SymbolServer* ConsoleContext::GetActiveSymbolServer() const {
  auto found = id_to_symbol_server_.find(active_symbol_server_id_);
  if (found == id_to_symbol_server_.end())
    return nullptr;
  return found->second;
}

void ConsoleContext::SetActiveThreadForTarget(const Thread* thread) {
  TargetRecord* record = GetTargetRecord(thread->GetProcess()->GetTarget());
  if (!record)
    return;

  auto found = record->thread_to_id.find(thread);
  if (found == record->thread_to_id.end()) {
    FX_NOTREACHED();
    return;
  }
  record->active_thread_id = found->second;
}

int ConsoleContext::GetActiveThreadIdForTarget(const Target* target) {
  const TargetRecord* record = GetTargetRecord(target);
  if (!record) {
    FX_NOTREACHED();
    return 0;
  }
  return record->active_thread_id;
}

Thread* ConsoleContext::GetActiveThreadForTarget(const Target* target) {
  const TargetRecord* record = GetTargetRecord(target);
  if (!record) {
    FX_NOTREACHED();
    return nullptr;
  }

  auto found = record->id_to_thread.find(record->active_thread_id);
  if (found == record->id_to_thread.end())
    return nullptr;
  return found->second.thread;
}

void ConsoleContext::SetActiveFrameForThread(const Frame* frame) {
  ThreadRecord* record = GetThreadRecord(frame->GetThread());
  if (!record) {
    FX_NOTREACHED();
    return;
  }
  record->active_frame_id = IdForFrame(frame);
}

void ConsoleContext::SetActiveFrameIdForThread(const Thread* thread, int id) {
  ThreadRecord* record = GetThreadRecord(thread);
  if (!record) {
    FX_NOTREACHED();
    return;
  }
  record->active_frame_id = id;
}

int ConsoleContext::GetActiveFrameIdForThread(const Thread* thread) const {
  const ThreadRecord* record = GetThreadRecord(thread);
  if (!record) {
    FX_NOTREACHED();
    return 0;
  }

  // Should be a valid frame index in the thread (or no frames and == 0).
  FX_DCHECK((thread->GetStack().empty() && record->active_frame_id == 0) ||
            (record->active_frame_id >= 0 &&
             record->active_frame_id < static_cast<int>(thread->GetStack().size())));
  return record->active_frame_id;
}

void ConsoleContext::SetActiveBreakpoint(const Breakpoint* breakpoint) {
  int id = IdForBreakpoint(breakpoint);
  if (id != 0)
    active_breakpoint_id_ = id;
}

int ConsoleContext::GetActiveBreakpointId() const { return active_breakpoint_id_; }

Breakpoint* ConsoleContext::GetActiveBreakpoint() const {
  if (active_breakpoint_id_ == 0)
    return nullptr;
  auto found = id_to_breakpoint_.find(active_breakpoint_id_);
  if (found == id_to_breakpoint_.end()) {
    FX_NOTREACHED();
    return nullptr;
  }
  return found->second;
}

void ConsoleContext::SetActiveFilter(const Filter* filter) {
  int id = IdForFilter(filter);
  if (id != 0)
    active_filter_id_ = id;
}

int ConsoleContext::GetActiveFilterId() const { return active_filter_id_; }

Filter* ConsoleContext::GetActiveFilter() const {
  if (active_filter_id_ == 0)
    return nullptr;
  auto found = id_to_filter_.find(active_filter_id_);
  if (found == id_to_filter_.end()) {
    FX_NOTREACHED();
    return nullptr;
  }
  return found->second;
}

SourceAffinity ConsoleContext::GetSourceAffinityForThread(const Thread* thread) const {
  const ThreadRecord* record = GetThreadRecord(thread);
  if (!record)
    return SourceAffinity::kSource;
  return record->source_affinity;
}

void ConsoleContext::SetSourceAffinityForThread(const Thread* thread,
                                                SourceAffinity source_affinity) {
  if (source_affinity == SourceAffinity::kNone)
    return;  // Don't change anything, previous command still stands.

  ThreadRecord* record = GetThreadRecord(thread);
  if (!record)
    return;
  record->source_affinity = source_affinity;
}

void ConsoleContext::ScheduleDisplayExpressions(Thread* thread) const {
  std::vector<std::string> exprs = thread->settings().GetList(ClientSettings::Thread::kDisplay);
  if (exprs.empty())
    return;

  // Thread stops should always have a frame.
  const Stack& stack = thread->GetStack();
  if (stack.empty())
    return;
  const Frame* frame = stack[0];
  fxl::RefPtr<EvalContext> eval_context = frame->GetEvalContext();

  // When something is printed every time, assume the user wants to see relatively little detail.
  FormatBufferOptions options;
  options.verbosity = FormatBufferOptions::Verbosity::kMinimal;
  options.wrapping = FormatBufferOptions::Wrapping::kSmart;
  options.pointer_expand_depth = 2;

  if (thread->GetProcess()) {
    options.num_format = GetNumberFormatForTarget(thread->GetProcess()->GetTarget());
  }

  Console::get()->Output(FormatExpressions(exprs, options, eval_context));
}

Err ConsoleContext::FillOutCommand(Command* cmd) const {
  // Target.
  const TargetRecord* target_record = nullptr;
  Err result = FillOutTarget(cmd, &target_record);
  if (result.has_error())
    return result;

  // Thread.
  const ThreadRecord* thread_record = nullptr;
  result = FillOutThread(cmd, target_record, &thread_record);
  if (result.has_error())
    return result;

  // Frame.
  result = FillOutFrame(cmd, thread_record);
  if (result.has_error())
    return result;

  // Breakpoint.
  result = FillOutBreakpoint(cmd);
  if (result.has_error())
    return result;

  // Filter.
  result = FillOutFilter(cmd);
  if (result.has_error())
    return result;

  // SymbolServer.
  result = FillOutSymbolServer(cmd);
  if (result.has_error())
    return result;

  return Err();
}

std::string ConsoleContext::GetConsoleMode() {
  return session()->system().settings().GetString(ClientSettings::System::kConsoleMode);
}

std::string ConsoleContext::GetEmbeddedModeContextOrDefault(
    std::optional<debug_ipc::ExceptionType> type) {
  if (auto context_string =
          session()->system().settings().GetString(ClientSettings::System::kEmbeddedModeContext);
      !context_string.empty()) {
    return context_string;
  } else if (type) {
    return debug_ipc::ExceptionTypeToString(*type);
  }

  // Give up and just return something generic.
  return "error";
}

void ConsoleContext::SetConsoleMode(std::string mode) {
  session()->system().settings().SetString(ClientSettings::System::kConsoleMode, std::move(mode));
  // If the mode changes, we will get notified via SettingStoreObserver.
}

void ConsoleContext::MaybeReturnToEmbeddedMode(Process* process) {
  auto running_processes = GetRunningProcesses(&session()->system());

  // If |process| is null, then the presence of any running process indicates that we should not
  // transition to embedded mode. If |process| is the only running process, then we will return to
  // embedded mode.
  bool has_other_running_process = !running_processes.empty();
  if (process) {
    has_other_running_process = !std::all_of(
        running_processes.begin(), running_processes.end(),
        [process](const Process* running) { return running->GetKoid() == process->GetKoid(); });
  }

  // We return to embedded mode if there are no other running targets or if the previous command was
  // executed against all targets (indicating the user doesn't want to debug them).
  if (GetConsoleMode() == ClientSettings::System::kConsoleMode_EmbeddedInteractive &&
      !has_other_running_process) {
    SetConsoleMode(ClientSettings::System::kConsoleMode_Embedded);
  }
}

void ConsoleContext::InitConsoleMode() {
  std::string mode = GetConsoleMode();
  Console* console = Console::get();

  if (mode == ClientSettings::System::kConsoleMode_Shell ||
      mode == ClientSettings::System::kConsoleMode_EmbeddedInteractive) {
    console->DisableStreaming();
    console->EnableInput();
    console->EnableOutput();
  }

  // Report which console mode we are for analytics. This doesn't double report when we're running
  // with the debug adapter running, which will use the noninteractive console.
  if (mode != ClientSettings::System::kConsoleMode_NonInteractive) {
    if (mode == ClientSettings::System::kConsoleMode_Embedded ||
        mode == ClientSettings::System::kConsoleMode_EmbeddedInteractive) {
      session()->analytics().ReportConsoleType(ConsoleType::Type::kCommandLineEmbedded);
    } else {
      session()->analytics().ReportConsoleType(ConsoleType::Type::kCommandLine);
    }
  }
}

void ConsoleContext::HandleNotification(NotificationType type, const std::string& msg) {
  OutputBuffer out;
  auto preamble = fxl::StringPrintf("[%s] ", NotificationTypeToString(type));
  switch (type) {
    case NotificationType::kError:
      out.Append(Syntax::kError, std::move(preamble));
      [[fallthrough]];
    case NotificationType::kWarning:
      out.Append(Syntax::kWarning, std::move(preamble));
      [[fallthrough]];
    case NotificationType::kProcessEnteredLimbo:
      [[fallthrough]];
    case NotificationType::kProcessStdout:
      [[fallthrough]];
    case NotificationType::kProcessStderr:
      break;
    case NotificationType::kNone:  // None is a no-op.
      return;
  }

  out.Append(msg);
  Console::get()->Output(std::move(out));
}

void ConsoleContext::HandlePreviousConnectedProcesses(
    const std::vector<debug_ipc::ProcessRecord>& processes) {
  OutputBuffer out(
      OutputBuffer{Syntax::kHeading, "Attaching to previously connected processes:\n"});
  for (auto& process : processes) {
    out.Append(
        fxl::StringPrintf("%" PRIu64 ": %s\n", process.process_koid, process.process_name.c_str()));
  }

  Console::get()->Output(std::move(out));
}

void ConsoleContext::HandleProcessesInLimbo(
    const std::vector<debug_ipc::ProcessRecord>& processes) {
  OutputBuffer out(OutputBuffer{Syntax::kHeading, "Processes attached from limbo:\n"});
  for (auto& process : processes) {
    out.Append(fxl::StringPrintf("  %" PRIu64 ": %s\n", process.process_koid,
                                 process.process_name.c_str()));
  }
  out.Append(OutputBuffer{
      Syntax::kComment,
      "Type \"detach <pid>\" to send back to Process Limbo if attached,\n"
      "type \"detach <pid>\" again to terminate the process if not attached, or\n"
      "type \"process <process context #> kill\" to terminate the process if attached.\n"
      "See \"help jitd\" for more information on Just-In-Time-Debugging.\n"});

  Console::get()->Output(std::move(out));
}

void ConsoleContext::DidCreateBreakpoint(Breakpoint* breakpoint) {
  int id = next_breakpoint_id_;
  next_breakpoint_id_++;

  id_to_breakpoint_[id] = breakpoint;
  breakpoint_to_id_[breakpoint] = id;
}

void ConsoleContext::WillDestroyBreakpoint(Breakpoint* breakpoint) {
  auto found_breakpoint = breakpoint_to_id_.find(breakpoint);
  if (found_breakpoint == breakpoint_to_id_.end()) {
    FX_NOTREACHED();
    return;
  }
  int id = found_breakpoint->second;

  // Clear any active breakpoint if it's the deleted one.
  if (active_breakpoint_id_ == id)
    active_breakpoint_id_ = 0;

  id_to_breakpoint_.erase(id);
  breakpoint_to_id_.erase(found_breakpoint);
}

void ConsoleContext::DidCreateFilter(Filter* filter) {
  int id = next_filter_id_;
  next_filter_id_++;

  id_to_filter_[id] = filter;
  filter_to_id_[filter] = id;
}

void ConsoleContext::WillDestroyFilter(Filter* filter) {
  auto found = filter_to_id_.find(filter);
  if (found == filter_to_id_.end()) {
    FX_NOTREACHED();
    return;
  }

  if (active_filter_id_ == found->second)
    active_filter_id_ = 0;

  id_to_filter_.erase(found->second);
  filter_to_id_.erase(found);
}

void ConsoleContext::DidCreateSymbolServer(SymbolServer* symbol_server) {
  int id = next_symbol_server_id_;
  next_symbol_server_id_++;

  id_to_symbol_server_[id] = symbol_server;
  symbol_server_to_id_[symbol_server] = id;

  if (active_symbol_server_id_ == 0) {
    active_symbol_server_id_ = id;
  }
}

void ConsoleContext::DidCreateTarget(Target* target) {
  int new_id = next_target_id_;
  next_target_id_++;

  TargetRecord record;
  record.target_id = new_id;
  record.target = target;

  id_to_target_[new_id] = std::move(record);
  target_to_id_[target] = new_id;

  // Set the active target only if there's none already.
  if (active_target_id_ == 0)
    active_target_id_ = new_id;
}

void ConsoleContext::WillDestroyTarget(Target* target) {
  int deleted_target_id = 0;

  {
    TargetRecord* record = GetTargetRecord(target);
    if (!record) {
      FX_NOTREACHED();
      return;
    }

    deleted_target_id = record->target_id;

    // There should be no threads by the time we erase the target mapping.
    FX_DCHECK(record->id_to_thread.empty());
    FX_DCHECK(record->thread_to_id.empty());

    target_to_id_.erase(target);
    id_to_target_.erase(deleted_target_id);
    // *record is now invalid.
  }

  if (active_target_id_ == deleted_target_id) {
    // Need to update the default target ID.
    if (id_to_target_.empty()) {
      // This should only happen in the shutting-down case.
      active_target_id_ = 0;
    } else {
      // Just pick the first target to be the active one. It might be nice to
      // have an ordering of which one the user had selected previously in
      // case they're toggling between two.
      active_target_id_ = id_to_target_.begin()->first;
    }
  }
}

void ConsoleContext::DidCreateProcess(Process* process, uint64_t timestamp) {
  TargetRecord* record = GetTargetRecord(process->GetTarget());
  if (!record) {
    FX_NOTREACHED();
    return;
  }

  // Restart the thread ID counting when the process starts in case this
  // target was previously running (we want to restart numbering every time).
  record->next_thread_id = 1;

  OutputBuffer out;
  switch (process->start_type()) {
    case Process::StartType::kAttach:
      out.Append("Attached ");
      break;
    case Process::StartType::kLaunch:
      out.Append("Launched ");
      break;
  }

  if (GetConsoleMode() == ClientSettings::System::kConsoleMode_Shell) {
    out.Append(FormatTarget(this, process->GetTarget()));
    Console::get()->Output(out);
  }
}

void ConsoleContext::DidLoadAllModuleSymbols(Process* process) {
  // Release the lock to signal the printing thread it's time to stop.
  lock_timer_.unlock();

  // Clean up the thread if we spawned one and give the console back to the user.
  if (symbol_loading_printer_thread_ && symbol_loading_printer_thread_->joinable()) {
    symbol_loading_printer_thread_->join();

    // The symbols have been processed, indicate and give the console back to the user.
    Console::get()->Output("Done.");
    // This must be called from the same thread as the MessageLoop, so the spawned thread cannot be
    // responsible for re-enabling the console input.
    Console::get()->EnableInput();
  }

  symbol_loading_printer_thread_.reset();
}

void ConsoleContext::WillDestroyProcess(Process* process, DestroyReason reason, int exit_code,
                                        uint64_t timestamp) {
  TargetRecord* record = GetTargetRecord(process->GetTarget());
  if (!record) {
    FX_NOTREACHED();
    return;
  }

  int process_index = IdForTarget(process->GetTarget());

  Console* console = Console::get();
  std::string msg;
  switch (reason) {
    case ProcessObserver::DestroyReason::kExit:
      msg = fxl::StringPrintf("Process %d exited with code %d.", process_index, exit_code);
      break;
    case ProcessObserver::DestroyReason::kDetach:
      msg = fxl::StringPrintf("Process %d detached.", process_index);
      break;
    case ProcessObserver::DestroyReason::kKill:
      msg = fxl::StringPrintf("Process %d killed.", process_index);
      break;
  }

  if (GetConsoleMode() == ClientSettings::System::kConsoleMode_Shell)
    console->Output(msg);

  MaybeReturnToEmbeddedMode(process);
}

void ConsoleContext::WillLoadModuleSymbols(Process* process, int num_modules) {
  if (!process)
    return;

  Console* console = Console::get();

  if (GetConsoleMode() == ClientSettings::System::kConsoleMode_Embedded) {
    // Disable streaming so we can print out a message. Balanced in |OnThreadStopped| (see the
    // comment there for details).
    console->DisableStreaming();

    OutputBuffer out;
    out.Append(Syntax::kHeading, "\n👋 zxdb is loading symbols to debug " +
                                     GetEmbeddedModeContextOrDefault(std::nullopt) + " in " +
                                     process->GetName() + ", please wait.\n");

    // Use Write directly here since we're not transitioning out of Embedded mode yet, but would
    // like to print something out so the user knows the console is not stuck.
    console->Write(out);

    return;
  }

  // Disable the console while the symbols are loaded. Once processing has finished, re-enable.
  console->DisableInput();

  OutputBuffer out("Loading ");
  out.Append(std::to_string(num_modules));
  out.Append(" modules for " + process->GetName() + " ");
  console->Output(out, false);

  lock_timer_.lock();
  symbol_loading_printer_thread_ = std::make_unique<std::thread>([this, console]() {
    const OutputBuffer out(".");
    while (!lock_timer_.try_lock_for(kSymbolLoadingPrintInterval)) {
      console->Output(out, false);
    }
    // If the lock attempt succeeds, then we're done loading and indexing symbols from this module.
    lock_timer_.unlock();
    return;
  });
}

void ConsoleContext::DidCreateThread(Thread* thread) {
  TargetRecord* record = GetTargetRecord(thread->GetProcess()->GetTarget());
  if (!record) {
    FX_NOTREACHED();
    return;
  }

  int thread_id = record->next_thread_id;
  record->next_thread_id++;

  record->id_to_thread[thread_id].thread = thread;
  record->thread_to_id[thread] = thread_id;

  // Only make a new thread the default if there is no current thread,
  // otherwise the context will be swapping out from under the user as the
  // program runs.
  if (record->active_thread_id == 0)
    record->active_thread_id = thread_id;
}

void ConsoleContext::WillDestroyThread(Thread* thread) {
  TargetRecord* record = GetTargetRecord(thread->GetProcess()->GetTarget());
  if (!record) {
    FX_NOTREACHED();
    return;
  }

  auto found_thread_to_id = record->thread_to_id.find(thread);
  if (found_thread_to_id == record->thread_to_id.end()) {
    FX_NOTREACHED();
    return;
  }
  int thread_id = found_thread_to_id->second;

  record->id_to_thread.erase(found_thread_to_id->second);
  record->thread_to_id.erase(found_thread_to_id);

  // Update the active thread if the currently active one is being deleted.
  if (thread_id == record->active_thread_id) {
    // Just pick the first thread to be the active one. It might be nice to
    // have an ordering of which one the user had selected previously in
    // case they're toggling between two.
    if (record->id_to_thread.empty()) {
      record->active_thread_id = 0;
    } else {
      record->active_thread_id = record->id_to_thread.begin()->first;
    }
  }
}

void ConsoleContext::OnSymbolLoadFailure(Process* process, const Err& err) {
  Console::get()->Output(err);
}

void ConsoleContext::WillAutomaticallyContinue(debug_ipc::ResumeRequest::How how,
                                               const StopInfo& info) {
  OutputBuffer out;

  bool is_second_chance =
      info.exception_record.strategy == debug_ipc::ExceptionStrategy::kSecondChance;
  out.Append(Syntax::kComment,
             fxl::StringPrintf("Automatically continuing over%sexception: %s\n",
                               is_second_chance ? " second-chance " : " ",
                               debug_ipc::ExceptionTypeToString(info.exception_type)));

  if (is_second_chance) {
    out.Append(Syntax::kComment, "The program will likely be killed now.");
  }

  // It's okay to dispatch this unconditionally since most of the time this observer event will be
  // triggered when they're using `fx test` to debug tests, which means we'll (typically) go
  // straight back to embedded mode when they're done. This can be useful for zxdb developers when
  // operating the debugger outside of these contexts but still targeting similar usecases (e.g.
  // outside of embedded mode).
  Console::get()->Output(out);
}

void ConsoleContext::OnThreadStopped(Thread* thread, const StopInfo& info) {
  // The stopped, process, thread, and frame should be active.
  Target* target = thread->GetProcess()->GetTarget();
  SetActiveTarget(target);
  SetActiveThreadForTarget(thread);
  SetActiveFrameIdForThread(thread, 0);
  SetActiveBreakpointForStop(info);

  bool should_hide_exception_info = false;

  if (GetConsoleMode() == ClientSettings::System::kConsoleMode_Embedded) {
    SetConsoleMode(ClientSettings::System::kConsoleMode_EmbeddedInteractive);

    // This is counterintuitive here because we just transitioned into EmbeddedInteractive mode
    // where we do not want streaming to be enabled. However, this is the required balancing for the
    // DisableStreaming call made in |WillLoadModuleSymbols|. Since we were in Embedded mode, we
    // know that that the WillLoadModuleSymbols notification will ONLY be sent if we're about to
    // make the transition to EmbeddedInteractive (otherwise we would never load modules due to the
    // use of Weak attaching).
    //
    // It's important that we do this AFTER setting the console mode, so that the streamer doesn't
    // flicker the screen between these two calls.
    Console::get()->EnableStreaming();

    auto embedded_mode_context = GetEmbeddedModeContextOrDefault(info.exception_type);

    OutputBuffer out(Syntax::kHeading, "⚠️  " + embedded_mode_context + " in " +
                                           thread->GetProcess()->GetName() +
                                           ", type `frame` or `help` to get started.");
    Console::get()->Output(out);

    // Check to see if we can find a test failure frame. If we find a match, set that frame as
    // "active".
    size_t best_frame_index = test_failure_stack_matcher_->Match(thread->GetStack());
    if (best_frame_index > 0) {
      // If we found a matching frame, then we don't need to show the exception information in the
      // stop output.
      should_hide_exception_info = true;
      SetActiveFrameForThread(thread->GetStack()[best_frame_index]);
      auto embedded_mode_context = GetEmbeddedModeContextOrDefault(info.exception_type);

      OutputBuffer out;
      out.Append(Syntax::kHeading, "⚠️  " + embedded_mode_context + " in " +
                                       thread->GetProcess()->GetName() +
                                       ", type `frame` or `help` to get started.\n");

      out.Append(Syntax::kComment,
                 "Exception details hidden by default. Use `exception-info` to print detailed "
                 "information about the stoppage.\n");

      Console::get()->Output(out);

      SetActiveFrameForThread(thread->GetStack()[best_frame_index]);

      // We detected a test, update the process kind so it can take special actions later if
      // it wants to.
      thread->GetProcess()->set_kind(Process::Kind::kTest);
    }
  }

  // Show the location information.
  Console::get()->Output(FormatThreadStop(this, thread, info, should_hide_exception_info));

  ScheduleDisplayExpressions(thread);
}

void ConsoleContext::DidUpdateStackFrames(Thread* thread) {
  ThreadRecord* record = GetThreadRecord(thread);
  if (!record) {
    FX_NOTREACHED();
    return;
  }

  // We don't really know what changed. We don't want to reset the active frame ID every time since
  // one of the update cases is that the frames have been appended to (so existing indices are still
  // valid) or that symbols are loaded (normally this means that the frames are unchanged, though
  // inline frames can get expanded in some cases).
  //
  // As a result, keep the index the unchanged unless it's now out-of-bounds. If symbols are loaded
  // and inline frames expand things, the current frame could possibly change. But normally the user
  // will be at frame 0 in this case anyway, and this avoids resetting any state in the more common
  // cases.
  if (record->active_frame_id >= static_cast<int>(thread->GetStack().size()))
    record->active_frame_id = 0;
}

void ConsoleContext::OnDownloadsStarted() { Console::get()->Output("Downloading symbols..."); }

void ConsoleContext::OnDownloadsStopped(size_t success, size_t fail) {
  Console::get()->Output(
      fxl::StringPrintf("Symbol downloading complete. %zu succeeded, %zu failed.", success, fail));

  if (fail > 0) {
    Console::get()->Output(
        "\nSome files failed to download. If you think this is in error, please file a bug:\n"
        "https://fxbug.dev/issues/new?component=1389559&template=1849567\n");
  }
}

void ConsoleContext::OnBreakpointImplicitUpdate(Breakpoint* breakpoint,
                                                BreakpointObserver::What what) {
  if (what == BreakpointObserver::What::kType) {
    OutputBuffer buf;

    // These are the input locations, which don't necessarily have any resolved symbols, but we
    // don't care about those here. Instead, we want to spit out the same thing that the user typed
    // in.
    const auto& input_locations = breakpoint->GetSettings().locations;

    std::string location_string;
    for (auto input_loc = input_locations.begin(); location_string.empty(); ++input_loc) {
      switch (input_loc->type) {
        case InputLocation::Type::kLine:
          location_string =
              fxl::StringPrintf("%s:%d", input_loc->line.file().c_str(), input_loc->line.line());
          break;
        case InputLocation::Type::kName:
          location_string = input_loc->name.GetFullNameNoQual();
          break;
        case InputLocation::Type::kAddress:
          location_string = std::to_string(input_loc->address);
          break;
        case InputLocation::Type::kNone:
          break;
      }
    }

    buf.Append(
        Syntax::kWarning,
        fxl::StringPrintf(
            "Software breakpoints in starnix kernel are currently not supported. This breakpoint "
            "has been converted to a hardware breakpoint.\nHardware breakpoints can be specified "
            "on the command line:\n\tbreak --type execute %s\nSee `help break` for details and "
            "follow https://fxbug.dev/396421111 for updates on enabling software breakpoints in "
            "starnix_kernel.",
            location_string.c_str()));

    Console::get()->Output(buf);
  }
}

void ConsoleContext::OnBreakpointMatched(Breakpoint* breakpoint, bool user_requested) {
  if (user_requested)
    return;  // Don't need to notify for user-requested changes.

  BreakpointSettings settings = breakpoint->GetSettings();
  size_t matched_locs = breakpoint->GetLocations().size();

  OutputBuffer out("Breakpoint ");
  out.Append(Syntax::kSpecial, std::to_string(IdForBreakpoint(breakpoint)));
  out.Append(fxl::StringPrintf(" now matching %zu addrs for ", matched_locs));
  out.Append(FormatInputLocations(settings.locations));

  Console::get()->Output(out);
}

void ConsoleContext::OnBreakpointUpdateFailure(Breakpoint* breakpoint, const Err& err) {
  Console* console = Console::get();
  if (breakpoint->IsInternal()) {
    // Although the user didn't explicitly set this breakpoint, they presumably were involved in
    // some operation that caused it to be made. Notify of the error so they know it's not working.
    console->Output(Err("Error updating internal breakpoint:\n" + err.msg()));
  } else {
    OutputBuffer out;
    out.Append("Error updating ");
    out.Append(FormatBreakpoint(this, breakpoint, false));
    out.Append(err);
    console->Output(out);
  }
}

ConsoleContext::TargetRecord* ConsoleContext::GetTargetRecord(int target_id) {
  return const_cast<TargetRecord*>(
      const_cast<const ConsoleContext*>(this)->GetTargetRecord(target_id));
}

void ConsoleContext::OnComponentStarted(const std::string& moniker, const std::string& url) {
  OutputBuffer out("Component Started");
  out.Append(Syntax::kVariable, " moniker");
  out.Append("=" + FormatConsoleString(moniker));
  out.Append(Syntax::kVariable, " url");
  out.Append("=" + FormatConsoleString(url));
  Console::get()->Output(out);
}

void ConsoleContext::OnComponentExited(const std::string& moniker, const std::string& url) {
  OutputBuffer out("Component Exited");
  out.Append(Syntax::kVariable, " moniker");
  out.Append("=" + FormatConsoleString(moniker));
  out.Append(Syntax::kVariable, " url");
  out.Append("=" + FormatConsoleString(url));
  Console::get()->Output(out);
}

void ConsoleContext::OnSettingChanged(const SettingStore&, const std::string& setting_name) {
  if (setting_name == ClientSettings::System::kConsoleMode) {
    std::string mode = GetConsoleMode();
    Console* console = Console::get();

    if (mode == ClientSettings::System::kConsoleMode_Shell ||
        mode == ClientSettings::System::kConsoleMode_EmbeddedInteractive) {
      console->DisableStreaming();
      console->EnableInput();
      console->EnableOutput();
    } else if (mode == ClientSettings::System::kConsoleMode_NonInteractive) {
      // TODO(https://fxbug.dev/462395065): Consider disabling input here.
      console->EnableInput();
      console->EnableOutput();
      // Note that Streaming is always enabled when in the noninteractive mode.
    } else {
      console->DisableInput();
      console->DisableOutput();
      console->EnableStreaming();
    }
  } else {
    LOGS(Warn) << "Console context handling invalid setting " << setting_name;
  }
}

const ConsoleContext::TargetRecord* ConsoleContext::GetTargetRecord(int target_id) const {
  auto found_to_record = id_to_target_.find(target_id);
  if (found_to_record == id_to_target_.end()) {
    FX_NOTREACHED();
    return nullptr;
  }
  return &found_to_record->second;
}

ConsoleContext::TargetRecord* ConsoleContext::GetTargetRecord(const Target* target) {
  return const_cast<TargetRecord*>(
      const_cast<const ConsoleContext*>(this)->GetTargetRecord(target));
}

const ConsoleContext::TargetRecord* ConsoleContext::GetTargetRecord(const Target* target) const {
  auto found_to_id = target_to_id_.find(target);
  if (found_to_id == target_to_id_.end()) {
    FX_NOTREACHED();
    return nullptr;
  }
  return GetTargetRecord(found_to_id->second);
}

ConsoleContext::ThreadRecord* ConsoleContext::GetThreadRecord(const Thread* thread) {
  // Share implementation with the non-const version.
  return const_cast<ThreadRecord*>(
      const_cast<const ConsoleContext*>(this)->GetThreadRecord(thread));
}

const ConsoleContext::ThreadRecord* ConsoleContext::GetThreadRecord(const Thread* thread) const {
  const TargetRecord* target_record = GetTargetRecord(thread->GetProcess()->GetTarget());
  if (!target_record) {
    FX_NOTREACHED();
    return nullptr;
  }

  auto found_thread_to_id = target_record->thread_to_id.find(thread);
  if (found_thread_to_id == target_record->thread_to_id.end()) {
    FX_NOTREACHED();
    return nullptr;
  }
  int thread_id = found_thread_to_id->second;

  auto found_id_to_thread = target_record->id_to_thread.find(thread_id);
  if (found_thread_to_id == target_record->thread_to_id.end()) {
    FX_NOTREACHED();
    return nullptr;
  }
  return &found_id_to_thread->second;
}

Err ConsoleContext::FillOutTarget(Command* cmd, TargetRecord const** out_target_record) const {
  int target_id = cmd->GetNounIndex(Noun::kProcess);
  if (target_id == Command::kNoIndex) {
    // No index: use the active one (which should always exist).
    target_id = active_target_id_;
    auto found_target = id_to_target_.find(target_id);
    FX_DCHECK(found_target != id_to_target_.end());
    cmd->add_target(found_target->second.target);

    FX_DCHECK(cmd->target());  // Default target should always exist.
    *out_target_record = GetTargetRecord(target_id);
    return Err();
  } else if (target_id == Command::kWildcard) {
    return Err(ErrType::kInput, "`*` is not supported for \"process\". Did you mean `detach *`?");
  }

  // Explicit index given, look it up.
  auto found_target = id_to_target_.find(target_id);
  if (found_target == id_to_target_.end()) {
    return Err(ErrType::kInput, fxl::StringPrintf("There is no process %d.", target_id));
  }
  cmd->add_target(found_target->second.target);
  *out_target_record = GetTargetRecord(target_id);
  return Err();
}

Err ConsoleContext::FillOutThread(Command* cmd, const TargetRecord* target_record,
                                  ThreadRecord const** out_thread_record) const {
  int thread_id = cmd->GetNounIndex(Noun::kThread);
  const ThreadRecord* thread_record = nullptr;

  if (thread_id == Command::kNoIndex) {
    // No thread specified, use the default one.
    thread_id = target_record->active_thread_id;
    auto found_thread = target_record->id_to_thread.find(thread_id);
    if (found_thread == target_record->id_to_thread.end()) {
      // When there are no threads, the active thread ID will be 0 and that's
      // fine. But if it's nonzero, the thread should always be valid.
      FX_DCHECK(thread_id == 0);
    } else {
      thread_record = &found_thread->second;
      cmd->add_thread(thread_record->thread);
    }
    *out_thread_record = thread_record;
    return Err();
  } else if (thread_id == Command::kWildcard) {
    // All threads specified.

    Process* process = target_record->target->GetProcess();
    if (!process)
      return Err(ErrType::kInput, "There is no process.");

    for (auto thread : process->GetThreads()) {
      cmd->add_thread(thread);
    }

    return Err();
  }

  // Explicit index given, look it up.
  auto found_thread = target_record->id_to_thread.find(thread_id);
  if (found_thread == target_record->id_to_thread.end()) {
    if (target_record->id_to_thread.empty()) {
      return Err(ErrType::kInput, "There are no threads in the process.");
    }
    return Err(ErrType::kInput,
               fxl::StringPrintf("There is no thread %d in the process.", thread_id));
  }

  thread_record = &found_thread->second;
  cmd->add_thread(thread_record->thread);
  *out_thread_record = thread_record;
  return Err();
}

Err ConsoleContext::FillOutFrame(Command* cmd, const ThreadRecord* thread_record) const {
  int frame_id = cmd->GetNounIndex(Noun::kFrame);
  if (frame_id == Command::kNoIndex) {
    // No index: use the active one (if any).
    if (thread_record) {
      auto& stack = thread_record->thread->GetStack();
      frame_id = thread_record->active_frame_id;
      if (frame_id >= 0 && frame_id < static_cast<int>(stack.size())) {
        cmd->add_frame(stack[frame_id]);
      } else if (!stack.empty()) {
        // Invalid frame index, default to 0th frame.
        frame_id = 0;
        cmd->add_frame(stack[0]);
      }
    }
    return Err();
  }

  // Frame index specified, use it.
  if (!thread_record)
    return Err(ErrType::kInput, "There is no thread to have frames.");

  Stack& stack = thread_record->thread->GetStack();
  if (frame_id >= 0 && frame_id < static_cast<int>(stack.size())) {
    // References a valid frame. Now check that the frame index references
    // the top physical frame (or one of its inline expansions above it) or
    // all frames are synced.
    bool top_physical_frame = true;
    for (int i = 0; i < frame_id; i++) {
      if (!stack[i]->IsInline()) {
        top_physical_frame = false;
        break;
      }
    }
    if (top_physical_frame || stack.has_all_frames()) {
      cmd->add_frame(stack[frame_id]);
      return Err();
    }
  }

  // Invalid frame specified. The full backtrace list is populated on
  // demand. It could be if the frames aren't synced for the thread we
  // could delay processing this command and get the frames, but we're not
  // set up to do that (this function is currently synchronous). Instead
  // if we detect the list isn't populated and the user requested one
  // that's out-of-range, request they manually sync the list.
  //
  // Check for the presence of any frames because the thread might not be
  // in a state to have frames (i.e. it's running).
  if (!stack.empty() && !thread_record->thread->GetStack().has_all_frames()) {
    return Err(ErrType::kInput,
               "The frames for this thread haven't been synced.\n"
               "Use \"frame\" to list the frames before selecting one to "
               "populate the frame list.");
  }
  return Err(ErrType::kInput,
             "Invalid frame index.\n"
             "Use \"frame\" to list available ones.");
}

Err ConsoleContext::FillOutBreakpoint(Command* cmd) const {
  int breakpoint_id = cmd->GetNounIndex(Noun::kBreakpoint);
  if (breakpoint_id == Command::kNoIndex) {
    // No index: use the active one (which may not exist).
    cmd->add_breakpoint(GetActiveBreakpoint());
    return Err();
  } else if (breakpoint_id == Command::kWildcard) {
    for (auto bp : session_->system().GetBreakpoints()) {
      cmd->add_breakpoint(bp);
    }
    return Err();
  }

  // Explicit index given, look it up.
  auto found_breakpoint = id_to_breakpoint_.find(breakpoint_id);
  if (found_breakpoint == id_to_breakpoint_.end()) {
    return Err(ErrType::kInput, fxl::StringPrintf("There is no breakpoint %d.", breakpoint_id));
  }
  cmd->add_breakpoint(found_breakpoint->second);
  return Err();
}

Err ConsoleContext::FillOutFilter(Command* cmd) const {
  int filter_id = cmd->GetNounIndex(Noun::kFilter);
  if (filter_id == Command::kNoIndex) {
    // No index: use the active one (which may not exist).
    cmd->add_filter(GetActiveFilter());
    return Err();
  } else if (filter_id == Command::kWildcard) {
    for (auto filter : session_->system().GetFilters()) {
      cmd->add_filter(filter);
    }
    return Err();
  }

  // Explicit index given, look it up.
  auto found_filter = id_to_filter_.find(filter_id);
  if (found_filter == id_to_filter_.end()) {
    return Err(ErrType::kInput, fxl::StringPrintf("There is no filter %d.", filter_id));
  }
  cmd->add_filter(found_filter->second);
  return Err();
}

Err ConsoleContext::FillOutSymbolServer(Command* cmd) const {
  int symbol_server_id = cmd->GetNounIndex(Noun::kSymServer);
  if (symbol_server_id == Command::kNoIndex) {
    // No index: use the active one (which may not exist).
    cmd->add_sym_server(GetActiveSymbolServer());
    return Err();
  } else if (symbol_server_id == Command::kWildcard) {
    for (auto ss : session_->system().GetSymbolServers()) {
      cmd->add_sym_server(ss);
    }
    return Err();
  }

  // Explicit index given, look it up.
  auto found_symbol_server = id_to_symbol_server_.find(symbol_server_id);
  if (found_symbol_server == id_to_symbol_server_.end()) {
    return Err(ErrType::kInput,
               fxl::StringPrintf("There is no symbol server %d.", symbol_server_id));
  }
  cmd->add_sym_server(found_symbol_server->second);
  return Err();
}

void ConsoleContext::SetActiveBreakpointForStop(const StopInfo& info) {
  // There can be multiple breakpoints at the same address. Use the one with the largest ID since
  // it will be the one set most recently.
  int max_id = 0;
  const Breakpoint* bp = nullptr;

  for (const auto& weak_bp : info.hit_breakpoints) {
    if (!weak_bp || weak_bp->IsInternal())
      continue;

    int bp_id = IdForBreakpoint(weak_bp.get());
    if (bp_id > max_id) {
      max_id = bp_id;
      bp = weak_bp.get();
    }
  }

  if (bp)
    SetActiveBreakpoint(bp);
}

}  // namespace zxdb
