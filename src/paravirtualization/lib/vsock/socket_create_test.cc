// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/sync/cpp/completion.h>
#include <lib/syslog/cpp/macros.h>
#include <sys/socket.h>

#include <thread>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/paravirtualization/lib/vsock/socket.h"
#include "src/paravirtualization/lib/vsock/vm_sockets.h"

namespace {
constexpr char kClientMessage[] = "Hello World";
constexpr char kServerMessage[] = "Ping Pong";
constexpr unsigned int kCid = VMADDR_CID_LOCAL;
constexpr unsigned int kVsockPort = 31000;
constexpr sockaddr_vm kVsockAddr{
    .svm_family = AF_VSOCK,
    .svm_port = kVsockPort,
    .svm_cid = kCid,
};

void RunServer(libsync::Completion* completion) {
  fbl::unique_fd server_fd;
  zx_status_t status = create_virtio_stream_socket(server_fd.reset_and_get_address());
  ZX_ASSERT(status == ZX_OK);

  int rv =
      bind(server_fd.get(), reinterpret_cast<const sockaddr*>(&kVsockAddr), sizeof(kVsockAddr));
  if (rv != 0) {
    FX_LOGS(FATAL) << "bind failed: " << errno << ": " << strerror(errno);
  }

  rv = listen(server_fd.get(), 1);
  if (rv != 0) {
    FX_LOGS(FATAL) << "listen failed: " << errno << ": " << strerror(errno);
  }
  completion->Signal();

  rv = accept(server_fd.get(), nullptr, nullptr);
  if (rv < 0) {
    FX_LOGS(FATAL) << "accept failed: " << errno << ": " << strerror(errno);
  }

  fbl::unique_fd conn_fd(rv);
  char buffer[1024] = {};
  ssize_t bytes = recv(conn_fd.get(), buffer, sizeof(buffer), 0);
  if (bytes < 0) {
    FX_LOGS(FATAL) << "recv failed: " << errno << ": " << strerror(errno);
  }
  if (strncmp(buffer, kClientMessage, std::min(static_cast<size_t>(rv), sizeof(kClientMessage))) !=
      0) {
    FX_LOGS(FATAL) << "Received from client: " << buffer << " expected: " << kClientMessage;
  }

  bytes = send(conn_fd.get(), kServerMessage, sizeof(kServerMessage), 0);
  if (bytes < 0) {
    FX_LOGS(FATAL) << "send failed: " << errno << ": " << strerror(errno);
  }
}
}  // namespace

TEST(Socket, PingPong) {
  libsync::Completion completion;
  std::thread server_thread(RunServer, &completion);
  completion.Wait();

  fbl::unique_fd client_fd;
  zx_status_t status = create_virtio_stream_socket(client_fd.reset_and_get_address());
  ASSERT_OK(status);

  int rv =
      connect(client_fd.get(), reinterpret_cast<const sockaddr*>(&kVsockAddr), sizeof(kVsockAddr));
  ASSERT_GE(rv, 0) << "connect failed: " << errno << ": " << strerror(errno);

  ssize_t bytes = send(client_fd.get(), kClientMessage, sizeof(kClientMessage), 0);
  ASSERT_GT(bytes, 0) << "send failed: " << errno << ": " << strerror(errno);

  char buffer[1024] = {};
  bytes = recv(client_fd.get(), buffer, sizeof(buffer), 0);
  ASSERT_GT(bytes, 0) << "recv failed: " << errno << ": " << strerror(errno);

  if (strncmp(buffer, kServerMessage, std::min(static_cast<size_t>(rv), sizeof(kClientMessage))) !=
      0) {
    FX_LOGS(FATAL) << "Received from server: " << buffer << " expected: " << kServerMessage;
  }
  server_thread.join();
}
