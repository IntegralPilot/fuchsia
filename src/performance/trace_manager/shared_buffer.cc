// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace_manager/shared_buffer.h"

#include <fidl/fuchsia.tracing/cpp/common_types.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-engine/types.h>

#include "src/performance/trace_manager/buffer_forwarder.h"

const char* ModeName(fuchsia_tracing::BufferingMode mode) {
  switch (mode) {
    case fuchsia_tracing::BufferingMode::kOneshot:
      return "oneshot";
    case fuchsia_tracing::BufferingMode::kCircular:
      return "circular";
    case fuchsia_tracing::BufferingMode::kStreaming:
      return "streaming";
    default:
      return "unknown";
  }
}

namespace {

// The size of the initialization record.
constexpr size_t kInitRecordSizeBytes = 16;

// Given |wrapped_count|, return the corresponding buffer number.
int32_t GetBufferNumber(uint32_t wrapped_count) { return static_cast<int32_t>(wrapped_count & 1); }

fuchsia_tracing::BufferingMode EngineBufferingModeToProviderMode(trace_buffering_mode_t mode) {
  switch (mode) {
    case TRACE_BUFFERING_MODE_ONESHOT:
      return fuchsia_tracing::BufferingMode::kOneshot;
    case TRACE_BUFFERING_MODE_CIRCULAR:
      return fuchsia_tracing::BufferingMode::kCircular;
    case TRACE_BUFFERING_MODE_STREAMING:
      return fuchsia_tracing::BufferingMode::kStreaming;
    default:
      __UNREACHABLE;
  }
}
}  // namespace

SharedBuffer::SharedBuffer(zx::vmo vmo, std::string provider_name,
                           fuchsia_tracing::BufferingMode buffering_mode, size_t buffer_size,
                           uint32_t provider_id)
    : vmo_(std::move(vmo)),
      provider_name_(std::move(provider_name)),
      buffering_mode_(buffering_mode),
      buffer_size_(buffer_size),
      provider_id_(provider_id) {}

zx::result<SharedBuffer> SharedBuffer::Create(size_t buffer_size,
                                              fuchsia_tracing::BufferingMode buffering_mode,
                                              std::string provider_name, uint32_t provider_id) {
  zx::vmo buffer_vmo;
  if (zx_status_t status = zx::vmo::create(buffer_size, 0u, &buffer_vmo); status != ZX_OK) {
    FX_PLOGS(ERROR, status) << std::format("{}: Failed to create trace buffer", provider_name);
    return zx::error(status);
  }

  char vmo_name[ZX_MAX_NAME_LEN];
  size_t would_write = snprintf(vmo_name, ZX_MAX_NAME_LEN, "trace:%s", provider_name.c_str());
  size_t name_length = std::min(ZX_MAX_NAME_LEN, would_write);
  if (zx_status_t status = buffer_vmo.set_property(ZX_PROP_NAME, vmo_name, name_length);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << std::format("{}: Failed to set name of trace buffer", provider_name);
    return zx::error(status);
  }
  return zx::ok(SharedBuffer{std::move(buffer_vmo), std::move(provider_name), buffering_mode,
                             buffer_size, provider_id});
}

bool SharedBuffer::VerifyBufferHeader(const trace::internal::BufferHeaderReader* header) const {
  fuchsia_tracing::BufferingMode converted_mode =
      EngineBufferingModeToProviderMode(header->buffering_mode());
  if (converted_mode != buffering_mode_) {
    FX_LOGS(ERROR) << std::format("{}: header corrupt, wrong buffering mode: {}", provider_name_,
                                  ModeName(converted_mode));
    return false;
  }
  return true;
}

zx::unowned_vmo SharedBuffer::Vmo() const { return vmo_.borrow(); }
fuchsia_tracing::BufferingMode SharedBuffer::BufferingMode() const { return buffering_mode_; }

tracing::TransferStatus SharedBuffer::WriteChunk(
    const std::shared_ptr<const tracing::BufferForwarder>& output, uint64_t offset, uint64_t last,
    uint64_t end, uint64_t buffer_size) const {
  ZX_DEBUG_ASSERT(last <= buffer_size);
  ZX_DEBUG_ASSERT(end <= buffer_size);
  ZX_DEBUG_ASSERT(end == 0 || last <= end);
  offset += last;
  if (buffering_mode_ == fuchsia_tracing::BufferingMode::kOneshot ||
      // If end is zero then the header wasn't updated when tracing stopped.
      end == 0) {
    uint64_t size = buffer_size - last;
    return output->WriteChunkBy(tracing::BufferForwarder::ForwardStrategy::Records, vmo_, offset,
                                size);
  }
  uint64_t size = end - last;
  return output->WriteChunkBy(tracing::BufferForwarder::ForwardStrategy::Size, vmo_, offset, size);
}

