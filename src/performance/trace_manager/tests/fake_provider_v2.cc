// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace_manager/tests/fake_provider_v2.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace-engine/buffer_internal.h>
#include <lib/trace-engine/fields.h>

#include <format>

namespace tracing {
namespace test {

FakeProviderV2::FakeProviderV2(zx_koid_t pid, const std::string& name) : pid_(pid), name_(name) {}

std::string FakeProviderV2::PrettyName() const { return std::format("{{{:d}:{}}}", pid_, name_); }

void FakeProviderV2::Initialize(InitializeRequest& request, InitializeCompleter::Sync& completer) {
  FX_LOGS(DEBUG) << PrettyName() << ": Received Initialize message";
  ++initialize_count_;

  auto& config = request.config();
  ASSERT_TRUE(config.buffering_mode().has_value());
  ASSERT_TRUE(config.buffer().has_value());
  ASSERT_TRUE(config.categories().has_value());

  buffering_mode_ = *config.buffering_mode();
  buffer_vmo_ = std::move(*config.buffer());
  enabled_categories_ = *config.categories();

  InitializeBuffer();
  WriteInitRecord();

  state_ = State::kInitialized;
}

void FakeProviderV2::Start(StartRequest& request, StartCompleter::Sync& completer) {
  FX_LOGS(DEBUG) << PrettyName() << ": Received Start message";
  ++start_count_;

  if (request.options().buffer_disposition() == fuchsia_tracing::BufferDisposition::kRetain) {
    FX_LOGS(DEBUG) << "Retaining buffer contents";
  } else {
    FX_LOGS(DEBUG) << "Clearing buffer contents";
    ResetBufferPointers();
    WriteInitRecord();
  }

  state_ = State::kStarting;
  start_completer_ = completer.ToAsync();
}

void FakeProviderV2::Stop(StopCompleter::Sync& completer) {
  FX_LOGS(DEBUG) << PrettyName() << ": Received Stop message";
  ++stop_count_;

  state_ = State::kStopping;
  stop_completer_ = completer.ToAsync();
}

void FakeProviderV2::Terminate(TerminateCompleter::Sync& completer) {
  FX_LOGS(DEBUG) << PrettyName() << ": Received Terminate message";
  ++terminate_count_;

  state_ = State::kTerminating;
  terminate_completer_ = completer.ToAsync();
}

void FakeProviderV2::NotifyBufferSaved(NotifyBufferSavedRequest& request,
                                       NotifyBufferSavedCompleter::Sync& completer) {
  // Our fake provider doesn't do much with this yet.
}

void FakeProviderV2::GetKnownCategories(GetKnownCategoriesCompleter::Sync& completer) {
  if (!responsive_) {
    pending_cat_completers_.push_back(completer.ToAsync());
    return;
  }
  completer.Reply({{.categories = known_categories_}});
}

void FakeProviderV2::Flush(FlushCompleter::Sync& completer) {
  // One-way method, no reply needed.
}

void FakeProviderV2::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_tracing_provider::ProviderV2> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FX_LOGS(WARNING) << "Received unknown method with ordinal " << metadata.method_ordinal;
}

void FakeProviderV2::MarkStarted() {
  FX_DCHECK(state_ == State::kStarting);
  state_ = State::kStarted;
  if (start_completer_) {
    start_completer_->Reply();
    start_completer_.reset();
  }
}

void FakeProviderV2::MarkStopped() {
  FX_DCHECK(state_ == State::kStopping);
  state_ = State::kStopped;
  UpdateBufferHeaderAfterStopped();
  if (stop_completer_) {
    stop_completer_->Reply();
    stop_completer_.reset();
  }
}

void FakeProviderV2::MarkTerminated() {
  FX_DCHECK(state_ == State::kTerminating);
  state_ = State::kTerminated;
  UpdateBufferHeaderAfterStopped();
  buffer_vmo_.reset();
  if (terminate_completer_) {
    terminate_completer_->Reply();
    terminate_completer_.reset();
  }
}

void FakeProviderV2::MarkUnresponsive() { responsive_ = false; }

void FakeProviderV2::MarkResponsive() {
  responsive_ = true;
  for (auto& completer : pending_cat_completers_) {
    completer.Reply({{.categories = known_categories_}});
  }
  pending_cat_completers_.clear();
}

void FakeProviderV2::InitializeBuffer() {
  ComputeBufferSizes();
  ResetBufferPointers();
  InitBufferHeader();

  if (buffering_mode_ == fuchsia_tracing::BufferingMode::kOneshot) {
    WriteZeroLengthRecord(FakeProvider::kHeaderSize);
  } else {
    size_t durable_buffer_offset = FakeProvider::kHeaderSize;
    WriteZeroLengthRecord(durable_buffer_offset);
    size_t rolling_buffer0_offset = durable_buffer_offset + durable_buffer_size_;
    WriteZeroLengthRecord(rolling_buffer0_offset);
    WriteZeroLengthRecord(rolling_buffer0_offset + rolling_buffer_size_);
  }
}

void FakeProviderV2::ComputeBufferSizes() {
  zx_status_t status = buffer_vmo_.get_size(&total_buffer_size_);
  FX_DCHECK(status == ZX_OK);

  size_t header_size = FakeProvider::kHeaderSize;
  switch (buffering_mode_) {
    case fuchsia_tracing::BufferingMode::kOneshot:
      durable_buffer_size_ = 0;
      rolling_buffer_size_ = total_buffer_size_ - header_size;
      break;
    case fuchsia_tracing::BufferingMode::kCircular:
    case fuchsia_tracing::BufferingMode::kStreaming: {
      size_t avail = total_buffer_size_ - header_size;
      durable_buffer_size_ = FakeProvider::kDurableBufferSize;
      uint64_t off_by = (avail - durable_buffer_size_) & 15;
      durable_buffer_size_ += off_by;
      rolling_buffer_size_ = (avail - durable_buffer_size_) / 2;
      break;
    }
    default:
      FX_NOTREACHED();
      break;
  }
}

void FakeProviderV2::ResetBufferPointers() { buffer_next_ = 0; }

void FakeProviderV2::InitBufferHeader() {
  trace::internal::trace_buffer_header header{};
  header.magic = TRACE_BUFFER_HEADER_MAGIC;
  header.version = TRACE_BUFFER_HEADER_V0;

  switch (buffering_mode_) {
    case fuchsia_tracing::BufferingMode::kOneshot:
      header.buffering_mode = static_cast<uint8_t>(TRACE_BUFFERING_MODE_ONESHOT);
      break;
    case fuchsia_tracing::BufferingMode::kCircular:
      header.buffering_mode = static_cast<uint8_t>(TRACE_BUFFERING_MODE_CIRCULAR);
      break;
    case fuchsia_tracing::BufferingMode::kStreaming:
      header.buffering_mode = static_cast<uint8_t>(TRACE_BUFFERING_MODE_STREAMING);
      break;
    default:
      break;
  }

  header.total_size = total_buffer_size_;
  header.durable_buffer_size = durable_buffer_size_;
  header.rolling_buffer_size = rolling_buffer_size_;

  buffer_vmo_.write(&header, 0, sizeof(header));
}

void FakeProviderV2::UpdateBufferHeaderAfterStopped() {
  size_t offset = offsetof(trace::internal::trace_buffer_header, rolling_data_end[0]);
  buffer_vmo_.write(reinterpret_cast<uint8_t*>(&buffer_next_), offset, sizeof(uint64_t));
}

void FakeProviderV2::WriteInitRecord() {
  size_t num_words = 2u;
  std::vector<uint64_t> record(num_words);
  record[0] =
      trace::RecordFields::Type::Make(trace::ToUnderlyingType(trace::RecordType::kInitialization)) |
      trace::RecordFields::RecordSize::Make(num_words);
  record[1] = 42;
  WriteRecordToBuffer(reinterpret_cast<uint8_t*>(record.data()), trace::WordsToBytes(num_words));
}

void FakeProviderV2::WriteRecordToBuffer(const uint8_t* data, size_t size) {
  size_t offset;
  switch (buffering_mode_) {
    case fuchsia_tracing::BufferingMode::kOneshot:
      offset = FakeProvider::kHeaderSize + buffer_next_;
      break;
    case fuchsia_tracing::BufferingMode::kCircular:
    case fuchsia_tracing::BufferingMode::kStreaming:
      offset = FakeProvider::kHeaderSize + durable_buffer_size_ + buffer_next_;
      break;
    default:
      FX_NOTREACHED();
      offset = 0;
      break;
  }
  WriteBytes(data, offset, size);
  buffer_next_ += size;
}

void FakeProviderV2::WriteZeroLengthRecord(size_t offset) {
  uint64_t zero_length_record = 0;
  WriteBytes(reinterpret_cast<const uint8_t*>(&zero_length_record), offset,
             sizeof(zero_length_record));
}

void FakeProviderV2::WriteBytes(const uint8_t* data, size_t offset, size_t size) {
  buffer_vmo_.write(data, offset, size);
}

fidl::ClientEnd<fuchsia_tracing_provider::ProviderV2> FakeProviderV2Binding::NewBinding(
    async_dispatcher_t* dispatcher) {
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_tracing_provider::ProviderV2>::Create();
  binding = fidl::BindServer(dispatcher, std::move(server_end), provider.get());
  return std::move(client_end);
}

}  // namespace test
}  // namespace tracing
