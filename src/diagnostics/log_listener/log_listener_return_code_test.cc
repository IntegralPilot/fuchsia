// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.diagnostics.host/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/namespace.h>
#include <lib/fdio/spawn.h>
#include <lib/zx/process.h>
#include <stdlib.h>
#include <unistd.h>
#include <zircon/process.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>

#include <chrono>
#include <condition_variable>
#include <cstring>
#include <mutex>
#include <thread>
#include <vector>

#include <gtest/gtest.h>

#include "src/lib/files/file.h"

static constexpr char kLogListenerPath[] = "/pkg/bin/log_listener";

class MockArchiveAccessor : public fidl::Server<fuchsia_diagnostics_host::ArchiveAccessor> {
 public:
  void StreamDiagnostics(StreamDiagnosticsRequest& request,
                         StreamDiagnosticsCompleter::Sync& completer) override {
    std::lock_guard<std::mutex> lock(mutex_);
    socket_ = std::move(request.stream());
    has_connection_ = true;
    completer.Reply();
    cv_.notify_all();
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_diagnostics_host::ArchiveAccessor> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  void WaitForConnection() {
    std::unique_lock<std::mutex> lock(mutex_);
    cv_.wait(lock, [this] { return has_connection_; });
  }

  zx::socket take_socket() {
    std::lock_guard<std::mutex> lock(mutex_);
    return std::move(socket_);
  }

 private:
  std::mutex mutex_;
  std::condition_variable cv_;
  zx::socket socket_;
  bool has_connection_ = false;
};

struct TestContext {
  TestContext(std::shared_ptr<MockArchiveAccessor> accessor)
      : dispatcher(async_get_default_dispatcher()),
        outgoing_dir(std::make_unique<component::OutgoingDirectory>(dispatcher)),
        accessor(std::move(accessor)) {
    ZX_ASSERT(dispatcher != nullptr);
  }

  void Init(fidl::ServerEnd<fuchsia_io::Directory> server_end) {
    zx::result<> status =
        outgoing_dir->AddUnmanagedProtocol<fuchsia_diagnostics_host::ArchiveAccessor>(
            bindings.CreateHandler(accessor.get(), dispatcher, fidl::kIgnoreBindingClosure));
    ZX_ASSERT(status.is_ok());
    ZX_ASSERT(outgoing_dir->Serve(std::move(server_end)).is_ok());
  }