std::pair<tracing::TransferStatus, TransferStats> SharedBuffer::TransferAll(
    const std::shared_ptr<const tracing::BufferForwarder>& output) const {
  if (auto transfer_status = WriteProviderIdRecord(output);
      transfer_status != tracing::TransferStatus::kComplete) {
    FX_LOGS(ERROR) << std::format("{}: Failed to write provider info record to trace.",
                                  provider_name_);
    return {transfer_status, {}};
  }

  trace::internal::trace_buffer_header header_buffer;
  if (vmo_.read(&header_buffer, 0, sizeof(header_buffer)) != ZX_OK) {
    FX_LOGS(ERROR) << std::format("{}: Failed to read header from buffer_vmo", provider_name_);
    return {tracing::TransferStatus::kProviderError, {}};
  }

  std::unique_ptr<trace::internal::BufferHeaderReader> header;
  std::string error =
      trace::internal::BufferHeaderReader::Create(&header_buffer, buffer_size_, &header);
  if (error != "") {
    FX_LOGS(ERROR) << std::format("{}: header corrupt, {}", provider_name_, error);
    return {tracing::TransferStatus::kProviderError, {}};
  }
  if (!VerifyBufferHeader(header.get())) {
    return {tracing::TransferStatus::kProviderError, {}};
  }

  if (header->num_records_dropped() > 0) {
    FX_LOGS(WARNING) << std::format("{}: {} records were dropped", provider_name_,
                                    header->num_records_dropped());
    // If we can't write the buffer overflow record, it's not the end of the
    // world.
    if (output->WriteProviderBufferOverflowEvent(provider_id_) !=
        tracing::TransferStatus::kComplete) {
      FX_LOGS(DEBUG) << std::format(
          "{}: Failed to write provider event (buffer overflow) record to trace.", provider_name_);
    }
  }

  if (buffering_mode_ != fuchsia_tracing::BufferingMode::kOneshot) {
    uint64_t offset = trace::internal::BufferHeaderReader::get_durable_buffer_offset();
    uint64_t last = last_durable_data_end_;
    uint64_t end = header->durable_data_end();
    uint64_t buffer_size = header->durable_buffer_size();
    FX_LOGS(DEBUG) << std::format("Writing durable buffer for {}", provider_name_);
    if (auto transfer_status = WriteChunk(output, offset, last, end, buffer_size);
        transfer_status != tracing::TransferStatus::kComplete) {
      return {transfer_status, {}};
    }
  }

  // There's only two buffers, thus the earlier one is not the current one.
  // It's important to process them in chronological order on the off
  // chance that the earlier buffer provides a stringref or threadref
  // referenced by the later buffer.
  //
  // We want to handle the case of still capturing whatever records we can if
  // the process crashes, in which case the header won't be up to date. In
  // oneshot mode we're covered: We run through the records and see what's
  // there. In circular and streaming modes after a buffer gets reused we can't
  // do that. But if the process crashes it may be the last trace records that
  // are important: we don't want to lose them. As a compromise, if the header
  // is marked as valid use it. Otherwise run through the buffer to count the
  // records we see.

  auto write_rolling_chunk = [this, output,
                              &header](int32_t buffer_number) -> tracing::TransferStatus {
    uint64_t offset = header->GetRollingBufferOffset(buffer_number);
    uint64_t last = 0;
    uint64_t end = header->rolling_data_end(buffer_number);
    uint64_t buffer_size = header->rolling_buffer_size();
    auto name = buffer_number == 0 ? "rolling buffer 0" : "rolling buffer 1";
    FX_LOGS(DEBUG) << "Writing chunks for " << name;
    return WriteChunk(output, offset, last, end, buffer_size);
  };

  if (header->wrapped_count() > 0) {
    int32_t buffer_number = GetBufferNumber(header->wrapped_count() - 1);
    if (buffering_mode_ != fuchsia_tracing::BufferingMode::kStreaming) {
      // In non streaming modes, we haven't transferred any data yet, so we always need to transfer
      // the non active buffer
      if (auto transfer_status = write_rolling_chunk(buffer_number);
          transfer_status != tracing::TransferStatus::kComplete) {
        return {transfer_status, {}};
      }
    } else if (last_wrapped_count_ < header->wrapped_count() - 1) {
      // Otherwise, in streaming mode, only write the previous buffer if our local record indicates
      // that we haven't transferred this version of it yet.
      if (auto transfer_status = write_rolling_chunk(buffer_number);
          transfer_status != tracing::TransferStatus::kComplete) {
        return {transfer_status, {}};
      }
    }
  }
  int32_t buffer_number = GetBufferNumber(header->wrapped_count());
  if (auto transfer_status = write_rolling_chunk(buffer_number);
      transfer_status != tracing::TransferStatus::kComplete) {
    return {transfer_status, {}};
  }

  uint8_t durable_buffer_used = 0;
  if (header->durable_buffer_size() > 0) {
    durable_buffer_used =
        static_cast<uint8_t>(100 * header->durable_data_end() / header->durable_buffer_size());
  }
  TransferStats stats{
      .durable_used_percent = durable_buffer_used,
      .wrapped_count = header->wrapped_count(),
      .records_dropped = header->num_records_dropped(),
      .non_durable_bytes_written = header->rolling_data_end(0) + header->rolling_data_end(1),
  };

  return {tracing::TransferStatus::kComplete, stats};
}

