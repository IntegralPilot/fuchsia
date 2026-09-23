// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.tracing.controller/cpp/fidl.h>
#include <fidl/fuchsia.tracing/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/observer.h>
#include <lib/zx/socket.h>
#include <unistd.h>

#include <latch>
#include <thread>

#include <perftest/perftest.h>

#include "zircon/system/public/zircon/syscalls.h"

namespace {

template <size_t amount>
void Drain(const zx::unowned_socket& socket, std::latch& done) {
  char buffer[4096];
  size_t actual = 0;
  size_t total = 0;
  bool decremented = false;
  while (zx_status_t result =
             socket->read(0, buffer, sizeof(buffer), &actual) != ZX_ERR_PEER_CLOSED) {
    if (result == ZX_ERR_SHOULD_WAIT) {
      zx_signals_t signals;
      zx_status_t wait_result =
          socket->wait_one(ZX_SOCKET_READABLE, zx::time::infinite(), &signals);
      if (wait_result != ZX_OK) {
        break;
      }
    } else {
      total += actual;
      if (!decremented && total > amount) {
        done.count_down();
        decremented = true;
      }
    }
  }
}

template <size_t amount>
bool StreamingModeThroughput(perftest::RepeatState* state) {
  state->DeclareStep("setup");
  state->DeclareStep("read_data");
  state->DeclareStep("cleanup");

  zx::result provisioner_client = component::Connect<fuchsia_tracing_controller::Provisioner>();
  if (provisioner_client.is_error()) {
    return false;
  }

  auto provisioner = fidl::SyncClient{std::move(*provisioner_client)};
  // Wait for the tracee to connect if needed.
  for (;;) {
    auto res = provisioner->GetProviders();
    FX_CHECK(res.is_ok());
    if (res.value().providers().size() == 1) {
      break;
    }
    usleep(100000);
  }

  while (state->KeepRunning()) {
    zx::socket in_socket;
    zx::socket outgoing_socket;
    FX_CHECK(zx::socket::create(0u, &in_socket, &outgoing_socket) == ZX_OK);
    std::latch drained{1};
    std::thread drainer{
        [socket = in_socket.borrow(), &drained]() { Drain<amount>(socket, drained); }};

    auto [client_end, server_end] = fidl::Endpoints<fuchsia_tracing_controller::Session>::Create();
    FX_CHECK(
        provisioner
            ->InitializeTracing({std::move(server_end),
                                 {{
                                     .categories = std::vector<std::string>{"benchmark"},
                                     .buffer_size_megabytes_hint = uint32_t{64},
                                     .buffering_mode = fuchsia_tracing::BufferingMode::kStreaming,
                                 }},
                                 std::move(outgoing_socket)})
            .is_ok());
    {
      auto client = fidl::SyncClient{std::move(client_end)};

      FX_CHECK(client->StartTracing({}).is_ok());
      state->NextStep();

      drained.wait();
      state->NextStep();

      FX_CHECK(client->StopTracing({{.write_results = {false}}}).is_ok());
    }

    drainer.join();
  }

  return true;
}

void RegisterTests() {
  constexpr size_t OneGiB = size_t{1024} * 1024 * 1024;
  perftest::RegisterTest("StreamingModeThroughput/1GiB", StreamingModeThroughput<OneGiB>);
}

PERFTEST_CTOR(RegisterTests)
}  // namespace

int main(int argc, char** argv) {
  const char* test_suite = "fuchsia.trace_system";
  return perftest::PerfTestMain(argc, argv, test_suite);
}
