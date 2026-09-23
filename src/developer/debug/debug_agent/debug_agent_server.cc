// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/debug_agent_server.h"

#include <lib/fit/result.h>
#include <lib/stdcompat/functional.h>

#include <algorithm>

#include "fidl/fuchsia.debugger/cpp/natural_types.h"
#include "src/developer/debug/debug_agent/backtrace_utils.h"
#include "src/developer/debug/debug_agent/component_manager.h"
#include "src/developer/debug/debug_agent/debug_agent.h"
#include "src/developer/debug/debug_agent/debugged_process.h"
#include "src/developer/debug/debug_agent/debugged_thread.h"
#include "src/developer/debug/debug_agent/minidump_iterator.h"
#include "src/developer/debug/debug_agent/process_info_iterator.h"
#include "src/developer/debug/debug_agent/system_interface.h"
#include "src/developer/debug/ipc/filter_utils.h"
#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/shared/message_loop.h"

namespace debug_agent {

namespace {

// Process names are short, just 32 bytes, and fidl messages have 64k to work with. So we can
// include 2048 process names in a single message. Realistically, DebugAgent will never be attached
// to that many processes at once, so we don't need to hit the absolute limit.
constexpr size_t kMaxBatchedProcessNames = 1024;

class AttachedProcessIterator : public fidl::Server<fuchsia_debugger::AttachedProcessIterator> {
 public:
  explicit AttachedProcessIterator(fxl::WeakPtr<DebugAgent> debug_agent)
      : debug_agent_(std::move(debug_agent)) {}

  void GetNext(GetNextCompleter::Sync& completer) override {
    // First request, get the attached processes. This is unbounded, so we will always receive all
    // of the processes that DebugAgent is attached to.
    if (reply_.processes.empty()) {
      FX_CHECK(debug_agent_);

      debug_ipc::StatusRequest request;
      debug_agent_->OnStatus(request, &reply_);
      it_ = reply_.processes.begin();
    }

    std::vector<std::string> names;
    for (; it_ != reply_.processes.end() && names.size() < kMaxBatchedProcessNames; ++it_) {
      names.push_back(it_->process_name);
    }

    completer.Reply(fuchsia_debugger::AttachedProcessIteratorGetNextResponse{
        {.process_names = std::move(names)}});
  }

