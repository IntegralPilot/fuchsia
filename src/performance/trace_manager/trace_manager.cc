// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace_manager/trace_manager.h"

#include <fidl/fuchsia.sysinfo/cpp/fidl.h>
#include <fidl/fuchsia.tracing.controller/cpp/fidl.h>
#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <fidl/fuchsia.tracing/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/clone.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <algorithm>
#include <iostream>
#include <set>

#include "src/performance/trace_manager/deferred_buffer_forwarder.h"

namespace tracing {
namespace {

// For large traces or when verbosity is on it can take awhile to write out
// all the records. E.g., cpuperf_provider can take 40 seconds with --verbose=2
constexpr zx::duration kStopTimeout = zx::sec(60);
constexpr uint32_t kMinBufferSizeMegabytes = 1;

// These defaults are copied from fuchsia.tracing/trace_controller.fidl.
constexpr uint32_t kDefaultBufferSizeMegabytesHint = 4;
constexpr zx::duration kDefaultStartTimeout{zx::msec(5000)};
constexpr fuchsia_tracing::BufferingMode kDefaultBufferingMode =
    fuchsia_tracing::BufferingMode::kOneshot;

constexpr size_t kMaxAlertQueueDepth = 16;

constexpr uint32_t kDefaultMajorVersion = 0;
constexpr uint32_t kDefaultMinorVersion = 1;

uint32_t ConstrainBufferSize(uint32_t buffer_size_megabytes) {
  return std::max(buffer_size_megabytes, kMinBufferSizeMegabytes);
}

struct KnownCategoryComparator {
  bool operator()(const fuchsia_tracing::KnownCategory& lhs,
                  const fuchsia_tracing::KnownCategory& rhs) const {
    if (lhs.name() != rhs.name()) {
      return lhs.name() < rhs.name();
    }
    return lhs.description() < rhs.description();
  }
};

using KnownCategorySet = std::set<fuchsia_tracing::KnownCategory, KnownCategoryComparator>;
using KnownCategoryVector = std::vector<fuchsia_tracing::KnownCategory>;

std::optional<std::string> GetBoardName() {
  zx::result client_end = component::Connect<fuchsia_sysinfo::SysInfo>();
  if (!client_end.is_ok()) {
    return std::nullopt;
  }
  fidl::SyncClient client{std::move(*client_end)};
  fidl::Result<fuchsia_sysinfo::SysInfo::GetBoardName> board_name_res = client->GetBoardName();
  if (board_name_res.is_error()) {
    return std::nullopt;
  }
  return board_name_res->name();
}

uint64_t sessionId = 0;

}  // namespace

TraceController::TraceController(std::unique_ptr<TraceSession> session,
                                 fidl::ServerEnd<fuchsia_tracing_controller::Session> server_end,
                                 async_dispatcher_t* dispatcher, fit::closure on_disconnect)
    : id_(sessionId++),
      session_{std::move(session)},
      binding_{dispatcher, std::move(server_end), this,
               [cb = std::move(on_disconnect)](fidl::UnbindInfo info) {
                 if (cb)
                   cb();
               }} {}

TraceController::~TraceController() = default;

// fidl
void TraceController::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_tracing_controller::Session> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FX_LOGS(WARNING) << "Received an unknown Session method with ordinal " << metadata.method_ordinal;
}

