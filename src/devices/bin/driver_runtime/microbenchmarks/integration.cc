// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.runtime.microbenchmarks/cpp/driver/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdf/cpp/env.h>
#include <lib/fdf/testing.h>
#include <lib/fidl_driver/cpp/wire_messaging.h>
#include <lib/fit/defer.h>
#include <lib/sync/cpp/completion.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <perftest/perftest.h>

#include "src/devices/bin/driver_runtime/microbenchmarks/assert.h"

namespace {

class FakeServerImpl : public fdf::WireServer<fuchsia_runtime_microbenchmarks::Device> {
 public:
  void Handshake(fdf::Arena& arena, HandshakeCompleter::Sync& completer) override {
    completer.buffer(arena).Reply(zx::ok());
  }
};

// Measures the time taken to complete a FIDL sync call using the driver transport,
// with no cross-thread wakeups.
bool InlineCallBenchmark(perftest::RepeatState* state) {
  void* client_driver = reinterpret_cast<void*>(1);
  void* server_driver = reinterpret_cast<void*>(2);

  libsync::Completion client_dispatcher_shutdown;
  auto client_dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      client_driver, fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "client",
      [&](fdf_dispatcher_t* dispatcher) { client_dispatcher_shutdown.Signal(); });
  ASSERT_OK(client_dispatcher.status_value());

  libsync::Completion server_dispatcher_shutdown;
  auto server_dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      server_driver, {}, "server",
      [&](fdf_dispatcher_t* dispatcher) { server_dispatcher_shutdown.Signal(); });
  ASSERT_OK(server_dispatcher.status_value());

  // We would like to run in the context of the client dispatcher.
  ASSERT_OK(fdf_testing_set_default_dispatcher(client_dispatcher->get()));
  auto unset = fit::defer([]() { ASSERT_OK(fdf_testing_set_default_dispatcher(nullptr)); });

  auto channels = fdf::ChannelPair::Create(0);
  ASSERT_OK(channels.status_value());

  fdf::ServerEnd<fuchsia_runtime_microbenchmarks::Device> server_end(std::move(channels->end0));
  fdf::ClientEnd<fuchsia_runtime_microbenchmarks::Device> client_end(std::move(channels->end1));

  FakeServerImpl server;
  fdf::BindServer(server_dispatcher->get(), std::move(server_end), &server);

  fdf::WireSyncClient<fuchsia_runtime_microbenchmarks::Device> client(std::move(client_end));

  fdf::Arena arena('BNCH');
  while (state->KeepRunning()) {
    // The Channel::Call will write to the peer channel, which makes an inline call to the server.
    // The server will immediately reply, and the Channel::Call will also read the result inline.
    auto result = client.buffer(arena)->Handshake();
    ZX_ASSERT(result.ok());
    ZX_ASSERT(!result->is_error());
  }

  client_dispatcher->ShutdownAsync();
  client_dispatcher_shutdown.Wait();

  server_dispatcher->ShutdownAsync();
  server_dispatcher_shutdown.Wait();

  return true;
}

void RegisterTests() { perftest::RegisterTest("FidlInlineEmptyCall", InlineCallBenchmark); }

PERFTEST_CTOR(RegisterTests)

}  // namespace
