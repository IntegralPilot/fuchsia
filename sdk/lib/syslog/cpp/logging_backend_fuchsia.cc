// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <fidl/fuchsia.diagnostics.types/cpp/fidl.h>
#include <fidl/fuchsia.diagnostics/cpp/fidl.h>
#include <fidl/fuchsia.logger/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async-loop/loop.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/log_level.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/logging_backend_fuchsia_globals.h>
#include <lib/syslog/structured_backend/cpp/log_buffer.h>
#include <lib/syslog/structured_backend/cpp/raw_log_settings.h>
#include <lib/zx/channel.h>
#include <lib/zx/process.h>
#include <zircon/process.h>
#include <zircon/processargs.h>

#include <cstddef>
#include <fstream>
#include <iostream>
#include <sstream>
#include <variant>

#include "lib/component/incoming/cpp/protocol.h"
#include "lib/syslog/cpp/macros.h"
#if FUCHSIA_API_LEVEL_AT_LEAST(29)
#include "log_settings_internal.h"
#endif

namespace fuchsia_logging {
namespace {

#if FUCHSIA_API_LEVEL_AT_LEAST(27)
using FidlInterest = fuchsia_diagnostics_types::wire::Interest;
#else
using FidlInterest = fuchsia_diagnostics::wire::Interest;
#endif

}  // namespace

#if FUCHSIA_API_LEVEL_LESS_THAN(29)

class GlobalStateLock;

namespace internal {
class LogState {
 public:
  static void Set(const fuchsia_logging::LogSettings& settings, const GlobalStateLock& lock);

  fuchsia_logging::RawLogSeverity min_severity() const { return min_severity_; }

  const std::vector<std::string>& tags() const { return tags_; }

  // Allowed to be const because descriptor_ is mutable
  std::variant<zx::socket, std::ofstream>& descriptor() const { return logsink_socket_; }

 private:
  explicit LogState(const fuchsia_logging::LogSettings& settings);

  void Connect();

  void PollInterest();

  void HandleInterest(FidlInterest interest);

  fidl::WireSharedClient<fuchsia_logger::LogSink> log_sink_;
  void (*on_severity_changed_)(fuchsia_logging::RawLogSeverity severity);
  // Loop that never runs any code, but is needed so FIDL
  // doesn't crash if we have no dispatcher thread.
  async::Loop unused_loop_;
  std::atomic<fuchsia_logging::RawLogSeverity> min_severity_;
  const fuchsia_logging::RawLogSeverity default_severity_;
  mutable std::variant<zx::socket, std::ofstream> logsink_socket_ = zx::socket();
  std::vector<std::string> tags_;
  async_dispatcher_t* interest_listener_dispatcher_;
  fuchsia_logging::InterestListenerBehavior interest_listener_config_;
  // Handle to a fuchsia.logger.LogSink instance.
  std::optional<fidl::ClientEnd<fuchsia_logger::LogSink>> provided_log_sink_;
};
}  // namespace internal

// Global state lock. In order to mutate the LogState through SetStateLocked
// and GetStateLocked you must hold this capability.
// Do not directly use the C API. The C API exists solely
// to expose globals as a shared library.
// If the logger is not initialized, this will implicitly init the logger.
class GlobalStateLock {
 public:
  GlobalStateLock(bool autoinit = true) {
    internal::FuchsiaLogAcquireState();
    if (autoinit && !internal::FuchsiaLogGetStateLocked()) {
      internal::LogState::Set(fuchsia_logging::LogSettings(), *this);
    }
  }

  // Retrieves the global state
  internal::LogState* operator->() const { return internal::FuchsiaLogGetStateLocked(); }

  // Sets the global state
  void Set(internal::LogState* state) const { FuchsiaLogSetStateLocked(state); }

  // Retrieves the global state
  internal::LogState* operator*() const { return internal::FuchsiaLogGetStateLocked(); }