// fidl
void TraceController::StartTracing(StartTracingRequest& request,
                                   StartTracingCompleter::Sync& completer) {
  FX_LOGS(DEBUG) << "StartTracing";

  if (!session_) {
    FX_LOGS(ERROR) << "Ignoring start request, trace must be initialized first";
    completer.Reply(fit::error(fuchsia_tracing_controller::StartError::kNotInitialized));
    return;
  }

  switch (session_->state()) {
    case TraceSession::State::kStarting:
    case TraceSession::State::kStarted:
      FX_LOGS(ERROR) << "Ignoring start request, trace already started";
      completer.Reply(fit::error(fuchsia_tracing_controller::StartError::kAlreadyStarted));
      return;
    case TraceSession::State::kStopping:
      FX_LOGS(ERROR) << "Ignoring start request, trace stopping";
      completer.Reply(fit::error(fuchsia_tracing_controller::StartError::kStopping));
      return;
    case TraceSession::State::kTerminating:
      FX_LOGS(ERROR) << "Ignoring start request, trace terminating";
      completer.Reply(fit::error(fuchsia_tracing_controller::StartError::kTerminating));
      return;
    case TraceSession::State::kInitialized:
    case TraceSession::State::kStopped:
      break;
    default:
      FX_NOTREACHED();
      return;
  }

  std::vector<std::string> additional_categories;
  if (request.additional_categories()) {
    additional_categories = std::move(*request.additional_categories());
  }

  // This default matches trace's.
  fuchsia_tracing::BufferDisposition buffer_disposition =
      fuchsia_tracing::BufferDisposition::kRetain;
  if (request.buffer_disposition()) {
    buffer_disposition = *request.buffer_disposition();
    switch (buffer_disposition) {
      case fuchsia_tracing::BufferDisposition::kClearEntire:
      case fuchsia_tracing::BufferDisposition::kClearNondurable:
      case fuchsia_tracing::BufferDisposition::kRetain:
        break;
      default:
        FX_LOGS(ERROR) << "Bad value for buffer disposition: "
                       << static_cast<uint32_t>(buffer_disposition) << ", dropping connection";
        completer.Close(ZX_ERR_NOT_SUPPORTED);
        return;
    }
  }

  FX_LOGS(INFO) << "Starting trace, buffer disposition: " << buffer_disposition;

  session_->Start(buffer_disposition, additional_categories,
                  [completer = completer.ToAsync()](
                      fit::result<fuchsia_tracing_controller::StartError> result) mutable {
                    completer.Reply(result);
                  });
}

// fidl
void TraceController::StopTracing(StopTracingRequest& request,
                                  StopTracingCompleter::Sync& completer) {
  if (session_->state() != TraceSession::State::kInitialized &&
      session_->state() != TraceSession::State::kStarting &&
      session_->state() != TraceSession::State::kStarted) {
    FX_LOGS(INFO) << "Ignoring stop request, state != Initialized,Starting,Started";
    completer.Reply(fit::error(fuchsia_tracing_controller::StopError::kNotStarted));
    return;
  }

  bool write_results = request.write_results().value_or(false);

  FX_LOGS(INFO) << "Stopping trace" << (write_results ? ", and writing results" : "");
  session_->Stop(
      write_results,
      [completer = completer.ToAsync()](
          fit::result<fuchsia_tracing_controller::StopError, fuchsia_tracing_controller::StopResult>
              result) mutable { completer.Reply(std::move(result)); });
}

void TraceController::TerminateTracing(fit::closure cb) {
  if (!session_) {
    FX_LOGS(INFO) << "TerminateTracing called but session_ is already null";
    FX_DCHECK(cb);
    cb();
    return;
  }

  // Check the state first because the log messages are useful, but not if
  // tracing has ended.
  if (session_->state() == TraceSession::State::kTerminating) {
    FX_LOGS(INFO) << "Ignoring terminate request. Already terminating";
    return;
  }

  if (!write_results_on_terminate_) {
    session_->set_write_results_on_terminate(false);
  }

  session_->Terminate(std::move(cb));
}

TraceManager::TraceManager(Config config, async::Executor& executor)
    : config_(std::move(config)), executor_(executor) {
  provider_registry_bindings_.CreateHandler(this, executor.dispatcher(),
                                            fidl::kIgnoreBindingClosure);
}

TraceManager::~TraceManager() {
  // Ensure we destroy the trace controller first.
  trace_controller_.reset();
}

void TraceManager::CloseSession() { trace_controller_.reset(); }

