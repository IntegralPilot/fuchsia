// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.tracing.controller/cpp/fidl.h>
#include <fidl/fuchsia.tracing/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/socket.h>
#include <stdlib.h>

#include <gtest/gtest.h>
#include <trace-test-utils/read_records.h>

TEST(PerfettoBridgeIntegrationTest, Init) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  zx::result client_end = component::Connect<fuchsia_tracing_controller::Provisioner>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  // Wait for perfetto bridge to attach
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
      .buffer_size_megabytes_hint = uint32_t{4},
      .buffering_mode = fuchsia_tracing::BufferingMode::kOneshot,
  }};

  fidl::Result<fuchsia_tracing_controller::Provisioner::GetProviders> providers =
      client->GetProviders();

  // We should have a single provider: perfetto
  ASSERT_TRUE(providers.is_ok());
  ASSERT_EQ(providers->providers().size(), size_t{1});
  ASSERT_EQ(providers->providers().begin()->name(), "perfetto");

  auto endpoints = fidl::Endpoints<fuchsia_tracing_controller::Session>::Create();
  auto init_response =
      client->InitializeTracing({std::move(endpoints.server), config, std::move(outgoing_socket)});
  ASSERT_TRUE(init_response.is_ok());
  const fidl::SyncClient controller_client{std::move(endpoints.client)};

  controller_client->StartTracing({});
  loop.Run(zx::deadline_after(zx::sec(1)));
  controller_client->StopTracing({{.write_results = true}});

  size_t num_perfetto_bytes = 0;
  trace::TraceReader::RecordConsumer handle_perfetto_blob =
      [&num_perfetto_bytes](trace::Record record) {
        if (record.type() == trace::RecordType::kBlob &&
            record.GetBlob().type == TRACE_BLOB_TYPE_PERFETTO) {
          num_perfetto_bytes += record.GetBlob().blob_size;
        }
      };
  trace::TraceReader reader(std::move(handle_perfetto_blob), [](fbl::String&&) {});

  constexpr size_t kBufferSize = 4096;
  uint8_t buffer[kBufferSize];
  uint8_t* buffer_base = buffer;
  size_t leftover_bytes = 0;
  for (;;) {
    size_t actual = 0;
    zx_status_t read_result =
        in_socket.read(0, buffer_base, sizeof(buffer) - leftover_bytes, &actual);
    EXPECT_TRUE(read_result == ZX_OK || read_result == ZX_ERR_SHOULD_WAIT);
    if (read_result == ZX_ERR_SHOULD_WAIT || actual == 0) {
      EXPECT_EQ(leftover_bytes, size_t{0});
      break;
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

  EXPECT_GT(num_perfetto_bytes, size_t{30000});
  loop.RunUntilIdle();
}

// TODO(https://fxbug.dev/42071555): Add a test to cover calls to Controller::GetKnownCategories
// once perfetto_producer.cc has a working backend. This would test the plumbing all the way from
// perfetto to ffx trace. TEST(PerfettoBridgeIntegrationTest, GetKnownCategories)
