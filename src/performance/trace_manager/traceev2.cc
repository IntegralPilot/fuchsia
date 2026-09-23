// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace_manager/traceev2.h"

#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-engine/fields.h>
#include <lib/trace-provider/provider.h>

#include <memory>

#include <fbl/algorithm.h>

#include "src/performance/trace_manager/shared_buffer.h"
#include "src/performance/trace_manager/util.h"
#include "zircon/syscalls.h"

// LINT.IfChange
// Pulled from trace_engine's context_impl.h
static constexpr size_t kMaxDurableBufferSize = size_t{1024} * 1024;

// LINT.ThenChange(//zircon/system/ulib/trace-engine/context_impl.h)

namespace tracing {
using State = Tracee::State;

TraceeV2::TraceeV2(async::Executor& executor, std::shared_ptr<const BufferForwarder> output,
                   ProviderConnection* connection)
    : output_(std::move(output)),
      connection_(connection),
      executor_(executor),
      weak_ptr_factory_(this) {}

bool TraceeV2::operator==(ProviderConnection* connection) const {
  return connection_ == connection;
}

bool TraceeV2::Initialize(std::vector<std::string> categories, size_t buffer_size,
                          fuchsia_tracing::BufferingMode buffering_mode) {
  FX_DCHECK(state_ == State::kReady);
  FX_DCHECK(!buffer_);

  // HACK(https://fxbug.dev/308796439): Until we get kernel trace streaming, kernel tracing is
  // special: it always allocates a fixed sized buffer in the kernel set by a boot arg. We're not at
  // liberty here in trace_manager to check what the bootarg is, but the default is 32MB. For
  // ktrace_provider, we should allocate a buffer at least large enough to hold the full kernel
  // trace.
  if (connection_->name == "ktrace_provider") {
    buffer_size = std::max(buffer_size, size_t{32} * 1024 * 1024);
    // In streaming and circular mode, part of the trace buffer will be reserved for the durable
    // buffer. If ktrace attempts to write 32MiB of data, and our buffer is also 32MiB, we'll drop
    // data because our usable buffer size will be slightly smaller.
    //
    // For the same reason, we need to add on some additional space for the metadata records that
    // trace-engine writes since the partially fill the buffer.
    if (buffering_mode != fuchsia_tracing::BufferingMode::kOneshot) {
      buffer_size += kMaxDurableBufferSize + size_t{zx_system_get_page_size()};
    }
  }

  zx::result<SharedBuffer> shared_buffer =
      SharedBuffer::Create(buffer_size, buffering_mode, connection_->name, connection_->id);
  if (shared_buffer.is_error()) {
    return false;
  }

  zx::vmo buffer_vmo_for_provider;
  if (zx_status_t status = shared_buffer->Vmo()->duplicate(
          ZX_RIGHTS_BASIC | ZX_RIGHTS_IO | ZX_RIGHT_MAP, &buffer_vmo_for_provider);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << std::format("{} Failed to duplicate trace buffer for provider",
                                           connection_->name);
    return false;
  }

  fuchsia_tracing_provider::ProviderConfigV2 provider_config;
  provider_config.buffering_mode(buffering_mode);
  provider_config.buffer(std::move(buffer_vmo_for_provider));
  provider_config.categories(std::move(categories));
  auto result = connection_->provider->Initialize({std::move(provider_config)});
  if (result.is_error()) {
    FX_LOGS(ERROR) << std::format("{}: Failed to initialize provider: ", *connection_)
                   << result.error_value();
    return false;
  }

  buffer_ = std::move(*shared_buffer);

  TransitionToState(State::kInitialized);
  return true;
}

void TraceeV2::Terminate(fit::closure cb) {
  if (state_ == State::kTerminating || state_ == State::kTerminated) {
    return;
  }

  connection_->provider->Terminate().ThenExactlyOnce(
      [weak = weak_ptr_factory_.GetWeakPtr(), cb = std::move(cb),
       conn = this->connection_](fidl::Result<fuchsia_tracing_provider::ProviderV2::Terminate>& e) {
        if (e.is_error()) {
          FX_LOGS(ERROR) << std::format("{}: Failed to terminate provider: {}", *conn,
                                        e.error_value().FormatDescription());
          return;
        }
        if (weak) {
          FX_DCHECK(weak->state() != State::kReady && weak->state() != State::kTerminated);
          weak->TransitionToState(State::kTerminated);
        }
        cb();
      });

  TransitionToState(State::kTerminating);
}

void TraceeV2::Start(fuchsia_tracing::BufferDisposition buffer_disposition,
                     const std::vector<std::string>& additional_categories, fit::closure cb) {
  // TraceSession should not call us unless we're ready, either because this
  // is the first time, or subsequent times after tracing has fully stopped
  // from the preceding time.
  FX_DCHECK(state_ == State::kInitialized || state_ == State::kStopped);

  fuchsia_tracing_provider::StartOptions start_options;
  start_options.buffer_disposition(buffer_disposition);
  start_options.additional_categories(additional_categories);
  connection_->provider->Start({std::move(start_options)})
      .ThenExactlyOnce([weak = weak_ptr_factory_.GetWeakPtr(), cb = std::move(cb),
                        conn = this->connection_](
                           fidl::Result<fuchsia_tracing_provider::ProviderV2::Start>& e) {
        if (e.is_error()) {
          FX_LOGS(ERROR) << std::format("{}: Failed to start provider: {}", *conn,
                                        e.error_value().FormatDescription());
          return;
        }
        if (weak) {
          if (weak->state() == State::kStarting) {
            weak->TransitionToState(State::kStarted);
            cb();
          } else {
            FX_LOGS(WARNING) << std::format("{}: Received Started confirmation in state ", *conn)
                             << weak->state();
          }
        }
      });

  TransitionToState(State::kStarting);
  was_started_ = true;
  results_written_ = false;
}

void TraceeV2::Stop(fit::closure cb) {
  if (state_ != State::kStarting && state_ != State::kStarted) {
    if (state_ == State::kInitialized) {
      // We must have gotten added after tracing started while tracing was
      // being stopped. Mark us as stopped so TraceSession won't try to wait
      // for us to do so.
      TransitionToState(State::kStopped);
    }
    return;
  }
  zx_time_t start_time = zx_clock_get_monotonic();
  connection_->provider->Stop().ThenExactlyOnce(
      [weak = weak_ptr_factory_.GetWeakPtr(), cb = std::move(cb), conn = this->connection_,
       start_time](fidl::Result<fuchsia_tracing_provider::ProviderV2::Stop>& e) {
        if (e.is_error()) {
          FX_LOGS(ERROR) << std::format("{}: Failed to stop provider: {}", *conn,
                                        e.error_value().FormatDescription());
          return;
        }
        if (!weak) {
          return;
        }
        switch (weak->state()) {
          case Tracee::State::kReady:
          case Tracee::State::kInitialized:
          case Tracee::State::kStarting:
          case Tracee::State::kStarted:
          case Tracee::State::kStopped:
          case Tracee::State::kTerminated:
            FX_LOGS(WARNING) << std::format("{}: Received Stopped confirmation in state ", *conn)
                             << weak->state();
            break;
          case Tracee::State::kStopping: {
            weak->TransitionToState(State::kStopped);
            zx_time_t end_time = zx_clock_get_monotonic();
            double elapsed_ms = static_cast<double>(end_time - start_time) / 1000000.0;
            if (elapsed_ms > 1000.0) {
              FX_LOGS(INFO) << std::format("{}: Stop took {:.2f} ms", *conn, elapsed_ms);
            } else {
              FX_LOGS(DEBUG) << std::format("{}: Stop took {:.2f} ms", *conn, elapsed_ms);
            }
          } break;
          case Tracee::State::kTerminating:
            break;
        }
        cb();
      });
  TransitionToState(State::kStopping);
}

zx::result<> TraceeV2::RequestFlush() {
  fit::result res = connection_->provider->Flush();
  if (res.is_error()) {
    return zx::error(res.error_value().status());
  }
  return zx::ok();
}

void TraceeV2::TransitionToState(State new_state) {
  FX_LOGS(DEBUG) << std::format("{}: Transitioning from ", *connection_) << state_ << " to "
                 << new_state;
  state_ = new_state;
}

TransferStatus TraceeV2::TransferRecords() const {
  FX_DCHECK(buffer_);

  // Regardless of whether we succeed or fail, mark results as being written.
  results_written_ = true;
  auto [status, stats] = buffer_->TransferAll(output_);
  if (status != TransferStatus::kComplete) {
    return status;
  }

  provider_stats_.name(connection_->name);
  provider_stats_.pid(connection_->pid);
  provider_stats_.buffering_mode(buffer_->BufferingMode());
  provider_stats_.buffer_wrapped_count(stats.wrapped_count);
  provider_stats_.records_dropped(stats.records_dropped);
  provider_stats_.percentage_durable_buffer_used(stats.durable_used_percent);
  provider_stats_.non_durable_bytes_written(stats.non_durable_bytes_written);

  return TransferStatus::kComplete;
}

std::optional<fuchsia_tracing_controller::ProviderStats> TraceeV2::GetStats() const {
  if (state_ == State::kTerminated || state_ == State::kStopped) {
    return std::move(provider_stats_);
  }
  return std::nullopt;
}

zx::result<> TraceeV2::TransferBuffer(uint32_t wrapped_count, uint64_t durable_data_end) {
  FX_DCHECK(buffer_);

  if (!buffer_->StreamingTransfer(output_, wrapped_count, durable_data_end)) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  FX_LOGS(DEBUG) << std::format("Buffer saved for {}, wrapped_count={}, durable_data_end={}",
                                *connection_, wrapped_count, durable_data_end);
  fit::result<fidl::OneWayError> res = connection_->provider->NotifyBufferSaved(
      {{.wrapped_count = wrapped_count, .durable_data_end = durable_data_end}});
  if (res.is_error()) {
    return zx::error(res.error_value().status());
  }
  return zx::ok();
}

}  // namespace tracing
