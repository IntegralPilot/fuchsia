// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_ULIB_TRACE_PROVIDER_SESSION_H_
#define ZIRCON_SYSTEM_ULIB_TRACE_PROVIDER_SESSION_H_

#include <fidl/fuchsia.tracing.provider/cpp/wire.h>
#include <lib/async/cpp/wait.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/fit/function.h>
#include <lib/trace-provider/handler.h>
#include <lib/trace-provider/provider.h>
#include <lib/zx/fifo.h>
#include <lib/zx/vmo.h>

#include <memory>
#include <string>
#include <vector>

namespace trace {
namespace internal {

class Session final : public trace::TraceHandler {
 public:
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  static std::unique_ptr<Session> InitializeEngine(
      async_dispatcher_t* dispatcher, trace_buffering_mode_t buffering_mode, zx::vmo buffer,
      std::vector<std::string> categories,
      fidl::ServerBindingRef<fuchsia_tracing_provider::ProviderV2> binding);
  void StartEngine(trace_start_mode_t start_mode, fit::callback<void()> cb);

  void StopEngine(fit::callback<void()> cb);
  void TerminateEngine(fit::callback<void()> cb) __TA_EXCLUDES(mutex_);
#else
  static void InitializeEngine(async_dispatcher_t* dispatcher,
                               trace_buffering_mode_t buffering_mode, zx::vmo buffer, zx::fifo fifo,
                               std::vector<std::string> categories);
  static void StartEngine(trace_start_mode_t start_mode);

  static void StopEngine();
  static void TerminateEngine();
#endif
  ~Session() override;

  // This is called in streaming mode when TraceManager reports back that
  // it has saved the buffer.
  static zx_status_t MarkBufferSaved(uint32_t wrapped_count, uint64_t durable_data_end);

 private:
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  Session(void* buffer, size_t buffer_num_bytes, std::vector<std::string> categories,
          fidl::ServerBindingRef<fuchsia_tracing_provider::ProviderV2> binding);
#else
  Session(async_dispatcher_t* dispatcher, void* buffer, size_t buffer_num_bytes, zx::fifo fifo,
          std::vector<std::string> categories);
#endif

  // |trace::TraceHandler|
  bool IsCategoryEnabled(const char* category) override;
  void TraceStarted() override;
  void TraceStopped(zx_status_t disposition) override;
  void TraceTerminated() override;
  // This is called in streaming mode to notify the trace manager that
  // buffer |buffer_number| is full and needs to be saved.
  void NotifyBufferFull(uint32_t wrapped_count, uint64_t durable_data_end) override;
  void SendAlert(const char* alert_name) override;

#if FUCHSIA_API_LEVEL_LESS_THAN(31)
  void HandleFifo(async_dispatcher_t* dispatcher, async::WaitBase* wait, zx_status_t status,
                  const zx_packet_signal_t* signal);
  bool ReadFifoMessage();
  void SendFifoPacket(const trace_provider_packet_t* packet);
#endif

  // We can be accessed by multiple threads concurrently if:
  //
  // - We're running in a multithreaded async loop/executor
  // - The async loop we're running on spawns it own thread. Then we're shared between the main
  //   thread and the async loop thread, which can race during destruction.
  std::mutex mutex_;

  fit::callback<void()> start_cb_ __TA_GUARDED(mutex_);
  fit::callback<void()> stop_cb_ __TA_GUARDED(mutex_);
  fit::callback<void()> terminate_cb_ __TA_GUARDED(mutex_);
  void* const buffer_;
  const size_t buffer_num_bytes_;
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  fidl::ServerBindingRef<fuchsia_tracing_provider::ProviderV2> binding_ __TA_GUARDED(mutex_);
#else
  async_dispatcher_t* const dispatcher_;
  zx::fifo fifo_;
  async::WaitMethod<Session, &Session::HandleFifo> fifo_wait_;
#endif
  std::vector<std::string> enabled_categories_ __TA_GUARDED(mutex_);

  Session(const Session&) = delete;
  Session(Session&&) = delete;
  Session& operator=(const Session&) = delete;
  Session& operator=(Session&&) = delete;
};

}  // namespace internal
}  // namespace trace

#endif  // ZIRCON_SYSTEM_ULIB_TRACE_PROVIDER_SESSION_H_
