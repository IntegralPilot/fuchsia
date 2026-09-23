// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/diagnostics/fake-log-sink/cpp/fake_log_sink.h"

#include <lib/async-loop/cpp/loop.h>
#include <zircon/assert.h>

#include <sdk/lib/syslog/cpp/log_settings.h>

#include "src/lib/diagnostics/fake-log-sink/rust/fake_log_sink.h"
#include "src/lib/diagnostics/log/message/rust/cpp-log-decoder/log_decoder.h"

namespace fuchsia_logging {

FakeLogSink::FakeLogSink(RawLogSeverity severity,
                         fidl::ServerEnd<fuchsia_logger::LogSink> server_end)
    : impl_(internal::fake_log_sink_new()) {
  SetSeverity(severity);
  if (server_end.is_valid()) {
    internal::fake_log_sink_serve(impl_, server_end.TakeChannel().release());
  } else {
    auto endpoints = fidl::CreateEndpoints<fuchsia_logger::LogSink>();
    ZX_ASSERT(endpoints.is_ok());

    internal::fake_log_sink_serve(impl_, endpoints->server.TakeChannel().release());

    // Create a dispatcher for the global logger.
    static async::Loop* global_loop = [] {
      auto* loop = new async::Loop(&kAsyncLoopConfigNeverAttachToThread);
      loop->StartThread();
      return loop;
    }();

    LogSettingsBuilder builder;
    builder.WithLogSink(endpoints->client.TakeChannel().release())
        .WithDispatcher(global_loop->dispatcher())
        .BuildAndInitialize();
  }
}

FakeLogSink::FakeLogSink(FakeLogSink&& other) : impl_(other.impl_) { other.impl_ = nullptr; }

FakeLogSink& FakeLogSink::operator=(FakeLogSink&& other) {
  if (this == &other) {
    return *this;
  }
  if (impl_) {
    internal::fake_log_sink_delete(impl_);
  }
  impl_ = other.impl_;
  other.impl_ = nullptr;
  return *this;
}

FakeLogSink::~FakeLogSink() {
  if (impl_) {
    internal::fake_log_sink_delete(impl_);
  }
}

bool FakeLogSink::WaitForRecord(zx::time deadline) const {
  return internal::fake_log_sink_wait_for_record(impl_, deadline.get()) > 0;
}

std::vector<uint8_t> FakeLogSink::ReadRecord() {
  uintptr_t record_size = internal::fake_log_sink_wait_for_record(impl_, ZX_TIME_INFINITE);
  ZX_ASSERT(record_size > 0);
  std::vector<uint8_t> buf(record_size);
  internal::fake_log_sink_read_record(impl_, buf.data(), record_size);
  return buf;
}

std::optional<diagnostics::reader::LogsData> FakeLogSink::ReadLogsData() {
  auto record = ReadRecord();
  if (record.empty()) {
    return {};
  }
  auto raw_message = fuchsia_decode_log_message_to_json(record.data(), record.size());
  rapidjson::Document document;
  document.Parse(raw_message);
  fuchsia_free_decoded_log_message(raw_message);
  rapidjson::Document log;
  log.CopyFrom(document.GetArray()[0], log.GetAllocator());
  return diagnostics::reader::LogsData(std::move(log));
}

void FakeLogSink::SetSeverity(RawLogSeverity severity) {
  internal::fake_log_sink_set_min_severity(impl_, severity);
}

}  // namespace fuchsia_logging