  ~GlobalStateLock() { internal::FuchsiaLogReleaseState(); }
};

#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)

namespace {

zx_koid_t ProcessSelfKoid() {
  zx_info_handle_basic_t info;
  // We need to use _zx_object_get_info to avoid breaking the driver ABI.
  // fake_ddk can fake out this method, which results in us deadlocking
  // when used in certain drivers because the fake doesn't properly pass-through
  // to the real syscall in this case.
  zx_status_t status = _zx_object_get_info(zx_process_self(), ZX_INFO_HANDLE_BASIC, &info,
                                           sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.koid : ZX_KOID_INVALID;
}

zx_koid_t globalPid = ProcessSelfKoid();
const char kTagFieldName[] = "tag";

#if FUCHSIA_API_LEVEL_AT_LEAST(29)

void FixDefaultSettings(RawLogSettings& settings, bool interest_listener_enabled) {
  // If no handle is provided, try the incoming namespace.
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
  if (settings.log_sink == ZX_HANDLE_INVALID) {
    // If there is no procargs handle, `log_sink` will be ZX_HANDLE_INVALID which will fallback to
    // the following `if` and connect to svc.
    settings.log_sink = zx_take_startup_handle(PA_LOG_SINK);
  }
#endif
  if (settings.log_sink == ZX_HANDLE_INVALID) {
    auto connect_result = component::Connect<fuchsia_logger::LogSink>();
    // If there is an error, we leave `log_sink` with ZX_HANDLE_INVALID which will result in a no-op
    // logger later.
    if (connect_result.is_ok()) {
      settings.log_sink = connect_result->TakeChannel().release();
    }
  }
  // If no dispatcher specified, try the default dispatcher.
  if (!settings.dispatcher && interest_listener_enabled) {
    settings.dispatcher = async_get_default_dispatcher();
  }
}

RawLogSettings GetDefaultSettings() {
  RawLogSettings settings;
  FixDefaultSettings(settings, true);
  return settings;
}

#endif

void BeginRecordInternal(LogBuffer* buffer, fuchsia_logging::RawLogSeverity severity,
                         std::optional<std::string_view> file_name, unsigned int line,
                         std::optional<std::string_view> msg,
                         std::optional<std::string_view> condition) {
  // Optional so no allocation overhead occurs if condition isn't set.
  std::optional<std::string> modified_msg;
  if (condition) {
    std::stringstream s;
    s << "Check failed: " << *condition << ". ";
    if (msg) {
      s << *msg;
    }
    modified_msg = s.str();
    msg = modified_msg->data();
  }
  buffer->BeginRecord(severity, file_name, line, msg, 0, globalPid,
                      internal::FuchsiaLogGetCurrentThreadKoid());
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  GlobalStateLock log_state;
  for (size_t i = 0; i < log_state->tags().size(); i++) {
    buffer->WriteKeyValue(kTagFieldName, log_state->tags()[i]);
  }
#else
  internal::FuchsiaLogForEachTag(
      internal::FuchsiaLogGetGlobalLogger(GetDefaultSettings), buffer,
      +[](void* context, const char* tag) {
        auto buffer = reinterpret_cast<LogBuffer*>(context);
        buffer->WriteKeyValue(kTagFieldName, tag);
      });
#endif
}

void BeginRecord(LogBuffer* buffer, fuchsia_logging::RawLogSeverity severity,
                 internal::NullSafeStringView file, unsigned int line,
                 internal::NullSafeStringView msg, internal::NullSafeStringView condition) {
  BeginRecordInternal(buffer, severity, file, line, msg, condition);
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  internal::SetFlushCallback(*buffer, [](cpp20::span<const uint8_t> data, FlushConfig config) {
    return internal::FlushToSocket(std::get<0>(GlobalStateLock()->descriptor()).get(), data,
                                   config);
  });
#endif
}

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
void BeginRecordWithSocket(LogBuffer* buffer, fuchsia_logging::RawLogSeverity severity,
                           internal::NullSafeStringView file_name, unsigned int line,
                           internal::NullSafeStringView msg, internal::NullSafeStringView condition,
                           zx_handle_t socket) {
  BeginRecordInternal(buffer, severity, file_name, line, msg, condition);
  internal::SetFlushCallback(*buffer,
                             [socket](cpp20::span<const uint8_t> data, FlushConfig config) {
                               return internal::FlushToSocket(socket, data, config);
                             });
}
#endif

void SetLogSettings(const fuchsia_logging::LogSettings& settings) {
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  GlobalStateLock lock(false);
  internal::LogState::Set(settings, lock);
#else
  const bool interest_listener_enabled =
      settings.interest_listener_config_ != InterestListenerBehavior::Disabled;
  internal::WithRawSettings(settings, [interest_listener_enabled](RawLogSettings& raw_settings) {
    FixDefaultSettings(raw_settings, interest_listener_enabled);
    internal::FuchsiaLogInitGlobalLogger(&raw_settings);
  });
#endif
}

}  // namespace

#if FUCHSIA_API_LEVEL_LESS_THAN(29)

void internal::LogState::PollInterest() {
  log_sink_->WaitForInterestChange().Then(
      [this](fidl::WireUnownedResult<fuchsia_logger::LogSink::WaitForInterestChange>&
                 interest_result) {
        // FIDL can cancel the operation if the logger is being reconfigured
        // which results in an error.
        if (!interest_result.ok()) {
          return;
        }
        HandleInterest(interest_result->value()->data);
        on_severity_changed_(min_severity_);
        PollInterest();
      });
}

void internal::LogState::HandleInterest(FidlInterest interest) {
  if (!interest.has_min_severity()) {
    min_severity_ = default_severity_;
  } else {
    min_severity_ = static_cast<fuchsia_logging::RawLogSeverity>(interest.min_severity());
  }
}

void internal::LogState::Connect() {
  auto default_dispatcher = async_get_default_dispatcher();
  bool missing_dispatcher = false;
  if (interest_listener_config_ != fuchsia_logging::Disabled) {
    if (!interest_listener_dispatcher_ && !default_dispatcher) {
      missing_dispatcher = true;
    }
  }
  // Regardless of whether or not we need to do anything async, FIDL async bindings
  // require a valid dispatcher or they panic.
  if (!interest_listener_dispatcher_) {
    if (default_dispatcher) {
      interest_listener_dispatcher_ = default_dispatcher;
    } else {
      interest_listener_dispatcher_ = unused_loop_.dispatcher();
    }
  }
  fidl::ClientEnd<fuchsia_logger::LogSink> client_end;
  if (!provided_log_sink_.has_value()) {
    auto connect_result = component::Connect<fuchsia_logger::LogSink>();
    if (connect_result.is_error()) {
      return;
    }
    client_end = std::move(connect_result.value());
  } else {
    client_end = std::move(*provided_log_sink_);
  }
  log_sink_.Bind(std::move(client_end), interest_listener_dispatcher_);
  if (interest_listener_config_ == fuchsia_logging::Enabled && !missing_dispatcher) {
    auto interest_result = log_sink_.sync()->WaitForInterestChange();
    if (!interest_result.ok()) {
      // Logging isn't available. Silently drop logs.
      return;
    }
    if (interest_result->is_ok()) {
      HandleInterest(interest_result->value()->data);
    }
    on_severity_changed_(min_severity_);
  }

  zx::socket local, remote;
  if (zx::socket::create(ZX_SOCKET_DATAGRAM, &local, &remote) != ZX_OK) {
    return;
  }
  if (!log_sink_->ConnectStructured(std::move(remote)).ok()) {
    return;
  }
  if (interest_listener_config_ != fuchsia_logging::Disabled) {
    PollInterest();
  }
  logsink_socket_ = std::move(local);
}

void internal::LogState::Set(const fuchsia_logging::LogSettings& settings,
                             const GlobalStateLock& lock) {
  auto old = *lock;
  lock.Set(new LogState(settings));
  if (old) {
    delete old;
  }
}

internal::LogState::LogState(const fuchsia_logging::LogSettings& settings)
    : unused_loop_(&kAsyncLoopConfigNeverAttachToThread),
      min_severity_(settings.min_log_level),
      default_severity_(settings.min_log_level) {
  interest_listener_dispatcher_ =
      static_cast<async_dispatcher_t*>(settings.single_threaded_dispatcher);
  interest_listener_config_ = settings.interest_listener_config_;
  min_severity_ = settings.min_log_level;
  if (settings.log_sink) {
    provided_log_sink_ = fidl::ClientEnd<fuchsia_logger::LogSink>(zx::channel(settings.log_sink));
  }
  on_severity_changed_ = settings.severity_change_callback;
  if (!on_severity_changed_) {
    on_severity_changed_ = [](fuchsia_logging::RawLogSeverity severity) {};
  }
  for (auto& tag : settings.tags) {
    tags_.push_back(tag);
  }
  Connect();
}

#else

zx::result<> FlushToGlobalLogger(LogBuffer& buffer) {
  cpp20::span<const uint8_t> data = buffer.EndRecord();
  return zx::make_result(internal::FuchsiaLogWrite(
      internal::FuchsiaLogGetGlobalLogger(GetDefaultSettings), data.data(), data.size()));
}

#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)

// Sets the default log severity. If not explicitly set,
// this defaults to INFO, or to the value specified by Archivist.
LogSettingsBuilder& LogSettingsBuilder::WithMinLogSeverity(
    fuchsia_logging::RawLogSeverity min_log_level) {
  settings_.min_log_level = min_log_level;
  return *this;
}

// Disables the interest listener.
LogSettingsBuilder& LogSettingsBuilder::DisableInterestListener() {
  WithInterestListenerConfiguration(fuchsia_logging::Disabled);
  return *this;
}

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
// Disables waiting for the initial interest from Archivist.
// The level specified in SetMinLogSeverity or INFO will be used
// as the default.
LogSettingsBuilder& LogSettingsBuilder::DisableWaitForInitialInterest() {
  if (settings_.interest_listener_config_ == fuchsia_logging::Enabled) {
    WithInterestListenerConfiguration(fuchsia_logging::EnabledNonBlocking);
  }
  return *this;
}

LogSettingsBuilder& LogSettingsBuilder::WithSeverityChangedListener(
    void (*callback)(fuchsia_logging::RawLogSeverity severity)) {
  settings_.severity_change_callback = callback;
  return *this;
}
#else
LogSettingsBuilder& LogSettingsBuilder::WithSeverityChangedListener(
    void* context, void (*callback)(void*, fuchsia_logging::RawLogSeverity severity)) {
  settings_.severity_change_callback = callback;
  settings_.severity_change_callback_context = context;
  return *this;
}
#endif

LogSettingsBuilder& LogSettingsBuilder::WithInterestListenerConfiguration(
    InterestListenerBehavior config) {
  settings_.interest_listener_config_ = config;
  return *this;
}
// Sets the log sink handle.
LogSettingsBuilder& LogSettingsBuilder::WithLogSink(zx_handle_t log_sink) {
  settings_.log_sink = log_sink;
  return *this;
}

// Sets the dispatcher to use.
LogSettingsBuilder& LogSettingsBuilder::WithDispatcher(async_dispatcher_t* dispatcher) {
  settings_.single_threaded_dispatcher = dispatcher;
  return *this;
}

LogSettingsBuilder& LogSettingsBuilder::WithTags(const std::initializer_list<std::string>& tags) {
  for (auto& tag : tags) {
    settings_.tags.push_back(tag);
  }
  return *this;
}

// Configures the log settings.
void LogSettingsBuilder::BuildAndInitialize() { SetLogSettings(settings_); }

#if FUCHSIA_API_LEVEL_LESS_THAN(29)
RawLogSeverity GetMinLogSeverity() { return GlobalStateLock()->min_severity(); }
#endif

LogBuffer LogBufferBuilder::Build() {
  LogBuffer buffer;
#if FUCHSIA_API_LEVEL_LESS_THAN(29)
  if (socket_) {
    fuchsia_logging::BeginRecordWithSocket(
        &buffer, severity_,
        fuchsia_logging::internal::NullSafeStringView::CreateFromOptional(file_name_), line_,
        fuchsia_logging::internal::NullSafeStringView::CreateFromOptional(msg_),
        fuchsia_logging::internal::NullSafeStringView::CreateFromOptional(condition_), socket_);
  } else
#endif
  {
    fuchsia_logging::BeginRecord(
        &buffer, severity_,
        fuchsia_logging::internal::NullSafeStringView::CreateFromOptional(file_name_), line_,
        fuchsia_logging::internal::NullSafeStringView::CreateFromOptional(msg_),
        fuchsia_logging::internal::NullSafeStringView::CreateFromOptional(condition_));
  }
  return buffer;
}

}  // namespace fuchsia_logging
