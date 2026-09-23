// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.tracing.controller/cpp/fidl.h>
#include <fidl/fuchsia.tracing/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-engine/context.h>
#include <lib/trace-engine/instrumentation.h>
#include <lib/trace-provider/provider.h>
#include <lib/zx/socket.h>

#include <gtest/gtest.h>

// Check that when we flush our events, we correctly read them back
TEST(AllInOneTest, Flush) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  loop.StartThread();

  async_dispatcher_t* dispatcher = loop.dispatcher();
  trace::TraceProviderWithFdio trace_provider(dispatcher, "test_provider");

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

  auto endpoints = fidl::Endpoints<fuchsia_tracing_controller::Session>::Create();
  auto init_response =
      client->InitializeTracing({std::move(endpoints.server), config, std::move(outgoing_socket)});
  ASSERT_TRUE(init_response.is_ok());

  {
    const fidl::SyncClient controller_client{std::move(endpoints.client)};
    controller_client->StartTracing({});

    uint64_t buffer[128];
    size_t actual;
    // Read out the metadata records
    zx_status_t status = in_socket.read(0, buffer, sizeof(buffer), &actual);
    ASSERT_EQ(status, ZX_OK);

    for (size_t i = 0; i < 10; i++) {
      trace_context_t* context = trace_acquire_context();
      void* bytes = trace_context_alloc_record(context, 8);
      *reinterpret_cast<uint64_t*>(bytes) = i;
      trace_release_context(context);
    }

    status = in_socket.read(0, buffer, sizeof(buffer), &actual);
    // There should still be nothing to read.
    ASSERT_EQ(status, ZX_ERR_SHOULD_WAIT);

    controller_client->FlushBuffers();

    // We should expect
    // 16 bytes: init record
    // 24 bytes: thread record
    // 10 * 8 bytes: our data
    // Total: 120 bytes
    size_t expected = 120;
    in_socket.set_property(ZX_PROP_SOCKET_RX_THRESHOLD, &expected, sizeof(expected));
    zx_signals_t signals;
    in_socket.wait_one(ZX_SOCKET_READ_THRESHOLD, zx::deadline_after(zx::sec(10)), &signals);
    status = in_socket.read(0, buffer, sizeof(buffer), &actual);
    // Now we should see our event.
    ASSERT_EQ(status, ZX_OK);

    // Check to make sure we got our events back
    for (uint64_t i = 0; i < 10; i++) {
      EXPECT_EQ(buffer[i + 5], uint64_t{i});
    }
    controller_client->StopTracing({{.write_results = false}});
  }
  zx_signals_t signals;
  in_socket.wait_one(ZX_SOCKET_PEER_CLOSED, zx::deadline_after(zx::sec(10)), &signals);

  loop.Quit();
  loop.JoinThreads();
}