 private:
  fxl::WeakPtr<DebugAgent> debug_agent_;
  debug_ipc::StatusReply reply_ = {};
  std::vector<debug_ipc::ProcessRecord>::iterator it_;
};

// Converts a FIDL filter to a debug_ipc filter or a FilterError if there was an error. |id| is
// optional because sometimes callers want to construct a debug_ipc Filter without the intention of
// installing it to DebugAgent. If |id| is not given and the filter is installed, it will be
// impossible to derive the correct attach configuration from the filter.
debug::Result<debug_ipc::Filter, fuchsia_debugger::FilterError> ToDebugIpcFilter(
    const fuchsia_debugger::Filter& request, std::optional<uint32_t> id) {
  debug_ipc::Filter filter;

  if (request.pattern().empty()) {
    return fuchsia_debugger::FilterError::kNoPattern;
  }

  switch (request.type()) {
    case fuchsia_debugger::FilterType::kUrl:
      filter.type = debug_ipc::Filter::Type::kComponentUrl;
      break;
    case fuchsia_debugger::FilterType::kMoniker:
      filter.type = debug_ipc::Filter::Type::kComponentMoniker;
      break;
    case fuchsia_debugger::FilterType::kMonikerPrefix:
      filter.type = debug_ipc::Filter::Type::kComponentMonikerPrefix;
      break;
    case fuchsia_debugger::FilterType::kMonikerSuffix:
      filter.type = debug_ipc::Filter::Type::kComponentMonikerSuffix;
      break;
    default:
      return fuchsia_debugger::FilterError::kUnknownType;
  }

  filter.pattern = request.pattern();
  if (id) {
    filter.id = debug_ipc::Filter::Identifier(*id, debug_ipc::Filter::Originator::kFidlServer);

    // We should never overflow our 2^24 filter id allocation.
    FX_DCHECK(filter.id.Decode().originator == debug_ipc::Filter::Originator::kFidlServer)
        << "Filter ID created in debug_agent_server exceeded allocated filter IDs. Please file a "
           "bug: https://fxbug.dev/issues/new?component=1389559&template=1849567.";
  }

  if (!request.options().job_only()) {
    // Non-job-only filters are always weak when attached via this interface.
    filter.config.weak = true;
  } else {
    // Meanwhile, job-only filters are always strong (the default) when attached via this interface.
    filter.config.job_only = *request.options().job_only();
  }

  if (request.options().recursive()) {
    filter.config.recursive = *request.options().recursive();
  }

  return filter;
}

std::vector<fuchsia_debugger::Filter> ExpandRecursiveFilter(
    const fuchsia_debugger::Filter& filter, const ComponentManager& component_manager) {
  std::vector<fuchsia_debugger::Filter> filters;
  switch (filter.type()) {
    case fuchsia_debugger::FilterType::kUrl: {
      const auto& matches = component_manager.FindComponentInfoByUrl(filter.pattern());

      // Create a moniker prefix filter for each matching component.
      for (const auto& match : matches) {
        auto& moniker_filter = filters.emplace_back();
        moniker_filter.pattern(match.moniker);
        moniker_filter.type(fuchsia_debugger::FilterType::kMonikerPrefix);
      }
      break;
    }
    // TODO(https://fxbug.dev/454686467): Handle MonikerSuffix filter type here too.
    default: {
      // All other filter types are not modified.
      filters = {filter};
    }
  }

  return filters;
}
}  // namespace

// Static.
DebugAgentServer* DebugAgentServer::BindServer(
    async_dispatcher_t* dispatcher, fidl::ServerEnd<fuchsia_debugger::DebugAgent> server_end,
    fxl::WeakPtr<DebugAgent> debug_agent) {
  auto server = std::make_unique<DebugAgentServer>(debug_agent, dispatcher);
  auto impl_ptr = server.get();

  impl_ptr->binding_ref_ =
      fidl::BindServer(dispatcher, std::move(server_end), std::move(server),
                       cpp20::bind_front(&debug_agent::DebugAgentServer::OnUnboundFn, impl_ptr));

  return impl_ptr;
}

DebugAgentServer::DebugAgentServer(fxl::WeakPtr<DebugAgent> agent, async_dispatcher_t* dispatcher)
    : debug_agent_(std::move(agent)), dispatcher_(dispatcher) {
  debug_agent_->AddObserver(this);
}

void DebugAgentServer::GetAttachedProcesses(GetAttachedProcessesRequest& request,
                                            GetAttachedProcessesCompleter::Sync& completer) {
  FX_CHECK(debug_agent_);

  // Create and bind the iterator.
  fidl::BindServer(
      dispatcher_,
      fidl::ServerEnd<fuchsia_debugger::AttachedProcessIterator>(std::move(request.iterator())),
      std::make_unique<AttachedProcessIterator>(debug_agent_->GetWeakPtr()), nullptr);
}

void DebugAgentServer::Connect(ConnectRequest& request, ConnectCompleter::Sync& completer) {
  FX_CHECK(debug_agent_);

  if (debug_agent_->is_connected()) {
    completer.Reply(zx::make_result(ZX_ERR_ALREADY_BOUND));
    return;
  }

  auto buffered_socket = std::make_unique<debug::BufferedZxSocket>(std::move(request.socket()));

  // Hand ownership of the socket to DebugAgent and start listening.
  debug_agent_->TakeAndConnectRemoteAPIStream(std::move(buffered_socket));

  completer.Reply(zx::make_result(ZX_OK));
}

void DebugAgentServer::AttachTo(AttachToRequest& request, AttachToCompleter::Sync& completer) {
  FX_DCHECK(debug_agent_);

  auto result = AddFilter(request);
  if (result.has_error()) {
    completer.Reply(fit::error(result.err()));
    return;
  }

  auto reply = result.take_value();

  completer.Reply(fit::success(AttachToFilterMatches(reply.matched_processes_for_filter)));
}

debug_ipc::UpdateFilterReply DebugAgentServer::SynchronizeDebugIpcFilters() {
  // OnUpdateFilter will clear all the filters before reinstalling the set that is present in the
  // IPC request, so we must be sure to copy all of the filters that were already there before
  // calling the method.
  std::vector<debug_ipc::Filter> update_filters;
  update_filters.reserve(debug_agent_->GetIpcFilters().size());
  std::ranges::transform(
      debug_agent_->GetIpcFilters(), std::back_inserter(update_filters),
      [](const debug_ipc::Filter* filter) -> debug_ipc::Filter { return *filter; });

  // Add any of our filters that are not already present in |update_filters|.
  std::ranges::for_each(filters_, [this, &update_filters](const auto& elem) {
    if (!debug_ipc::GetFilterForId(debug_agent_->GetIpcFilters(), elem.second.id)) {
      update_filters.push_back(elem.second);
    }
  });

  debug_ipc::UpdateFilterRequest ipc_request{.filters = std::move(update_filters)};
  debug_ipc::UpdateFilterReply reply;
  debug_agent_->OnUpdateFilter(ipc_request, &reply);

  return reply;
}

void DebugAgentServer::AddDebugIpcFilter(const debug_ipc::Filter& filter) {
  filters_[filter.id] = filter;
}

DebugAgentServer::AddFilterResult DebugAgentServer::AddFilter(
    const fuchsia_debugger::Filter& fidl_filter) {
  auto result = ToDebugIpcFilter(fidl_filter, debug_ipc::GenerateFilterIdValue());
  if (result.has_error()) {
    return result.err();
  }

  const auto& new_filter = result.value();

  AddDebugIpcFilter(new_filter);
  return SynchronizeDebugIpcFilters();
}

uint32_t DebugAgentServer::AttachToFilterMatches(
    const std::vector<debug_ipc::FilterMatch>& filter_matches) const {
  // This is not a size_t because this count is eventually fed back through a FIDL type, which
  // does not have support for size types.
  uint32_t attaches = 0;

  auto pids_to_attach =
      debug_ipc::GetAttachConfigsForFilterMatches(filter_matches, debug_agent_->GetIpcFilters());

  for (const auto& [koid, attach_config] : pids_to_attach) {
    auto status = AttachWithConfig(koid, attach_config);

    // We may get an error if we're already attached to this process, or in the case of job-only,
    // attached to an ancestor. DebugAgent already prints a trace log for this, and it's not a
    // problem for clients if we're already attached to what they care about, so this case is
    // ignored.
    if (status.has_error() && status.type() != debug::Status::Type::kAlreadyExists) {
      DEBUG_LOG(Agent) << " attach to koid " << koid << " failed: " << status.message();
    } else {
      // Normal case where we attached to something.
      attaches++;
    }
  }

  return attaches;
}

debug::Status DebugAgentServer::AttachWithConfig(zx_koid_t koid,
                                                 const debug_ipc::AttachConfig& config) const {
  debug_ipc::AttachRequest request;
  request.koid = koid;
  request.config = config;

  debug_ipc::AttachReply reply;
  debug_agent_->OnAttach(request, &reply);

  return reply.status;
}

void DebugAgentServer::OnNotification(const debug_ipc::NotifyProcessStarting& notify) {
  // Ignore launching notifications.
  if (notify.type == debug_ipc::NotifyProcessStarting::Type::kLaunch) {
    return;
  }

  // We only get process starting notifications (as a debug_ipc client) when a filter matches. We
  // also only get this notification for processes specifically, so the koid is always a process.
  // When we matched a job_only filter, we create a DebuggedProcess object for it internally, and
  // don't need an explicit attach.
  if (!debug_agent_->GetDebuggedProcess(notify.koid)) {
    bool have_any_matching_filter = std::ranges::any_of(
        notify.filter_ids,
        [&](const auto& filter_id) -> bool { return filters_.contains(filter_id); });

    // Only issue the attach if one of the matching filters was ours.
    if (have_any_matching_filter) {
      AttachWithConfig(notify.koid, notify.attach_config);
    }
  }
}

void DebugAgentServer::OnNotification(const debug_ipc::NotifyException& notify) {
  // We always destruct ourselves whenever the client hangs up.
  FX_DCHECK(binding_ref_);

  // The thread is in an exception, we don't need to suspend it, but we do need
  // to resume it when we're done (if there isn't a debug_ipc client).
  auto thread = debug_agent_->GetDebuggedThread(notify.thread.id);
  if (!thread) {
    FX_LOGS(WARNING) << "Got exception notification for unknown thread " << notify.thread.id.thread;
    return;
  }

  fuchsia_debugger::DebugAgentOnFatalExceptionRequest event;

  event.thread(notify.thread.id.thread);
  event.backtrace(
      GetBacktraceMarkupForThread(thread->process()->process_handle(), thread->thread_handle()));

  fit::result result = fidl::SendEvent(*binding_ref_)->OnFatalException(event);
  if (!result.is_ok()) {
    FX_LOGS(WARNING) << "Error sending event: " << result.error_value();
  }

  // Asynchronously detach from the process so the system can handle the exception as normal if
  // there is no debug_ipc client. This must be asynchronous so that the low level exception handler
  // doesn't have the process removed out from under it when we should be synchronously handling the
  // exception.
  //
  // |this| is owned by the async dispatcher associated with this message loop, so it's safe to
  // capture. Similarly, DebugAgent is allocated in main, so our reference should also always be
  // valid here. The thread and process might exit independently before the message loop runs this
  // callback, so we capture the process's koid by value first.
  debug::MessageLoop::Current()->PostTask(
      FROM_HERE, [=, this, process_koid = thread->process()->koid()]() {
        FX_DCHECK(this);
        FX_DCHECK(debug_agent_);

        // The check for the DebuggedProcess isn't strictly necessary, but prevents some log spam
        // in the case that the process has already gone away after the exception was released. We
        // want the log to be forwarded to debug_ipc clients so we'll check here to avoid the error
        // case when this is more likely to happen.
        //
        // TODO(https://fxbug.dev/377671670): Write better tests for this.
        if (!debug_agent_->is_connected() && debug_agent_->GetDebuggedProcess(process_koid)) {
          debug_ipc::DetachRequest request;
          request.koid = process_koid;
          debug_ipc::DetachReply reply;
          debug_agent_->OnDetach(request, &reply);

          if (reply.status.has_error()) {
            FX_LOGS(WARNING) << "Failed to detach from process " << process_koid << ": "
                             << reply.status.message();
          }
        }
      });
}

void DebugAgentServer::OnNotification(const debug_ipc::NotifyComponentStarting& notify) {
  std::vector<debug_ipc::FilterMatch> filter_matches;
  for (const auto& match : notify.matching_filters) {
    // There will only ever be one koid per match.
    FX_DCHECK(match.matches.size() == 1);

    if (match.matches[0].koid != ZX_KOID_INVALID) {
      filter_matches.emplace_back(match);
    }
  }

  // NOTE: (as of IPC version 73) we ignore the notification's optional returned filter. In IPC
  // versions prior to 73, the value was (perhaps incorrectly) ignored. Since our supported version
  // is synchronized with DebugAgent's, we only need to add support for |NotifyFilterCreated|.

  AttachToFilterMatches(filter_matches);
}

void DebugAgentServer::OnNotification(const debug_ipc::NotifyFilterCreated& notify) {
  if (notify.originating_filter_id.Decode().originator ==
      debug_ipc::Filter::Originator::kFidlServer) {
    AddDebugIpcFilter(notify.filter);
    if (!notify.participated_in_matching) {
      // This is important to send on the message loop instead of re-entrantly. This notification
      // can come from the component starting sequence, which potentially needs to both send this
      // notification as well as send a component starting notification. If we immediately install
      // this filter by reentrantly calling OnUpdateFilter, then we'll unintentionally deallocate
      // the filters the DebugAgent is currently trying to match against and send to clients via the
      // FilterMatch instances in NotifyComponentStarting.
      debug::MessageLoop::Current()->PostTask(FROM_HERE, [this]() {
        AttachToFilterMatches(SynchronizeDebugIpcFilters().matched_processes_for_filter);
      });
    }
  }

  // Ignore new filters that were not originated by us.
}

DebugAgentServer::GetMatchingProcessesResult DebugAgentServer::GetMatchingProcesses(
    std::optional<fuchsia_debugger::Filter> filter) const {
  FX_DCHECK(debug_agent_);

  std::vector<DebuggedProcess*> processes;
  const auto& attached_processes = debug_agent_->GetAllProcesses();

  std::vector<fuchsia_debugger::Filter> filters;
  if (filter) {
    if (filter->options().recursive()) {
      filters =
          ExpandRecursiveFilter(*filter, debug_agent_->system_interface().GetComponentManager());
    } else {
      filters = {*filter};
    }
  } else {
    for (const auto& [_koid, process] : attached_processes) {
      processes.push_back(process.get());
    }

    return processes;
  }

  for (const auto& filter : filters) {
    // We're not installing this filter, so don't need to provide a filter id.
    auto result = ToDebugIpcFilter(filter, std::nullopt);
    if (result.has_error()) {
      return result.err();
    }

    const auto& component_manager = debug_agent_->system_interface().GetComponentManager();

    for (const auto& [_koid, process] : attached_processes) {
      const auto& components = component_manager.FindComponentInfo(process->process_handle());

      if (debug_ipc::FilterMatches(result.value(), process->process_handle().GetName(),
                                   components)) {
        processes.push_back(process.get());
      }
    }
  }

  return processes;
}

void DebugAgentServer::GetProcessInfo(GetProcessInfoRequest& request,
                                      GetProcessInfoCompleter::Sync& completer) {
  FX_DCHECK(debug_agent_);

  auto result = GetMatchingProcesses(request.options().filter());
  if (result.has_error()) {
    return completer.Reply(fit::error(result.err()));
  }

  // At this point it is invalid to have either filtered out all of the attached processes (or be
  // attached to nothing). The first GetNext call on the iterator will produce this error, which is
  // more appropriate than for this method.
  fidl::BindServer(
      dispatcher_,
      fidl::ServerEnd<fuchsia_debugger::ProcessInfoIterator>(std::move(request.iterator())),
      std::make_unique<ProcessInfoIterator>(debug_agent_, result.take_value(),
                                            std::move(request.options().interest())));

  completer.Reply(fit::success());
}

void DebugAgentServer::GetMinidumps(GetMinidumpsRequest& request,
                                    GetMinidumpsCompleter::Sync& completer) {
  auto result = GetMatchingProcesses(request.options().filter());
  if (result.has_error()) {
    completer.Reply(fit::error(result.err()));
    return;
  }

  fidl::BindServer(
      dispatcher_,
      fidl::ServerEnd<fuchsia_debugger::MinidumpIterator>(std::move(request.iterator())),
      std::make_unique<MinidumpIterator>(debug_agent_, result.take_value()));

  completer.Reply(fit::success());
}

void DebugAgentServer::OnUnboundFn(DebugAgentServer* impl, fidl::UnbindInfo info,
                                   fidl::ServerEnd<fuchsia_debugger::DebugAgent> server_end) {
  // DebugAgent will be destructed before the server bound to the outgoing directory, so it is
  // possible for DebugAgent to be null here.
  if (!debug_agent_)
    return;

  debug_agent_->RemoveObserver(this);
}

void DebugAgentServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_debugger::DebugAgent> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FX_LOGS(WARNING) << "Unknown method: " << metadata.method_ordinal;
}

}  // namespace debug_agent