void TraceManager::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_tracing_provider::Registry> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FX_LOGS(WARNING) << "Received an unknown Registry method with ordinal "
                   << metadata.method_ordinal;
}

void TraceManager::OnControllerDisconnect() {
  // While one controller could go away and another remain causing a trace
  // to not be terminated, at least handle the common case.
  FX_LOGS(INFO) << "Controller is gone";
  if (trace_controller_) {
    FX_LOGS(DEBUG) << "Terminating trace and closing session";
    // Terminate the running trace and the close the trace session.
    trace_controller_->TerminateTracing([this]() { CloseSession(); });
  }
}

// fidl
void TraceManager::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_tracing_controller::Provisioner> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FX_LOGS(WARNING) << "Received an unknown Provisioner method with ordinal "
                   << metadata.method_ordinal;
}

// fidl
void TraceManager::InitializeTracing(InitializeTracingRequest& request,
                                     InitializeTracingCompleter::Sync& completer) {
  FX_LOGS(DEBUG) << "InitializeTracing";

  // We only support once session at a time.
  if (trace_controller_) {
    request.controller().Close(ZX_ERR_ALREADY_EXISTS);
    FX_LOGS(ERROR) << "Ignoring initialize request, trace already initialized";
    return;
  }

  const auto& config = request.config();
  uint32_t default_buffer_size_megabytes = ConstrainBufferSize(
      config.buffer_size_megabytes_hint().value_or(kDefaultBufferSizeMegabytesHint));

  TraceProviderSpecMap provider_specs;
  if (config.provider_specs()) {
    for (const auto& it : *config.provider_specs()) {
      // Names are provided just for debugging diagnostic purposes. If there isn't one, we can just
      // skip it.
      if (!it.name()) {
        continue;
      }
      TraceProviderSpec provider_spec;
      if (it.buffer_size_megabytes_hint()) {
        provider_spec.buffer_size_megabytes = *it.buffer_size_megabytes_hint();
      }
      if (it.categories()) {
        provider_spec.categories = *it.categories();
      }
      provider_specs[*it.name()] = provider_spec;
    }
  }

  fuchsia_tracing::BufferingMode tracing_buffering_mode =
      config.buffering_mode().value_or(kDefaultBufferingMode);
  const char* mode_name;
  switch (tracing_buffering_mode) {
    case fuchsia_tracing::BufferingMode::kOneshot:
      mode_name = "oneshot";
      break;
    case fuchsia_tracing::BufferingMode::kCircular:
      mode_name = "circular";
      break;
    case fuchsia_tracing::BufferingMode::kStreaming:
      mode_name = "streaming";
      break;
    default:
      FX_LOGS(ERROR) << "Invalid buffering mode: " << static_cast<unsigned>(tracing_buffering_mode);
      return;
  }

  fuchsia_tracing_controller::FxtVersion fxt_version;
  if (config.version()) {
    if (config.version()->major()) {
      fxt_version.major(*config.version()->major());
    }
    if (config.version()->minor()) {
      fxt_version.minor(*config.version()->minor());
    }
  } else {
    fxt_version.major(kDefaultMajorVersion);
    fxt_version.minor(kDefaultMinorVersion);
  }

  FX_LOGS(INFO) << "Initializing trace with " << default_buffer_size_megabytes
                << " MB buffers, buffering mode=" << mode_name;
  if (provider_specs.size() > 0) {
    FX_LOGS(INFO) << "Provider overrides:";
    for (const auto& it : provider_specs) {
      FX_LOGS(INFO) << it.first << ": buffer size "
                    << it.second.buffer_size_megabytes.value_or(default_buffer_size_megabytes)
                    << " MB";
    }
  }

  std::vector<std::string> categories;
  if (config.categories()) {
    categories = *config.categories();
  }

  zx::duration start_timeout =
      zx::msec(static_cast<int64_t>(config.start_timeout_milliseconds().value_or(
          static_cast<uint64_t>(kDefaultStartTimeout.to_msecs()))));

  std::string board_name = GetBoardName().value_or("");

  DataForwarding forwarding = DataForwarding::kEager;
  // astro has very little disk space to use to buffer, so we're better of keeping the data in
  // memory.
  //
  // TODO(https://fxbug.dev/416067886): Currently, to determine whether to buffer our trace to disk
  // or not, we check that we're not currently running on astro, since astro has very little disk
  // space. We should generalize this check to say, check for how much storage is available and make
  // a decision based on that instead of just checking a specific hardcoded board name.
  if (config.defer_transfer().value_or(false) && board_name != "astro") {
    forwarding = DataForwarding::kBuffered;
  }

  std::shared_ptr<BufferForwarder> forwarder =
      forwarding == DataForwarding::kEager
          ? std::make_shared<BufferForwarder>(std::move(request.output()))
          : std::make_shared<DeferredBufferForwarder>(std::move(request.output()));

  auto session = std::make_unique<TraceSession>(
      executor_, std::move(forwarder), std::move(categories), default_buffer_size_megabytes,
      tracing_buffering_mode, std::move(provider_specs), start_timeout, kStopTimeout,
      std::move(fxt_version), [this]() { trace_controller_.reset(); },
      [this](const std::string& alert_name) {
        if (trace_controller_) {
          trace_controller_->OnAlert(alert_name);
        }
      });

  // The trace header is written now to ensure it appears first, and to avoid
  // timing issues if the trace is terminated early (and the session being
  // deleted).
  session->WriteTraceInfo();

  for (auto& variant : providers_) {
    std::visit([session = session.get()](auto& provider) { session->AddProvider(&provider); },
               variant);
  }

  trace_controller_.emplace(
      std::move(session), std::move(request.controller()), executor_.dispatcher(),
      [weak = weak_ptr_factory_.GetWeakPtr(), id = sessionId]() {
        if (weak && weak->trace_controller_.has_value() && id == weak->trace_controller_->Id()) {
          weak->OnControllerDisconnect();
        }
      });
}

