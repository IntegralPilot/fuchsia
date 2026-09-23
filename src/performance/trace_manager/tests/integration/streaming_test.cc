// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.tracing.controller/cpp/fidl.h>
#include <fidl/fuchsia.tracing/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/socket.h>
#include <stdlib.h>

#include <gtest/gtest.h>
#include <trace-reader/reader.h>
#include <trace-reader/records.h>

/*
 * This reads 10 MiB of data in streaming mode and checks that the data is valid, in order, and not
 * duplicated.
 */

namespace {
void ConsumeEvents(zx::socket in_socket, zx::eventpair e) {
  size_t most_recent_arg = 0;
  trace::TraceReader::RecordConsumer handle_events = [&most_recent_arg](trace::Record record) {
    if (record.type() == trace::RecordType::kEvent) {
      uint64_t counter = record.GetEvent().arguments[0].value().GetUint64();
      ASSERT_GT(counter, most_recent_arg);
      most_recent_arg = counter;
    }
  };
  trace::TraceReader reader(std::move(handle_events), [](std::string_view) {});

  constexpr size_t kBufferSize = 4096;
  uint8_t buffer[kBufferSize];
  uint8_t* buffer_base = buffer;
  size_t leftover_bytes = 0;
  size_t bytes_read = 0;
  for (;;) {
    size_t actual = 0;
    zx_status_t read_result =
        in_socket.read(0, buffer_base, sizeof(buffer) - leftover_bytes, &actual);
    if (read_result == ZX_ERR_SHOULD_WAIT) {
      zx_signals_t signals;
      in_socket.wait_one(ZX_SOCKET_READABLE | ZX_SIGNAL_HANDLE_CLOSED,
                         zx::deadline_after(zx::sec(1)), &signals);
      continue;
    }
    if (read_result == ZX_ERR_PEER_CLOSED) {
      return;
    }

    bytes_read += actual;

    if (bytes_read > size_t{10} * 1024 * 1024) {
      ASSERT_EQ(ZX_OK, e.signal_peer(0, ZX_USER_SIGNAL_0));
    }

    trace::Chunk data{reinterpret_cast<uint64_t*>(buffer), (actual + leftover_bytes) >> 3};
    reader.ReadRecords(data);

    // trace::Chunk only deals in full words so we may have some bytes we rounded off
    size_t unchunked_bytes = (actual + leftover_bytes) % 8;

    // We may have leftover data in the chunk if our read boundary was not on a record boundary.
    // Copy it back into the buffer for the next round.
    leftover_bytes = (data.remaining_words() * 8) + unchunked_bytes;
    if (leftover_bytes != 0) {
      memcpy(buffer, buffer + data.current_byte_offset(), leftover_bytes);
    }
    buffer_base = buffer + leftover_bytes;
  }
}
}  // namespace

TEST(StreamingTest, Stream) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  zx::result client_end = component::Connect<fuchsia_tracing_controller::Provisioner>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  // Wait for the producer to attach
  for (unsigned retries = 0; retries < 5; retries++) {
    fidl::Result<fuchsia_tracing_controller::Provisioner::GetProviders> providers =
        client->GetProviders();
    ASSERT_TRUE(providers.is_ok());
    if (providers->providers().size() == 1) {
      break;
    }
    sleep(1);
  }
  zx::socket in_socket;
  zx::socket outgoing_socket;

  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);
  const fuchsia_tracing_controller::TraceConfig config{{
      .buffer_size_megabytes_hint = uint32_t{1},
      .buffering_mode = fuchsia_tracing::BufferingMode::kStreaming,
  }};

  fidl::Result<fuchsia_tracing_controller::Provisioner::GetProviders> providers =
      client->GetProviders();

  auto endpoints = fidl::Endpoints<fuchsia_tracing_controller::Session>::Create();
  auto init_response =
      client->InitializeTracing({std::move(endpoints.server), config, std::move(outgoing_socket)});
  ASSERT_TRUE(init_response.is_ok());
  zx::eventpair e1;
  zx::eventpair e2;
  zx::eventpair::create(0, &e1, &e2);
  std::thread t1{ConsumeEvents, std::move(in_socket), std::move(e1)};
  {
    const fidl::SyncClient controller_client{std::move(endpoints.client)};
    controller_client->StartTracing({});
    loop.Run(zx::deadline_after(zx::sec(1)));
    zx_signals_t signals;
    zx_status_t res = e2.wait_one(ZX_USER_SIGNAL_0, zx::deadline_after(zx::sec(10)), &signals);
    ASSERT_EQ(res, ZX_OK);
    ASSERT_TRUE((signals & ZX_USER_SIGNAL_0) != 0);
    controller_client->StopTracing({{.write_results = true}});
    loop.Run(zx::deadline_after(zx::sec(1)));
  }
  t1.join();

  loop.RunUntilIdle();
}
