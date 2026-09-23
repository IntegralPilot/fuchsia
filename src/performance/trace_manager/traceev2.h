// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE_MANAGER_TRACEEV2_H_
#define SRC_PERFORMANCE_TRACE_MANAGER_TRACEEV2_H_

#include <fidl/fuchsia.tracing.controller/cpp/fidl.h>
#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <fidl/fuchsia.tracing/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/cpp/wait.h>
#include <lib/fit/function.h>
#include <lib/zx/vmo.h>

#include <trace-reader/reader_internal.h>

#include "src/performance/trace_manager/buffer_forwarder.h"
#include "src/performance/trace_manager/provider_connection.h"
#include "src/performance/trace_manager/tracee.h"

namespace tracing {

class TraceSession;

class TraceeV2 {
 public:
  TraceeV2(async::Executor& executor, std::shared_ptr<const BufferForwarder> output,
           ProviderConnection* connection);
  bool operator==(ProviderConnection* connection) const;
  ~TraceeV2() = default;

  // Transfer all collected records to output_.
  TransferStatus TransferRecords() const;

  // Save the buffer specified by |wrapped_count|.
  // This is a callback from the TraceSession loop.
  // That's why the result is void and not Tracee::TransferStatus.
  zx::result<> TransferBuffer(uint32_t wrapped_count, uint64_t durable_data_end);

  bool Initialize(std::vector<std::string> categories, size_t buffer_size,
                  fuchsia_tracing::BufferingMode buffering_mode);
  void Terminate(fit::closure cb);

  void Start(fuchsia_tracing::BufferDisposition buffer_disposition,
             const std::vector<std::string>& additional_categories, fit::closure cb);

  void Stop(fit::closure cb);

  ProviderConnection* connection() const { return connection_; }
  Tracee::State state() const { return state_; }
  bool was_started() const { return was_started_; }
  bool results_written() const { return results_written_; }
  std::optional<fuchsia_tracing_controller::ProviderStats> GetStats() const;
  void NotifyBufferSaved(uint32_t wrapped_count, uint64_t durable_data_end);

  // Request the provider prepare to flush its buffer.
  zx::result<> RequestFlush();

  void Abort();
  fxl::WeakPtr<TraceeV2> GetWeakPtr() { return weak_ptr_factory_.GetWeakPtr(); }

 private:
  TraceeV2(const TraceeV2&) = delete;
  TraceeV2(TraceeV2&&) = delete;
  TraceeV2& operator=(const TraceeV2&) = delete;
  TraceeV2& operator=(TraceeV2&&) = delete;

  void TransitionToState(Tracee::State new_state);

  const std::shared_ptr<const BufferForwarder> output_;
  ProviderConnection* connection_;
  Tracee::State state_ = Tracee::State::kReady;

  async::Executor& executor_;
  std::optional<SharedBuffer> buffer_;

  // Set to true when starting. This is used to not write any results,
  // including provider info, if the tracee was never started.
  bool was_started_ = false;

  // Set to false when starting and true when results are written.
  // This is used to not save the results twice when terminating.
  mutable bool results_written_ = false;

  // Final trace stats
  mutable fuchsia_tracing_controller::ProviderStats provider_stats_;

  fxl::WeakPtrFactory<TraceeV2> weak_ptr_factory_;
};
}  // namespace tracing

#endif  // SRC_PERFORMANCE_TRACE_MANAGER_TRACEEV2_H_
