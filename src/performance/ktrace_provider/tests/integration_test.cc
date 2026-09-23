// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <fidl/fuchsia.tracing.controller/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zircon-internal/ktrace.h>
#include <lib/zx/socket.h>

#include <gtest/gtest.h>
#include <trace-reader/reader.h>

namespace {

// Continuously sleep to generate a bunch of scheduler records.
std::thread DoWork(zx::unowned_event e) {
  std::thread t{[&]() {
    while (true) {
      zx_signals_t out;
      zx_status_t status = e->wait_one(ZX_EVENT_SIGNALED | ZX_SIGNAL_HANDLE_CLOSED,
                                       zx::deadline_after(zx::msec(1)), &out);
      if ((out & ZX_EVENT_SIGNALED) != 0) {
        return;
      }
      if (status == ZX_ERR_PEER_CLOSED) {
        return;
      }
    }
  }};
  return t;
}

bool WaitForKtrace(const fidl::SyncClient<fuchsia_tracing_controller::Provisioner>& client) {
  // Wait for ktrace to attach
  for (unsigned retries = 0; retries < 5; retries++) {
    fidl::Result<fuchsia_tracing_controller::Provisioner::GetProviders> providers =
        client->GetProviders();
    if (providers.is_error()) {
      return false;
    }
    if (providers->providers().size() == 1) {
      break;
    }
    sleep(1);
  }
  return true;
}

// Check that we are able to connect to kernel tracing and actually get data.
TEST(KtraceProviderIntegrationTest, VerifyData) {
  zx::result client_end = component::Connect<fuchsia_tracing_controller::Provisioner>();
  ASSERT_TRUE(client_end.is_ok());
  fidl::SyncClient client{std::move(*client_end)};

  ASSERT_TRUE(WaitForKtrace(client));

  zx::socket in_socket;
  zx::socket outgoing_socket;
  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);

  auto endpoints = fidl::Endpoints<fuchsia_tracing_controller::Session>::Create();

  fuchsia_tracing_controller::TraceConfig config{{
      // Include lots of trace data so we quickly fill up the buffer
      .categories = {{"kernel:meta", "kernel:sched", "kernel:syscall", "kernel:ipc", "kernel:vm",
                      "kernel:irq"}},
      .buffer_size_megabytes_hint = 1,
      .buffering_mode = fuchsia_tracing::BufferingMode::kStreaming,
  }};
  auto init_response =
      client->InitializeTracing({std::move(endpoints.server), config, std::move(outgoing_socket)});
  ASSERT_TRUE(init_response.is_ok());
  fidl::SyncClient controller_client{std::move(endpoints.client)};

  zx::event e;
  zx::event::create(0, &e);
  // Create some background threads to ensure we get context switch events.
  std::thread worker1 = DoWork(e.borrow());
  std::thread worker2 = DoWork(e.borrow());
  std::thread worker3 = DoWork(e.borrow());
  std::thread worker4 = DoWork(e.borrow());
  controller_client->StartTracing({});

  // Read 256K from the socket
  std::vector<uint8_t> buffer;
  size_t total_read = 0;

  bool trace_stopped = false;
  std::thread stop_thread;

  while (true) {
    buffer.resize(total_read + 4096);
    size_t bytes_read;
    zx_status_t status = in_socket.read(0u, buffer.data() + total_read, 4096, &bytes_read);
    if (status == ZX_ERR_PEER_CLOSED) {
      buffer.resize(total_read);
      break;
    }
    if (status == ZX_ERR_SHOULD_WAIT) {
      zx_status_t result = in_socket.wait_one(ZX_SOCKET_READABLE | ZX_SOCKET_PEER_CLOSED,
                                              zx::deadline_after(zx::sec(30)), nullptr);
      if (result == ZX_ERR_TIMED_OUT) {
        // We traced for 30 seconds, but we may not have had enough data to trigger a buffer flush.
        // That's fine, we'll just stop the trace and get what was written.
        if (!trace_stopped) {
          stop_thread = std::thread{[&]() {
            controller_client->StopTracing({{.write_results = true}});
            controller_client.TakeClientEnd().reset();
          }};
          trace_stopped = true;
        }
        continue;
      }
      if (result == ZX_ERR_PEER_CLOSED) {
        break;
      }
      ASSERT_EQ(ZX_OK, result);
      continue;
    }
    ASSERT_EQ(ZX_OK, status);
    total_read += bytes_read;
    if (total_read >= size_t{256} * 1024) {
      if (!trace_stopped) {
        stop_thread = std::thread{[&]() {
          controller_client->StopTracing({{.write_results = true}});
          controller_client.TakeClientEnd().reset();
        }};
        trace_stopped = true;
      }
    }
  }

  e.signal(0, ZX_EVENT_SIGNALED);

  ASSERT_TRUE(trace_stopped);
  stop_thread.join();

  worker1.join();
  worker2.join();
  worker3.join();
  worker4.join();

  bool found_sched_record = false;
  size_t parse_failures = 0;
  trace::TraceReader reader(
      [&](trace::Record record) {
        if (record.type() == trace::RecordType::kScheduler) {
          found_sched_record = true;
        }
      },
      [&parse_failures](std::string_view error) {
        parse_failures += 1;
        FX_LOGS(ERROR) << "TraceReader error: " << error;
      });