  async_dispatcher_t* dispatcher;
  std::unique_ptr<component::OutgoingDirectory> outgoing_dir;
  std::shared_ptr<MockArchiveAccessor> accessor;
  fidl::ServerBindingGroup<fuchsia_diagnostics_host::ArchiveAccessor> bindings;
};

TEST(LogListenerReturnCode, ReturnNonzeroOnBadArgs) {
  // Spawn log_listener with bad args
  uint32_t flags = FDIO_SPAWN_CLONE_ALL;
  const char* argv[] = {kLogListenerPath, "very", "invalid", "arguments", NULL};
  zx::process process;
  zx_status_t status =
      fdio_spawn(ZX_HANDLE_INVALID, flags, kLogListenerPath, argv, process.reset_and_get_address());
  ASSERT_EQ(ZX_OK, status);

  // Verify return code
  status = process.wait_one(ZX_TASK_TERMINATED, zx::time::infinite(), nullptr);
  ASSERT_EQ(ZX_OK, status);
  zx_info_process_t proc_info;
  status = process.get_info(ZX_INFO_PROCESS, &proc_info, sizeof(proc_info), nullptr, nullptr);
  ASSERT_EQ(ZX_OK, status);
  ASSERT_NE(0, proc_info.return_code);
}

TEST(LogListenerReturnCode, NoCrashWhenStdioClosed) {
  // This test verifies that `log_listener` exits cleanly (return code 0) and does not crash
  // (panic) when its stdout/stderr pipes are closed while it is running. This simulates
  // a user terminating a `component explore` session (e.g. via Ctrl+C) while `log` is active.
  //
  // Steps:
  // 1. Spawn `log_listener` as a child process.
  // 2. Redirect the child's stdout/stderr to pipes.
  // 3. Reconstruct the child's namespace to override `/svc` with a mock directory.
  // 4. Serve a mock `ArchiveAccessor` in that mock `/svc` on a background thread.
  // 5. Wait for `log_listener` to start and connect to the mock service.
  // 6. Close the stdout/stderr pipe read ends (triggering BrokenPipe in the child).
  // 7. Close the log socket (triggering EOF in the child).
  // 8. Verify the child terminates and returns exit code 0.

  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  ASSERT_EQ(ZX_OK, loop.StartThread("fidl-thread"));

  auto [outgoing_client, outgoing_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto mock_accessor = std::make_shared<MockArchiveAccessor>();

  async_patterns::TestDispatcherBound<TestContext> context{loop.dispatcher()};
  context.emplace(mock_accessor);
  context.SyncCall(&TestContext::Init, std::move(outgoing_server));

  // Open the "svc" directory
  zx::result svc_local = component::OpenDirectoryAt(outgoing_client, "svc");
  ASSERT_TRUE(svc_local.is_ok());

  // Create pipes for stdout/stderr
  int stdout_pipe[2];
  int stderr_pipe[2];
  ASSERT_EQ(0, pipe(stdout_pipe));
  ASSERT_EQ(0, pipe(stderr_pipe));

  // Export current namespace to manually clone it without /svc
  fdio_flat_namespace_t* flat_ns = nullptr;
  ASSERT_EQ(ZX_OK, fdio_ns_export_root(&flat_ns));

  std::vector<fdio_spawn_action_t> actions;

  // Add all entries from flat_ns except /svc
  for (size_t i = 0; i < flat_ns->count; ++i) {
    if (strcmp(flat_ns->path[i], "/svc") == 0) {
      continue;
    }
    fdio_spawn_action_t action = {
        .action = FDIO_SPAWN_ACTION_ADD_NS_ENTRY,
        .ns =
            {
                .prefix = flat_ns->path[i],
                .handle = flat_ns->handle[i],
            },
    };
    flat_ns->handle[i] = ZX_HANDLE_INVALID;  // Transfer ownership
    actions.push_back(action);
  }

  // Add our mock /svc
  fdio_spawn_action_t svc_action = {
      .action = FDIO_SPAWN_ACTION_ADD_NS_ENTRY,
      .ns =
          {
              .prefix = "/svc",
              .handle = svc_local->TakeChannel().release(),
          },
  };
  actions.push_back(svc_action);

  // Add stdout/stderr pipes
  fdio_spawn_action_t stdout_action = {
      .action = FDIO_SPAWN_ACTION_TRANSFER_FD,
      .fd =
          {
              .local_fd = stdout_pipe[1],
              .target_fd = STDOUT_FILENO,
          },
  };
  actions.push_back(stdout_action);

  // Note: We don't close the read end of stderr_pipe immediately, so we can capture panic logs.
  fdio_spawn_action_t stderr_action = {
      .action = FDIO_SPAWN_ACTION_TRANSFER_FD,
      .fd =
          {
              .local_fd = stderr_pipe[1],
              .target_fd = STDERR_FILENO,
          },
  };
  actions.push_back(stderr_action);

  // Spawn log_listener with our manually constructed namespace and pipes
  uint32_t flags = FDIO_SPAWN_CLONE_JOB | FDIO_SPAWN_DEFAULT_LDSVC | FDIO_SPAWN_CLONE_ENVIRON |
                   FDIO_SPAWN_CLONE_UTC_CLOCK;
  const char* argv[] = {kLogListenerPath, NULL};
  zx::process process;
  char err_msg[FDIO_SPAWN_ERR_MSG_MAX_LENGTH];
  zx_status_t spawn_status =
      fdio_spawn_etc(ZX_HANDLE_INVALID, flags, kLogListenerPath, argv, NULL, actions.size(),
                     actions.data(), process.reset_and_get_address(), err_msg);

  // Free flat_ns (closes any remaining handles, like the skipped /svc)
  fdio_ns_free_flat_ns(flat_ns);

  ASSERT_EQ(ZX_OK, spawn_status) << "fdio_spawn_etc failed: " << err_msg;

  // Start a thread to read from stderr and print it.
  std::thread stderr_reader([fd = stderr_pipe[0]]() {
    char buf[1024];
    while (true) {
      ssize_t n = read(fd, buf, sizeof(buf) - 1);
      if (n <= 0) {
        break;
      }
      buf[n] = '\0';
      printf("[Child Stderr] %s", buf);
      fflush(stdout);
    }
    close(fd);
  });

  // Wait for connection to mock using condition variable (indefinitely)
  mock_accessor->WaitForConnection();

  // Close stdout read end to trigger BrokenPipe on stdout in the child
  close(stdout_pipe[0]);

  // Close the socket to trigger EOF on the read loop in the child
  {
    zx::socket socket = mock_accessor->take_socket();
    // socket goes out of scope and is closed here
  }

  // Verify return code (wait indefinitely for termination)
  zx_status_t wait_status = process.wait_one(ZX_TASK_TERMINATED, zx::time::infinite(), nullptr);
  ASSERT_EQ(ZX_OK, wait_status);

  zx_info_process_t proc_info;
  zx_status_t info_status =
      process.get_info(ZX_INFO_PROCESS, &proc_info, sizeof(proc_info), nullptr, nullptr);
  ASSERT_EQ(ZX_OK, info_status);

  // Join the reader thread
  stderr_reader.join();

  // It should exit with 0 (clean exit after ignoring BrokenPipe on stdout)
  EXPECT_EQ(0, proc_info.return_code);
}