// fidl
void TraceManager::GetProviders(GetProvidersCompleter::Sync& completer) {
  FX_LOGS(DEBUG) << "GetProviders";
  std::vector<fuchsia_tracing_controller::ProviderInfo> provider_info;
  for (const auto& variant : providers_) {
    std::visit(
        [&provider_info](const auto& provider) {
          fuchsia_tracing_controller::ProviderInfo info{
              {.id = provider.id, .pid = provider.pid, .name = provider.name}};
          provider_info.push_back(std::move(info));
        },
        variant);
  }
  completer.Reply({{.providers = std::move(provider_info)}});
}

// Allows multiple callers to race to call the same callback.
// The first caller will successfully have their value forwarded to the callback, and each
// subsequent call will be dropped. This allows a callback to race against a timeout to call a
// completer.
//
// The CompleterMerger is internally reference counted so that it may be passed by value as a
// callback to multiple callers
template <typename T>
class CompleterMerger {
 public:
  explicit CompleterMerger(fit::function<void(T)> completer)
      : state_(std::make_shared<State>(std::move(completer))) {}

  void operator()(T&& categories) const {
    bool expected = false;
    if (state_->called_.compare_exchange_weak(expected, true)) {
      state_->completer_(std::forward<T>(categories));
    }
  }

 private:
  struct State {
    explicit State(fit::function<void(T)> completer)
        : called_(false), completer_(std::move(completer)) {}
    std::atomic<bool> called_;
    fit::function<void(T)> completer_;
  };
  std::shared_ptr<State> state_;
};

