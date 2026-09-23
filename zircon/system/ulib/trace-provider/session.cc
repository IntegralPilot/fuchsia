// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "session.h"

#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <inttypes.h>
#include <lib/async/cpp/task.h>
#include <lib/fit/defer.h>
#include <lib/trace-provider/provider.h>
#include <lib/zx/process.h>
#include <lib/zx/vmar.h>
#include <stdio.h>
#include <zircon/assert.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <string>
#include <utility>

#include "utils.h"

namespace trace {
namespace internal {

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
Session::Session(void* buffer, size_t buffer_num_bytes, std::vector<std::string> categories,
                 fidl::ServerBindingRef<fuchsia_tracing_provider::ProviderV2> binding)
    : buffer_(buffer),
      buffer_num_bytes_(buffer_num_bytes),
      binding_(std::move(binding)),
      enabled_categories_(std::move(categories)) {}
#else
Session::Session(async_dispatcher_t* dispatcher, void* buffer, size_t buffer_num_bytes,
                 zx::fifo fifo, std::vector<std::string> categories)
    : buffer_(buffer),
      buffer_num_bytes_(buffer_num_bytes),
      dispatcher_(dispatcher),
      fifo_(std::move(fifo)),
      fifo_wait_(this, fifo_.get(), ZX_FIFO_READABLE | ZX_FIFO_PEER_CLOSED),
      enabled_categories_(std::move(categories)) {}
#endif

Session::~Session() {
  std::scoped_lock<std::mutex> lock{mutex_};
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  // If there are uncalled callbacks, they may hold FIDL callbacks which are an error
  // to drop without first replying to.
  if (start_cb_) {
    start_cb_();
  }
  if (stop_cb_) {
    stop_cb_();
  }
  if (terminate_cb_) {
    terminate_cb_();
  }
#endif
  zx_status_t status =
      zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(buffer_), buffer_num_bytes_);
  ZX_DEBUG_ASSERT(status == ZX_OK);
#if FUCHSIA_API_LEVEL_LESS_THAN(31)
  status = fifo_wait_.Cancel();
  ZX_DEBUG_ASSERT(status == ZX_OK || status == ZX_ERR_NOT_FOUND);
#endif
}

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
std::unique_ptr<Session> Session::InitializeEngine(
    async_dispatcher_t* dispatcher, trace_buffering_mode_t buffering_mode, zx::vmo buffer,
    std::vector<std::string> categories,
    fidl::ServerBindingRef<fuchsia_tracing_provider::ProviderV2> binding) {
#else
void Session::InitializeEngine(async_dispatcher_t* dispatcher,
                               trace_buffering_mode_t buffering_mode, zx::vmo buffer, zx::fifo fifo,
                               std::vector<std::string> categories) {
#endif
  ZX_DEBUG_ASSERT(buffer);
#if FUCHSIA_API_LEVEL_LESS_THAN(31)
  ZX_DEBUG_ASSERT(fifo);
#endif

  // If the engine isn't stopped flag an error. No one else should be
  // starting/stopping the engine so testing this here is ok.
  switch (trace_state()) {
    case TRACE_STOPPED:
      break;
    case TRACE_STOPPING:
      fprintf(stderr,
              "Session for process %" PRIu64
              ": cannot initialize engine, still stopping from previous trace\n",
              GetPid());
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
      return nullptr;
#else
      return;
#endif
    case TRACE_STARTED:
      // We can get here if the app errantly tried to create two providers.
      // This is a bug in the app, provide extra assistance for diagnosis.
      // Including the pid here has been extraordinarily helpful.
      fprintf(stderr,
              "Session for process %" PRIu64
              ": engine is already initialized. Is there perchance two"
              " providers in this app?\n",
              GetPid());
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
      return nullptr;
#else
      return;
#endif
    default:
      __UNREACHABLE;
  }

  uint64_t buffer_num_bytes;
  zx_status_t status = buffer.get_size(&buffer_num_bytes);
  if (status != ZX_OK) {
    fprintf(stderr, "Session: error getting buffer size, status=%d(%s)\n", status,
            zx_status_get_string(status));
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
    return nullptr;
#else
    return;
#endif
  }

  uintptr_t buffer_ptr;
  status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0u, buffer, 0u,
                                      buffer_num_bytes, &buffer_ptr);
  if (status != ZX_OK) {
    fprintf(stderr, "Session: error mapping buffer, status=%d(%s)\n", status,
            zx_status_get_string(status));
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
    return nullptr;
#else
    return;
#endif
  }

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  auto session =
      std::unique_ptr<Session>(new Session(reinterpret_cast<void*>(buffer_ptr), buffer_num_bytes,
                                           std::move(categories), std::move(binding)));
  status = trace_engine_initialize(dispatcher, session.get(), buffering_mode, session->buffer_,
                                   session->buffer_num_bytes_);
#else
  auto session = new Session(dispatcher, reinterpret_cast<void*>(buffer_ptr), buffer_num_bytes,
                             std::move(fifo), std::move(categories));

  status = session->fifo_wait_.Begin(dispatcher);
  if (status != ZX_OK) {
    fprintf(stderr, "Session: error starting fifo wait, status=%d(%s)\n", status,
            zx_status_get_string(status));
    delete session;
    return;
  }

  status = trace_engine_initialize(dispatcher, session, buffering_mode, session->buffer_,
                                   session->buffer_num_bytes_);
#endif

  if (status != ZX_OK) {
    fprintf(stderr, "Session: error starting engine, status=%d(%s)\n", status,
            zx_status_get_string(status));
#if FUCHSIA_API_LEVEL_LESS_THAN(31)
    delete session;
#endif
  }

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  return session;
#else
  // The session will be destroyed in |TraceTerminated()|.
#endif
}

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
void Session::StartEngine(trace_start_mode_t start_mode, fit::callback<void()> cb) {
  std::scoped_lock<std::mutex> lock{mutex_};
  auto d = fit::defer([&cb]() { cb(); });
  if (start_cb_) {
    // Already a start request in flight, our contract is to ignore repeat calls.
    return;
  }
#else
void Session::StartEngine(trace_start_mode_t start_mode) {
#endif
  // If the engine isn't stopped flag an error. No one else should be
  // starting/stopping the engine so testing this here is ok.
  switch (trace_state()) {
    case TRACE_STOPPED:
      break;
    case TRACE_STOPPING:
      fprintf(stderr,
              "Session for process %" PRIu64
              ": cannot start engine, still stopping from previous trace\n",
              GetPid());
      return;
    case TRACE_STARTED:
      // Ignore.
      return;
    default:
      __UNREACHABLE;
  }

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  start_cb_ = std::move(cb);
  d.cancel();
#endif
  zx_status_t status = trace_engine_start(start_mode);
  if (status != ZX_OK) {
    // There's nothing more we can do here. There's currently no easy way
    // to inform trace-manager of the error because we don't have a copy
    // of "this", we're a static method and we give ownership of "this" to
    // the trace engine. When the trace-engine wants to invoke one of our
    // methods it does so via the "handler" API. But we're not in the
    // engine here. Fortunately there's nothing more we need to do here.
    // The kinds of errors we can get here fall into three categories:
    // 1) Can't happen: If it can't happen then just let trace-manager
    //    timeout waiting for us to start.
    // 2) To be ignored: The FIDL provider protocol specifies that if a
    //    Start request is received when the engine is not stopped then the
    //    request is to be ignored.
    // 3) Async loop shutting down: If the provider is shutting down its
    //    async loop then the engine is about to be terminated anyway.
    //
    // Just log the error for debugging purposes.
    // Ignore BAD_STATE as that is case #2.
    if (status != ZX_ERR_BAD_STATE) {
      fprintf(stderr, "Session: error starting engine, status=%d(%s)\n", status,
              zx_status_get_string(status));
    }
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
    start_cb_ = nullptr;
#endif
    return;
  }
}

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
void Session::StopEngine(fit::callback<void()> cb) {
  std::scoped_lock<std::mutex> lock{mutex_};
  if (stop_cb_) {
    // Already a stop request in flight, our contract is to ignore repeat calls.
    cb();
    return;
  }
  stop_cb_ = std::move(cb);
  trace_engine_stop(ZX_OK);
}
#else
void Session::StopEngine() { trace_engine_stop(ZX_OK); }
#endif

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
void Session::TerminateEngine(fit::callback<void()> cb) {
  bool already_terminating = false;
  {
    std::scoped_lock<std::mutex> lock{mutex_};
    if (terminate_cb_) {
      already_terminating = true;
    } else {
      terminate_cb_ = std::move(cb);
    }
  }
  if (already_terminating) {
    // Already a terminate request in flight, our contract is to ignore repeat calls.
    cb();
  }
  trace_engine_terminate();
}
#else
void Session::TerminateEngine() { trace_engine_terminate(); }
#endif

#if FUCHSIA_API_LEVEL_LESS_THAN(31)
void Session::HandleFifo(async_dispatcher_t* dispatcher, async::WaitBase* wait, zx_status_t status,
                         const zx_packet_signal_t* signal) {
  if (status == ZX_ERR_CANCELED) {
    // The wait could be canceled if we're shutting down, e.g., the
    // program is exiting.
    return;
  }

  if (status != ZX_OK) {
    fprintf(stderr, "Session: FIFO wait failed: status=%d(%s)\n", status,
            zx_status_get_string(status));
  } else if (signal->observed & ZX_FIFO_READABLE) {
    if (ReadFifoMessage()) {
      zx_status_t status = wait->Begin(dispatcher);
      if (status == ZX_OK) {
        return;
      }
      fprintf(stderr, "Session: Error re-registering FIFO wait: status=%d(%s)\n", status,
              zx_status_get_string(status));
    }
  } else {
    ZX_DEBUG_ASSERT(signal->observed & ZX_FIFO_PEER_CLOSED);
  }

  // TraceManager is gone or other error with the fifo.
  TerminateEngine();
}

bool Session::ReadFifoMessage() {
  trace_provider_packet_t packet;
  auto status = fifo_.read(sizeof(packet), &packet, 1u, nullptr);
  ZX_DEBUG_ASSERT(status == ZX_OK);
  if (packet.data16 != 0) {
    fprintf(stderr, "Session: data16 field non-zero from TraceManager: %u\n", packet.data16);
    return false;
  }
  switch (packet.request) {
    case TRACE_PROVIDER_BUFFER_SAVED: {
      auto wrapped_count = packet.data32;
      auto durable_data_end = packet.data64;
      status = MarkBufferSaved(wrapped_count, durable_data_end);
      if (status == ZX_ERR_BAD_STATE) {
        // This happens when tracing has stopped. Ignore it.
      } else if (status != ZX_OK) {
        fprintf(stderr, "Session: MarkBufferSaved failed: status=%d\n", status);
        return false;
      }
      break;
    }
    default:
      fprintf(stderr, "Session: Bad request from TraceManager: %u\n", packet.request);
      return false;
  }

  return true;
}

#endif

zx_status_t Session::MarkBufferSaved(uint32_t wrapped_count, uint64_t durable_data_end) {
  return trace_engine_mark_buffer_saved(wrapped_count, durable_data_end);
}

bool DoesCategoryMatch(const std::string& category, const std::string& match_string) {
  if (match_string.empty())
    return false;
  if (match_string.back() != '*')
    return category == match_string;

  const auto prefix_size = match_string.length() - 1;
  return category.compare(0, prefix_size, match_string, 0, prefix_size) == 0;
}

bool Session::IsCategoryEnabled(const char* category) {
  std::scoped_lock<std::mutex> lock{mutex_};
  if (enabled_categories_.size() == 0) {
    // If none are specified, enable all categories.
    return true;
  }
  for (const auto& enabled_category : enabled_categories_) {
    if (DoesCategoryMatch(category, enabled_category))
      return true;
  }
  return false;
}

#if FUCHSIA_API_LEVEL_LESS_THAN(31)
void Session::SendFifoPacket(const trace_provider_packet_t* packet) {
  auto status = fifo_.write(sizeof(*packet), packet, 1, nullptr);
  ZX_DEBUG_ASSERT(status == ZX_OK || status == ZX_ERR_PEER_CLOSED);
}
#endif

void Session::TraceStarted() {
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  std::scoped_lock<std::mutex> lock{mutex_};
  // Unlike stopping or terminating which may occur on trace-engine's behest, starting should only
  // occur if we ask for it.
  ZX_DEBUG_ASSERT(start_cb_);
  start_cb_();
#else
  trace_provider_packet_t packet{};
  packet.request = TRACE_PROVIDER_STARTED;
  packet.data32 = TRACE_PROVIDER_FIFO_PROTOCOL_VERSION;
  SendFifoPacket(&packet);
#endif
}

void Session::TraceStopped(zx_status_t disposition) {
  std::scoped_lock<std::mutex> lock{mutex_};
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  if (stop_cb_) {
    stop_cb_();
  }
#else
  trace_provider_packet_t packet{};
  packet.request = TRACE_PROVIDER_STOPPED;
  SendFifoPacket(&packet);
#endif
}

void Session::TraceTerminated() {
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  fit::callback<void()> cb = nullptr;
  {
    std::scoped_lock<std::mutex> lock{mutex_};
    if (terminate_cb_) {
      // this terminate_cb_ owns `this` and will destroy us. Do not reference `this` after
      // calling it.
      cb = std::move(terminate_cb_);
    }
  }
  // Call cb without holding the lock
  if (cb) {
    cb();
  }
#else
  std::scoped_lock<std::mutex> lock{mutex_};
  // Destruction can race with HandleFifo, e.g., if the dispatcher runs on a background thread
  // and tracing terminates on a different thread. Handle this by running the destructor on the
  // dispatcher's thread (which we assume is single-threaded). It may also happen that the task
  // is not run. This can happen if the loop is quit and destructed before the task is run.
  // Handle this by letting destruction of the closure delete the session.
  std::unique_ptr<Session> session{this};
  async::PostTask(dispatcher_, [session = std::move(session)]() {});
#endif
}

void Session::NotifyBufferFull(uint32_t wrapped_count, uint64_t durable_data_end) {
  std::scoped_lock<std::mutex> lock{mutex_};
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  auto result = fidl::SendEvent(binding_)->OnSaveBuffer(
      {{.wrapped_count = wrapped_count, .durable_data_end = durable_data_end}});
  if (result.is_error()) {
    fprintf(stderr, "Session: NotifyBufferFull failed: %s\n",
            zx_status_get_string(result.error_value().status()));
  }
#else
  trace_provider_packet_t packet{};
  packet.request = TRACE_PROVIDER_SAVE_BUFFER;
  packet.data32 = wrapped_count;
  packet.data64 = durable_data_end;
  auto status = fifo_.write(sizeof(packet), &packet, 1, nullptr);
  // There's something wrong in our protocol or implementation if we fill
  // the fifo buffer.
  ZX_DEBUG_ASSERT(status == ZX_OK || status == ZX_ERR_PEER_CLOSED);
#endif
}

void Session::SendAlert(const char* alert_name) {
  std::scoped_lock<std::mutex> lock{mutex_};
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  auto result = fidl::SendEvent(binding_)->OnAlert({{.name = alert_name}});
  if (result.is_error()) {
    fprintf(stderr, "Session: SendAlert failed: %s\n",
            zx_status_get_string(result.error_value().status()));
  }
#else
  trace_provider_packet_t packet{};

  size_t alert_name_length = strlen(alert_name);
  if (alert_name_length > sizeof(packet.data16) + sizeof(packet.data32) + sizeof(packet.data64)) {
    fprintf(stderr, "Session: Alert name too long: %s\n", alert_name);
    return;
  }

  packet.request = TRACE_PROVIDER_ALERT;
  memcpy(&packet.data16, alert_name, alert_name_length);
  auto status = fifo_.write(sizeof(packet), &packet, 1, nullptr);
  ZX_DEBUG_ASSERT(status == ZX_OK || status == ZX_ERR_PEER_CLOSED);
#endif
}

}  // namespace internal
}  // namespace trace
