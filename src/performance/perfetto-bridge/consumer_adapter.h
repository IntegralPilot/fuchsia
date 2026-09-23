// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_PERFETTO_BRIDGE_CONSUMER_ADAPTER_H_
#define SRC_PERFORMANCE_PERFETTO_BRIDGE_CONSUMER_ADAPTER_H_

#include <lib/fit/function.h>
#include <lib/trace-engine/instrumentation.h>
#include <lib/trace-provider/provider.h>
#include <lib/trace/observer.h>

#include <atomic>
#include <memory>
#include <mutex>

#include <perfetto/base/task_runner.h>
#include <perfetto/ext/tracing/core/consumer.h>
#include <perfetto/ext/tracing/core/tracing_service.h>

#include "src/lib/fxl/synchronization/thread_annotations.h"

// Encapsulation of the Fuchsia trace observer and provider protocols,
// for testing.
// TODO(https://fxbug.dev/42067571): Use a testable C++ API from trace engine if one becomes
// available.
class FuchsiaTracing {
 public:
  FuchsiaTracing() = default;
  virtual ~FuchsiaTracing() = default;

  virtual void StartObserving(fit::function<void(trace_state_t)>) = 0;
  virtual void AcquireProlongedContext() = 0;
  virtual void ReleaseProlongedContext() = 0;
  virtual void AcquireWriteContext() = 0;
  virtual bool HasWriteContext() = 0;
  virtual void WriteBlob(const char* data, size_t size) = 0;
  virtual void ReleaseWriteContext() = 0;
  virtual trace::ProviderConfig GetProviderConfig() = 0;
  virtual void SetGetKnownCategoriesCallback(trace::GetKnownCategoriesCallback callback) = 0;
};

// Adapts the Fuchsia Tracing protocol to the Perfetto Consumer protocol.
// Perfetto events are handled via the perfetto::Consumer method implementations.
// Commands are sent to Perfetto via |consumer_endpoint_|.
class ConsumerAdapter : public perfetto::Consumer {
 public:
  using ConnectConsumerCallback =
      std::function<std::unique_ptr<perfetto::ConsumerEndpoint>(perfetto::Consumer*)>;

  // Sets the amount of buffer usage that will cause the buffer to be read mid-trace.
  static constexpr float kConsumerUtilizationReadThreshold = 0.6f;

  ConsumerAdapter(perfetto::TracingService* tracing_service, trace::TraceProvider* trace_provider,
                  perfetto::base::TaskRunner* perfetto_task_runner);

  // Public for testing.
  ConsumerAdapter(ConnectConsumerCallback connect_callback,
                  std::unique_ptr<FuchsiaTracing> fuchsia_tracing,
                  perfetto::base::TaskRunner* perfetto_task_runner);

  ~ConsumerAdapter() override;

  ConsumerAdapter(const ConsumerAdapter& other) = delete;
  void operator=(const ConsumerAdapter& other) = delete;

 private:
  class ScopedProlongedTraceContext;

  // Finite state machine states.
  // State transition rules are applied in ChangeState().
  enum class State {
    INACTIVE,  // Tracing inactive.

    // Active tracing states.
    ACTIVE,   // Tracing active; scheduled stats-checking task pending.
    STATS,    // Periodic buffer utilization check before READING.
              // Changes to ACTIVE if there is sufficient space in the buffer.
    READING,  // Reading consumer buffer once STATS threshold is hit.
              // Changes to ACTIVE on read completion.

    // Shutdown states, run in-order in response to the Fuchsia TRACE_STOPPING event.
    READING_PENDING_SHUTDOWN,  // If shutdown is called mid-read, defers shutdown until reading
                               // has finished. Changes to SHUTDOWN_FLUSH on read completion.
    SHUTDOWN_FLUSH,            // Flush() called on shutdown.
    SHUTDOWN_DISABLED,         // DisableTracing() called after flush completion.
    SHUTDOWN_READING,          // ReadBuffers() called after tracing has stopped.
    SHUTDOWN_STATS,            // GetTraceStats() called for end-of-session diagnostics logging.
                               // Changes to INACTIVE when complete.
  };

  void ChangeState(State new_state);
  State GetState();

  // Handles fuchsia.tracing events.
  void OnTraceStateUpdate(trace_state_t new_state);

  // Called in response to the Fuchsia TRACE_STARTED event.
  void OnStartTracing();

  // Called in response to the Fuchsia TRACE_STOPPING event.
  void CallPerfettoDisableTracing();

  void ShutdownTracing();

  void SchedulePerfettoGetStats();
  void CallPerfettoReadBuffers(bool on_shutdown);
  void OnPerfettoReadBuffersComplete();

  void CallPerfettoFlush();
  void CallPerfettoGetTraceStats(bool on_shutdown);

  std::vector<trace::KnownCategory> GetKnownCategories();

  // perfetto::Consumer implementation.
  void OnConnect() override;
  void OnDisconnect() override;
  void OnTracingDisabled(const std::string& error) override;
  void OnTraceData(std::vector<perfetto::TracePacket> packets, bool has_more) override;
  void OnDetach(bool success) override;
  void OnAttach(bool success, const perfetto::TraceConfig&) override;
  void OnTraceStats(bool success, const perfetto::TraceStats&) override;
  void OnObservableEvents(const perfetto::ObservableEvents&) override;

  // Interactions with `perfetto_service_` and `consumer_endpoint_` must take place on
  // `perfetto_task_runner_`. `consumer_endpoint_` lives for the duration of a tracing session.
  perfetto::base::TaskRunner* perfetto_task_runner_;
  ConnectConsumerCallback connect_callback_;
  std::unique_ptr<perfetto::ConsumerEndpoint> consumer_endpoint_;

  std::unique_ptr<FuchsiaTracing> fuchsia_tracing_;
  std::atomic<State> state_ FXL_GUARDED_BY(state_mutex_) = State::INACTIVE;
  std::mutex state_mutex_;
};

#endif  // SRC_PERFORMANCE_PERFETTO_BRIDGE_CONSUMER_ADAPTER_H_