// fidl
void TraceManager::GetKnownCategories(GetKnownCategoriesCompleter::Sync& completer) {
  FX_LOGS(DEBUG) << "GetKnownCategories";
  KnownCategorySet known_categories;
  for (const auto& [name, description] : config_.known_categories()) {
    known_categories.insert({{.name = {name}, .description = {description}}});
  }
  std::vector<fpromise::promise<KnownCategoryVector>> promises;
  fpromise::promise<> timeout = executor_.MakeDelayedPromise(zx::sec(1));
  for (const auto& variant : providers_) {
    fpromise::bridge<KnownCategoryVector> bridge;
    promises.push_back(bridge.consumer.promise());

    CompleterMerger<KnownCategoryVector> merger{bridge.completer.bind()};
    std::visit(
        [merger](const auto& provider) {
          provider.provider->GetKnownCategories().Then([merger](auto& result) {
            if (result.is_ok()) {
              merger(std::move(result->categories()));
            } else {
              merger({});
            }
          });
        },
        variant);
    timeout = fpromise::promise<>{timeout.and_then([merger = merger]() mutable { merger({}); })};
  }
  auto joined_promise =
      fpromise::join_promise_vector(std::move(promises))
          .and_then([completer = completer.ToAsync(),
                     known_categories = std::move(known_categories)](
                        std::vector<fpromise::result<KnownCategoryVector>>& results) mutable {
            for (const auto& result : results) {
              if (result.is_ok()) {
                const auto& result_known_categories = result.value();
                known_categories.insert(result_known_categories.begin(),
                                        result_known_categories.end());
              }
            }
            KnownCategoryVector final_categories{known_categories.begin(), known_categories.end()};
            completer.Reply({{.categories = std::move(final_categories)}});
          });

  executor_.schedule_task(std::move(joined_promise));
  executor_.schedule_task(std::move(timeout));
}

void TraceController::WatchAlert(WatchAlertCompleter::Sync& completer) {
  FX_LOGS(DEBUG) << "WatchAlert";
  if (alerts_.empty()) {
    watch_alert_completers_.push(completer.ToAsync());
  } else {
    completer.Reply({{.alert_name = std::move(alerts_.front())}});
    alerts_.pop();
  }
}

void TraceManager::RegisterProviderWorker(
    fidl::ClientEnd<fuchsia_tracing_provider::Provider> provider, uint64_t pid,
    const std::string& name) {
  FX_LOGS(DEBUG) << "Registering provider {" << pid << ":" << name << "}";
  auto it = providers_.emplace(providers_.end(), std::in_place_type<TraceProviderBundle>,
                               std::move(provider), next_provider_id_++, pid, name,
                               executor_.dispatcher());

  std::get<TraceProviderBundle>(*it).SetOnUnbound([this, it](fidl::UnbindInfo info) {
    auto& bundle = std::get<TraceProviderBundle>(*it);
    if (session()) {
      session()->RemoveDeadProvider(&bundle);
    }
    providers_.erase(it);
  });

  if (session()) {
    session()->AddProvider(&std::get<TraceProviderBundle>(*it));
  }
}

void TraceManager::RegisterProviderV2Worker(
    fidl::ClientEnd<fuchsia_tracing_provider::ProviderV2> provider, uint64_t pid,
    const std::string& name) {
  FX_LOGS(DEBUG) << "Registering provider V2 {" << pid << ":" << name << "}";
  auto it = providers_.emplace(providers_.end(), std::in_place_type<ProviderConnection>,
                               std::move(provider), next_provider_id_++, pid, name,
                               executor_.dispatcher());

  std::get<ProviderConnection>(*it).SetOnUnbound([this, it](fidl::UnbindInfo info) {
    auto& connection = std::get<ProviderConnection>(*it);
    if (session()) {
      session()->RemoveDeadV2Provider(&connection);
    }
    providers_.erase(it);
  });

  if (session()) {
    session()->AddProvider(&std::get<ProviderConnection>(*it));
  }
}

void TraceManager::RegisterV2(RegisterV2Request& request, RegisterV2Completer::Sync& completer) {
  RegisterProviderV2Worker(std::move(request.provider()), request.pid(), request.name());
}