  if (total_read > 0) {
    trace::Chunk chunk(reinterpret_cast<const uint64_t*>(buffer.data()), buffer.size() / 8);
    reader.ReadRecords(chunk);
  }

  EXPECT_TRUE(found_sched_record);
  EXPECT_EQ(parse_failures, size_t{0});
}

// Ensure we are able to pass kernel:retain to get a previous run's data.
TEST(KtraceProviderIntegrationTest, Retain) {
  // We start by manually starting a kernel trace, but not reading the data.
  auto tracing_client_end = component::Connect<fuchsia_kernel::TracingResource>();
  ASSERT_TRUE(tracing_client_end.is_ok());
  auto tracing_result = fidl::SyncClient(std::move(*tracing_client_end))->Get();
  ASSERT_TRUE(tracing_result.is_ok());
  ASSERT_EQ(ZX_OK,
            zx_ktrace_control(tracing_result->resource().get(), KTRACE_ACTION_STOP, 0, nullptr));
  ASSERT_EQ(ZX_OK,
            zx_ktrace_control(tracing_result->resource().get(), KTRACE_ACTION_REWIND, 0, nullptr));
  ASSERT_EQ(ZX_OK, zx_ktrace_control(tracing_result->resource().get(), KTRACE_ACTION_START,
                                     KTRACE_GRP_SYSCALL, nullptr));
  // Do syscalls to ensure we get some data
  for (size_t i = 0; i < 10000; i++) {
    usleep(1);
  }
  ASSERT_EQ(ZX_OK,
            zx_ktrace_control(tracing_result->resource().get(), KTRACE_ACTION_STOP, 0, nullptr));

  // Now, we should be able to read out only syscall records using kernel:retain.
  zx::result client_end = component::Connect<fuchsia_tracing_controller::Provisioner>();
  ASSERT_TRUE(client_end.is_ok());
  fidl::SyncClient client{std::move(*client_end)};

  ASSERT_TRUE(WaitForKtrace(client));

  zx::socket in_socket;
  zx::socket outgoing_socket;
  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);

  auto endpoints = fidl::Endpoints<fuchsia_tracing_controller::Session>::Create();

  fuchsia_tracing_controller::TraceConfig config{{
      .categories = {{"kernel:retain"}},
      .buffer_size_megabytes_hint = 1,
      .buffering_mode = fuchsia_tracing::BufferingMode::kOneshot,
  }};
  auto init_response =
      client->InitializeTracing({std::move(endpoints.server), config, std::move(outgoing_socket)});
  ASSERT_TRUE(init_response.is_ok());
  fidl::SyncClient controller_client{std::move(endpoints.client)};

  controller_client->StartTracing({});
  sleep(1);
  std::thread stop_thread{[&]() {
    controller_client->StopTracing({{.write_results = true}});
    controller_client.TakeClientEnd().reset();
  }};

  std::vector<uint8_t> buffer;
  size_t total_read = 0;
  while (true) {
    buffer.resize(total_read + 4096);
    size_t bytes_read;
    zx_status_t status = in_socket.read(0u, buffer.data() + total_read, 4096, &bytes_read);
    if (status == ZX_ERR_PEER_CLOSED) {
      buffer.resize(total_read);
      break;
    }
    if (status == ZX_ERR_SHOULD_WAIT) {
      zx_status_t result = in_socket.wait_one(ZX_SOCKET_READABLE | ZX_SOCKET_PEER_CLOSED,
                                              zx::deadline_after(zx::sec(30)), nullptr);
      ASSERT_EQ(ZX_OK, result);
      continue;
    }
    ASSERT_EQ(ZX_OK, status);
    total_read += bytes_read;
  }
  stop_thread.join();

  bool found_syscall_record = false;
  bool found_non_syscall_record = false;
  size_t parse_failures = 0;
  trace::TraceReader reader(
      [&](trace::Record record) {
        switch (record.type()) {
          case trace::RecordType::kEvent: {
            const trace::Record::Event& data = record.GetEvent();
            if (data.category == "kernel:syscall") {
              found_syscall_record = true;
            } else {
              FX_LOGS(ERROR) << "Got record of category: " << data.category;
              found_non_syscall_record = true;
            }
            break;
          }
          case trace::RecordType::kMetadata:
          case trace::RecordType::kInitialization:
          case trace::RecordType::kString:
          case trace::RecordType::kThread:
          case trace::RecordType::kKernelObject:
            break;
          default: {
            FX_LOGS(ERROR) << "Got record of type: " << static_cast<uint64_t>(record.type());
            found_non_syscall_record = true;
          }
        }
      },
      [&parse_failures](std::string_view error) {
        parse_failures += 1;
        FX_LOGS(ERROR) << "TraceReader error: " << error;
      });

  if (total_read > 0) {
    trace::Chunk chunk(reinterpret_cast<const uint64_t*>(buffer.data()), buffer.size() / 8);
    reader.ReadRecords(chunk);
  }

  EXPECT_TRUE(found_syscall_record);
  EXPECT_FALSE(found_non_syscall_record);
  EXPECT_EQ(parse_failures, size_t{0});
}
}  // namespace
