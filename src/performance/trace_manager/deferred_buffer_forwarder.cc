// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace_manager/deferred_buffer_forwarder.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace-engine/fields.h>

#include <filesystem>
#include <format>

namespace tracing {

namespace {
const char* kTraceDir = "/traces";
}

DeferredBufferForwarder::DeferredBufferForwarder(zx::socket destination,
                                                 size_t transfer_buffer_size,
                                                 size_t backpressure_percentage)
    : BufferForwarder(std::move(destination), backpressure_percentage),
      chunk_buffer_(transfer_buffer_size > 0 ? transfer_buffer_size : kDefaultTransferBufferSize) {
  // In the event where trace_manager was killed while copying a trace, we might have an old file
  // laying around. Remove them just in case.
  std::filesystem::path dir_path = kTraceDir;  // Current directory
  for (auto& p : std::filesystem::directory_iterator(dir_path)) {
    std::filesystem::remove(p);
  }
  std::chrono::time_point now = std::chrono::system_clock::now();
  // TODO(https://fxbug.dev/446873535): Go back to naming these files with a timestamp again.
  // std::string fname = std::format("trace_{}.fxt", now.time_since_epoch().count());
  std::string fname = std::format("trace.fxt", now.time_since_epoch().count());
  buffer_path_ = dir_path / fname;
  buffer_file_ = fopen(buffer_path_.c_str(), "a+");
}

DeferredBufferForwarder::~DeferredBufferForwarder() {
  Flush();
  if (buffer_file_ != nullptr) {
    fclose(buffer_file_);
  }
  // TODO(https://fxbug.dev/446873535): Go back to cleaning up trace files.
  FX_LOGS(WARNING) << "Leaving file at " << buffer_path_ << " https://fxbug.dev/446873535";
  // std::filesystem::remove(buffer_path_);
}
TransferStatus DeferredBufferForwarder::Flush() {
  if (flushed_) {
    return TransferStatus::kComplete;
  }
  FX_LOGS(INFO) << "Flushing trace file to socket: backpressure threshold="
                << backpressure_percentage_
                << "%, transfer batch buffer=" << (chunk_buffer_.size() / 1024) << "KB";
  if (buffer_file_ == nullptr) {
    FX_LOGS(ERROR) << "Failed to open trace file: " << buffer_path_ << " for read!";
    return TransferStatus::kWriteError;
  }
  if (fseek(buffer_file_, 0, SEEK_SET) != 0) {
    FX_LOGS(ERROR) << "Failed to seek to beginning of: " << buffer_path_ << " for read!";
    return TransferStatus::kWriteError;
  }

  for (;;) {
    if (TransferStatus status = WaitForSocketDrain(); status != TransferStatus::kComplete) {
      return status;
    }

    size_t bytes_read =
        fread(chunk_buffer_.data(), sizeof(uint8_t), chunk_buffer_.size(), buffer_file_);
    if (bytes_read == 0) {
      if (ferror(buffer_file_)) {
        FX_LOGS(ERROR) << "Error reading trace file during flush: " << buffer_path_;
        return TransferStatus::kWriteError;
      }
      break;
    }
    if (TransferStatus status = BufferForwarder::WriteBuffer({chunk_buffer_.data(), bytes_read});
        status != TransferStatus::kComplete) {
      return status;
    }
  }
  flushed_ = true;
  return TransferStatus::kComplete;
}

TransferStatus DeferredBufferForwarder::WriteBuffer(cpp20::span<const uint8_t> data) const {
  if (buffer_file_ == nullptr) {
    FX_LOGS(ERROR) << "Failed to open trace file for write: " << buffer_path_;
    return TransferStatus::kWriteError;
  }
  while (!data.empty()) {
    size_t actual = fwrite(data.data(), sizeof(uint8_t), data.size(), buffer_file_);
    data = data.subspan(actual);
  }
  return TransferStatus::kComplete;
}

}  // namespace tracing