void TraceManager::RegisterV2Synchronously(RegisterV2SynchronouslyRequest& request,
                                           RegisterV2SynchronouslyCompleter::Sync& completer) {
  RegisterProviderV2Worker(std::move(request.provider()), request.pid(), request.name());
  auto session_ptr = session();
  bool already_started = (session_ptr && (session_ptr->state() == TraceSession::State::kStarting ||
                                          session_ptr->state() == TraceSession::State::kStarted));
  completer.Reply(fit::ok(already_started));
}

void TraceController::FlushBuffers(FlushBuffersCompleter::Sync& completer) {
  FX_LOGS(DEBUG) << "FlushBuffers";
  if (!session_) {
    FX_LOGS(ERROR) << "Ignoring flush request, trace must be initialized first";
    completer.Reply(fit::error(fuchsia_tracing_controller::FlushError::kNotInitialized));
    return;
  }
  switch (session()->state()) {
    case TraceSession::State::kInitialized:
    case TraceSession::State::kStarting:
      completer.Reply(fit::error(fuchsia_tracing_controller::FlushError::kNotStarted));
      return;
    case TraceSession::State::kStopping:
    case TraceSession::State::kStopped:
    case TraceSession::State::kTerminating:
      completer.Reply(fit::error(fuchsia_tracing_controller::FlushError::kAborted));
      return;
    case TraceSession::State::kStarted:
      break;
  }
  session_->FlushProviders();
  completer.Reply(fit::ok());
}

// Deprecated, but we need to support the old apis until all supported api levels drop support for
// the api.
void TraceManager::RegisterProvider(RegisterProviderRequest& request,
                                    RegisterProviderCompleter::Sync& completer) {
  RegisterProviderWorker(std::move(request.provider()), request.pid(), request.name());
}

// Deprecated, but we need to support the old apis until all supported api levels drop support for
// the api.
void TraceManager::RegisterProviderSynchronously(
    RegisterProviderSynchronouslyRequest& request,
    RegisterProviderSynchronouslyCompleter::Sync& completer) {
  RegisterProviderWorker(std::move(request.provider()), request.pid(), request.name());
  auto session_ptr = session();
  bool already_started = (session_ptr && (session_ptr->state() == TraceSession::State::kStarting ||
                                          session_ptr->state() == TraceSession::State::kStarted));
  completer.Reply({ZX_OK, already_started});
}

void TraceController::SendSessionStateEvent(fuchsia_tracing_controller::SessionState state) {
  fit::result<fidl::Error> result = fidl::SendEvent(binding_)->OnSessionStateChange({state});
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to send OnSessionStateChange event: " << result.error_value();
  }
}

TraceSession* TraceManager::session() const {
  if (trace_controller_) {
    return trace_controller_->session();
  }
  return nullptr;
}

fuchsia_tracing_controller::SessionState TraceController::TranslateSessionState(
    TraceSession::State state) {
  switch (state) {
    case TraceSession::State::kInitialized:
      return fuchsia_tracing_controller::SessionState::kInitialized;
    case TraceSession::State::kStarting:
      return fuchsia_tracing_controller::SessionState::kStarting;
    case TraceSession::State::kStarted:
      return fuchsia_tracing_controller::SessionState::kStarted;
    case TraceSession::State::kStopping:
      return fuchsia_tracing_controller::SessionState::kStopping;
    case TraceSession::State::kStopped:
      return fuchsia_tracing_controller::SessionState::kStopped;
    case TraceSession::State::kTerminating:
      return fuchsia_tracing_controller::SessionState::kTerminating;
  }
}

void TraceController::OnAlert(const std::string& alert_name) {
  if (watch_alert_completers_.empty()) {
    if (alerts_.size() == kMaxAlertQueueDepth) {
      // We're at our queue depth limit. Discard the oldest alert.
      alerts_.pop();
    }

    alerts_.push(alert_name);
    return;
  }

  watch_alert_completers_.front().Reply({{.alert_name = alert_name}});
  watch_alert_completers_.pop();
}

}  // namespace tracing