tracing::TransferStatus SharedBuffer::WriteProviderIdRecord(
    const std::shared_ptr<const tracing::BufferForwarder>& output) const {
  if (provider_info_record_written_) {
    return output->WriteProviderSectionRecord(provider_id_);
  }
  auto status = output->WriteProviderInfoRecord(provider_id_, provider_name_);
  provider_info_record_written_ = true;
  return status;
}

bool SharedBuffer::StreamingTransfer(const std::shared_ptr<const tracing::BufferForwarder>& output,
                                     uint32_t wrapped_count, uint64_t durable_data_end) {
  FX_DCHECK(buffering_mode_ == fuchsia_tracing::BufferingMode::kStreaming);
  if (wrapped_count == 0 && last_wrapped_count_ == 0) {
    // ok
  } else if (wrapped_count != last_wrapped_count_ + 1) {
    FX_LOGS(ERROR) << std::format("{}: unexpected wrapped_count from provider: {}", provider_name_,
                                  wrapped_count);
    return false;
  } else if (durable_data_end < last_durable_data_end_ || (durable_data_end & 7) != 0) {
    FX_LOGS(ERROR) << std::format("{}: unexpected durable_data_end from provider: {}",
                                  provider_name_, durable_data_end);
    return false;
  }

  int32_t buffer_number = GetBufferNumber(wrapped_count);

  if (WriteProviderIdRecord(output) != tracing::TransferStatus::kComplete) {
    FX_LOGS(ERROR) << std::format("{}: Failed to write provider section record to trace.",
                                  provider_name_);
    return false;
  }

  trace::internal::trace_buffer_header header_buffer;
  if (vmo_.read(&header_buffer, 0, sizeof(header_buffer)) != ZX_OK) {
    FX_LOGS(ERROR) << std::format("{}: Failed to read header from vmo_", provider_name_);
    return false;
  }

  std::unique_ptr<trace::internal::BufferHeaderReader> header;
  std::string error =
      trace::internal::BufferHeaderReader::Create(&header_buffer, buffer_size_, &header);
  if (error != "") {
    FX_LOGS(ERROR) << std::format("{}: header corrupt, {}", provider_name_, error);
    return false;
  }
  if (!VerifyBufferHeader(header.get())) {
    return false;
  }

  FX_LOGS(DEBUG) << "Dropped records: " << header->num_records_dropped();

  // Don't use |header.durable_data_end| here, we want the value at the time
  // the message was sent.
  if (durable_data_end < kInitRecordSizeBytes || durable_data_end > header->durable_buffer_size() ||
      (durable_data_end & 7) != 0 || durable_data_end < last_durable_data_end_) {
    FX_LOGS(ERROR) << std::format("{}: bad durable_data_end: {}", provider_name_, durable_data_end);
    return false;
  }

  // However we can use rolling_data_end from the header.
  // This buffer is no longer being written to until we save it.
  // [And if it does get written to it'll potentially result in corrupt
  // data, but that's not our problem; as long as we can't crash, which is
  // always the rule here.]
  uint64_t rolling_data_end = header->rolling_data_end(buffer_number);

  // Only transfer what's new in the durable buffer since the last time.
  uint64_t durable_buffer_offset = trace::internal::BufferHeaderReader::get_durable_buffer_offset();
  if (durable_data_end > last_durable_data_end_) {
    uint64_t size = durable_data_end - last_durable_data_end_;
    FX_LOGS(DEBUG) << std::format("Writing durable buffer for {}", provider_name_);
    if (output->WriteChunkBy(tracing::BufferForwarder::ForwardStrategy::Size, vmo_,
                             durable_buffer_offset + last_durable_data_end_,
                             size) != tracing::TransferStatus::kComplete) {
      return false;
    }
  }

  uint64_t buffer_offset = header->GetRollingBufferOffset(buffer_number);
  auto buffer_name = buffer_number == 0 ? "rolling buffer 0" : "rolling buffer 1";
  FX_LOGS(DEBUG) << std::format("Writing {} for {}", buffer_name, provider_name_);
  tracing::TransferStatus transfer_status = output->WriteChunkBy(
      tracing::BufferForwarder::ForwardStrategy::Size, vmo_, buffer_offset, rolling_data_end);
  if (transfer_status != tracing::TransferStatus::kComplete) {
    return false;
  }

  last_wrapped_count_ = wrapped_count;
  last_durable_data_end_ = durable_data_end;
  return true;
}
