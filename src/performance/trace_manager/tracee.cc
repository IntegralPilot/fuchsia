// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace_manager/tracee.h"

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

Tracee::Tracee(async::Executor& executor, std::shared_ptr<const BufferForwarder> output,
               const TraceProviderBundle* bundle)
    : output_(std::move(output)),
      bundle_(bundle),
      executor_(executor),
      wait_(this),
      weak_ptr_factory_(this) {}

bool Tracee::operator==(TraceProviderBundle* bundle) const { return bundle_ == bundle; }

bool Tracee::Initialize(std::vector<std::string> categories, size_t buffer_size,
                        fuchsia_tracing::BufferingMode buffering_mode, StartCallback start_callback,
                        StopCallback stop_callback, TerminateCallback terminate_callback,
                        AlertCallback alert_callback) {
  FX_DCHECK(state_ == State::kReady);
  FX_DCHECK(!buffer_);
  FX_DCHECK(start_callback);
  FX_DCHECK(stop_callback);
  FX_DCHECK(terminate_callback);
  FX_DCHECK(alert_callback);

  // HACK(https://fxbug.dev/308796439): Until we get kernel trace streameing, kernel tracing is
  // special: it always allocates a fixed sized buffer in the kernel set by a boot arg. We're not at
  // liberty here in trace_manager to check what the bootarg is, but the default is 32MB. For
  // ktrace_provider, we should allocate a buffer at least large enough to hold the full kernel
  // trace.
  if (bundle_->name == "ktrace_provider") {
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
      SharedBuffer::Create(buffer_size, buffering_mode, bundle_->name, bundle_->id);
  if (shared_buffer.is_error()) {
    return false;
  }

  zx::vmo buffer_vmo_for_provider;
  if (zx_status_t status = shared_buffer->Vmo()->duplicate(
          ZX_RIGHTS_BASIC | ZX_RIGHTS_IO | ZX_RIGHT_MAP, &buffer_vmo_for_provider);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << std::format("{} Failed to duplicate trace buffer for provider",
                                           bundle_->name);
    return false;
  }

  zx::fifo fifo, fifo_for_provider;
  if (zx_status_t status = zx::fifo::create(kFifoSizeInPackets, sizeof(trace_provider_packet_t), 0u,
                                            &fifo, &fifo_for_provider);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << *bundle_ << ": Failed to create trace buffer fifo";
    return false;
  }

  fuchsia_tracing_provider::ProviderConfig provider_config;
  provider_config.buffering_mode(buffering_mode);
  provider_config.buffer(std::move(buffer_vmo_for_provider));
  provider_config.fifo(std::move(fifo_for_provider));
  provider_config.categories(std::move(categories));
  auto result = bundle_->provider->Initialize({std::move(provider_config)});
  if (result.is_error()) {
    FX_LOGS(ERROR) << *bundle_ << ": Failed to initialize provider: " << result.error_value();
    return false;
  }

  buffer_ = std::move(*shared_buffer);
  fifo_ = std::move(fifo);

  start_callback_ = std::move(start_callback);
  stop_callback_ = std::move(stop_callback);
  terminate_callback_ = std::move(terminate_callback);
  alert_callback_ = std::move(alert_callback);

  wait_.set_object(fifo_.get());
  wait_.set_trigger(ZX_FIFO_READABLE | ZX_FIFO_PEER_CLOSED);
  const zx_status_t status = wait_.Begin(executor_.dispatcher());
  FX_CHECK(status == ZX_OK) << "Failed to add handler: status=" << status;

  TransitionToState(State::kInitialized);
  return true;
}

void Tracee::Terminate() {
  if (state_ == State::kTerminating || state_ == State::kTerminated) {
    return;
  }
  auto result = bundle_->provider->Terminate();
  if (result.is_error()) {
    FX_LOGS(ERROR) << *bundle_ << ": Failed to terminate provider: " << result.error_value();
  }
  TransitionToState(State::kTerminating);
}

void Tracee::Start(fuchsia_tracing::BufferDisposition buffer_disposition,
                   const std::vector<std::string>& additional_categories) {
  // TraceSession should not call us unless we're ready, either because this
  // is the first time, or subsequent times after tracing has fully stopped
  // from the preceding time.
  FX_DCHECK(state_ == State::kInitialized || state_ == State::kStopped);

  fuchsia_tracing_provider::StartOptions start_options;
  start_options.buffer_disposition(buffer_disposition);
  start_options.additional_categories(additional_categories);
  auto result = bundle_->provider->Start({std::move(start_options)});
  if (result.is_error()) {
    FX_LOGS(ERROR) << *bundle_ << ": Failed to start provider: " << result.error_value();
  }

  TransitionToState(State::kStarting);
  was_started_ = true;
  results_written_ = false;
}

void Tracee::Stop(bool write_results) {
  if (state_ != State::kStarting && state_ != State::kStarted) {
    if (state_ == State::kInitialized) {
      // We must have gotten added after tracing started while tracing was
      // being stopped. Mark us as stopped so TraceSession won't try to wait
      // for us to do so.
      TransitionToState(State::kStopped);
    }
    return;
  }
  auto result = bundle_->provider->Stop();
  if (result.is_error()) {
    FX_LOGS(ERROR) << *bundle_ << ": Failed to stop provider: " << result.error_value();
  }
  TransitionToState(State::kStopping);
  write_results_ = write_results;
}

void Tracee::TransitionToState(State new_state) {
  FX_LOGS(DEBUG) << *bundle_ << ": Transitioning from " << state_ << " to " << new_state;
  state_ = new_state;
}

void Tracee::OnHandleReady(async_dispatcher_t* dispatcher, async::WaitBase* wait,
                           zx_status_t status, const zx_packet_signal_t* signal) {
  if (status != ZX_OK) {
    OnHandleError(status);
    return;
  }

  zx_signals_t pending = signal->observed;
  FX_LOGS(DEBUG) << *bundle_ << ": pending=0x" << std::hex << pending;
  FX_DCHECK(pending & (ZX_FIFO_READABLE | ZX_FIFO_PEER_CLOSED));
  FX_DCHECK(state_ != State::kReady && state_ != State::kTerminated);

  if (pending & ZX_FIFO_READABLE) {
    auto weak = weak_ptr_factory_.GetWeakPtr();
    OnFifoReadable(dispatcher, wait);
    if (!weak) {
      return;
    }
    // Keep reading packets, one per call, until the peer goes away.
    status = wait->Begin(dispatcher);
    if (status != ZX_OK)
      OnHandleError(status);
    return;
  }

  FX_DCHECK(pending & ZX_FIFO_PEER_CLOSED);
  wait_.set_object(ZX_HANDLE_INVALID);
  TransitionToState(State::kTerminated);
  fit::closure terminate_callback = std::move(terminate_callback_);
  FX_DCHECK(terminate_callback);
  terminate_callback();
}

void Tracee::OnFifoReadable(async_dispatcher_t* dispatcher, async::WaitBase* wait) {
  trace_provider_packet_t packet;
  auto status2 = zx_fifo_read(wait_.object(), sizeof(packet), &packet, 1u, nullptr);
  FX_DCHECK(status2 == ZX_OK);
  if (packet.data16 != 0 && packet.request != TRACE_PROVIDER_ALERT) {
    FX_LOGS(ERROR) << *bundle_ << ": Received bad packet, non-zero data16 field: " << packet.data16;
    Abort();
    return;
  }

  switch (packet.request) {
    case TRACE_PROVIDER_STARTED:
      // The provider should only be signalling us when it has finished
      // startup.
      if (packet.data32 != TRACE_PROVIDER_FIFO_PROTOCOL_VERSION) {
        FX_LOGS(ERROR) << *bundle_
                       << ": Received bad packet, unexpected version: " << packet.data32;
        Abort();
        break;
      }
      if (packet.data64 != 0) {
        FX_LOGS(ERROR) << *bundle_
                       << ": Received bad packet, non-zero data64 field: " << packet.data64;
        Abort();
        break;
      }
      if (state_ == State::kStarting) {
        TransitionToState(State::kStarted);
        start_callback_();
      } else {
        // This could be a problem in the provider or it could just be slow.
        // TODO(dje): Disconnect it and force it to reconnect?
        FX_LOGS(WARNING) << *bundle_ << ": Received TRACE_PROVIDER_STARTED in state " << state_;
      }
      break;
    case TRACE_PROVIDER_SAVE_BUFFER:
      if (buffer_->BufferingMode() != fuchsia_tracing::BufferingMode::kStreaming) {
        FX_LOGS(WARNING) << *bundle_ << ": Received TRACE_PROVIDER_SAVE_BUFFER in mode "
                         << ModeName(buffer_->BufferingMode());
      } else if (state_ == State::kStarted || state_ == State::kStopping ||
                 state_ == State::kTerminating) {
        uint32_t wrapped_count = packet.data32;
        uint64_t durable_data_end = packet.data64;
        // Schedule the write with the main async loop.
        FX_LOGS(DEBUG) << "Buffer save request from " << *bundle_
                       << ", wrapped_count=" << wrapped_count << ", durable_data_end=0x" << std::hex
                       << durable_data_end;
        async::PostTask(executor_.dispatcher(),
                        [weak = weak_ptr_factory_.GetWeakPtr(), wrapped_count, durable_data_end] {
                          if (weak) {
                            weak->TransferBuffer(wrapped_count, durable_data_end);
                          }
                        });
      } else {
        FX_LOGS(WARNING) << *bundle_ << ": Received TRACE_PROVIDER_SAVE_BUFFER in state " << state_;
      }
      break;
    case TRACE_PROVIDER_STOPPED:
      if (packet.data16 != 0 || packet.data32 != 0 || packet.data64 != 0) {
        FX_LOGS(ERROR) << *bundle_ << ": Received bad packet, non-zero data fields";
        Abort();
        break;
      }
      if (state_ == State::kStopping || state_ == State::kTerminating) {
        // If we're terminating leave the transition to kTerminated to
        // noticing the fifo peer closed.
        if (state_ == State::kStopping) {
          TransitionToState(State::kStopped);
        }
        stop_callback_(write_results_);
      } else {
        // This could be a problem in the provider or it could just be slow.
        // TODO(dje): Disconnect it and force it to reconnect?
        FX_LOGS(WARNING) << *bundle_ << ": Received TRACE_PROVIDER_STOPPED in state " << state_;
      }
      break;
    case TRACE_PROVIDER_ALERT: {
      auto p = reinterpret_cast<const char*>(&packet.data16);
      size_t size = sizeof(packet.data16) + sizeof(packet.data32) + sizeof(packet.data64);
      std::string alert_name;
      alert_name.reserve(size);

      for (size_t i = 0; i < size && *p != 0; ++i) {
        alert_name.push_back(*p++);
      }

      alert_callback_(std::move(alert_name));
    } break;
    default:
      FX_LOGS(ERROR) << *bundle_ << ": Received bad packet, unknown request: " << packet.request;
      Abort();
      break;
  }
}

void Tracee::OnHandleError(zx_status_t status) {
  FX_LOGS(DEBUG) << *bundle_ << ": error=" << status;
  FX_DCHECK(status == ZX_ERR_CANCELED);
  FX_DCHECK(state_ != State::kReady && state_ != State::kTerminated);
  wait_.set_object(ZX_HANDLE_INVALID);
  TransitionToState(State::kTerminated);
}

TransferStatus Tracee::TransferRecords() const {
  FX_DCHECK(buffer_);

  // Regardless of whether we succeed or fail, mark results as being written.
  results_written_ = true;
  auto [status, stats] = buffer_->TransferAll(output_);
  if (status != TransferStatus::kComplete) {
    return status;
  }

  provider_stats_.name(bundle_->name);
  provider_stats_.pid(bundle_->pid);
  provider_stats_.buffering_mode(buffer_->BufferingMode());
  provider_stats_.buffer_wrapped_count(stats.wrapped_count);
  provider_stats_.records_dropped(stats.records_dropped);
  provider_stats_.percentage_durable_buffer_used(stats.durable_used_percent);
  provider_stats_.non_durable_bytes_written(stats.non_durable_bytes_written);

  return TransferStatus::kComplete;
}

std::optional<fuchsia_tracing_controller::ProviderStats> Tracee::GetStats() const {
  if (state_ == State::kTerminated || state_ == State::kStopped) {
    return std::move(provider_stats_);
  }
  return std::nullopt;
}

void Tracee::TransferBuffer(uint32_t wrapped_count, uint64_t durable_data_end) {
  FX_DCHECK(buffer_);

  if (!buffer_->StreamingTransfer(output_, wrapped_count, durable_data_end)) {
    Abort();
    return;
  }
  NotifyBufferSaved(wrapped_count, durable_data_end);
}

void Tracee::NotifyBufferSaved(uint32_t wrapped_count, uint64_t durable_data_end) {
  FX_LOGS(DEBUG) << "Buffer saved for " << *bundle_ << ", wrapped_count=" << wrapped_count
                 << ", durable_data_end=" << durable_data_end;
  trace_provider_packet_t packet{
      .request = TRACE_PROVIDER_BUFFER_SAVED,
      .data16 = 0,
      .data32 = wrapped_count,
      .data64 = durable_data_end,
  };
  auto status = fifo_.write(sizeof(packet), &packet, 1, nullptr);
  if (status == ZX_ERR_SHOULD_WAIT) {
    // The FIFO should never fill. If it does then the provider is sending us
    // buffer full notifications but not reading our replies. Terminate the
    // connection.
    Abort();
  } else {
    FX_DCHECK(status == ZX_OK || status == ZX_ERR_PEER_CLOSED);
  }
}

void Tracee::Abort() {
  FX_LOGS(ERROR) << *bundle_ << ": Aborting connection";
  Terminate();
}

std::ostream& operator<<(std::ostream& out, Tracee::State state) {
  switch (state) {
    case Tracee::State::kReady:
      out << "ready";
      break;
    case Tracee::State::kInitialized:
      out << "initialized";
      break;
    case Tracee::State::kStarting:
      out << "starting";
      break;
    case Tracee::State::kStarted:
      out << "started";
      break;
    case Tracee::State::kStopping:
      out << "stopping";
      break;
    case Tracee::State::kStopped:
      out << "stopped";
      break;
    case Tracee::State::kTerminating:
      out << "terminating";
      break;
    case Tracee::State::kTerminated:
      out << "terminated";
      break;
  }

  return out;
}

}  // namespace tracing
