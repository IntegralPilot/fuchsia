// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <IBinderEcho.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/fdio.h>
#include <lib/syslog/cpp/macros.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <sys/socket.h>

#include <utility>

#include <binder/Binder.h>
#include <binder/RpcServer.h>
#include <binder/RpcSession.h>
#include <binder/unique_fd.h>
#include <gtest/gtest.h>

TEST(Client, InetLoopback) {
  constexpr int kLoopbackPort = 62000;
  auto session = android::RpcSession::make();
  session->setMaxIncomingThreads(1);

  auto status = session->setupPreconnectedClient({}, [] {
    FX_LOGS(TRACE) << "preconnected client callback invoked";
    auto socket_fd = android::binder::unique_fd(socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, 0));
    if (!socket_fd.ok()) {
      FX_LOGS(ERROR) << "socket failed: " << errno << ": " << strerror(errno);
      return android::binder::unique_fd();
    }

    int no_delay = 1;
    int rv = setsockopt(socket_fd.get(), IPPROTO_TCP, TCP_NODELAY, &no_delay, sizeof(no_delay));
    if (rv < 0) {
      FX_LOGS(ERROR) << "setsockopt failed: " << errno << ": " << strerror(errno);
      return android::binder::unique_fd();
    }

    sockaddr_in loopback_addr{
        .sin_family = AF_INET,
        .sin_port = htons(kLoopbackPort),
        .sin_addr =
            {
                .s_addr = htonl(INADDR_LOOPBACK),
            },
    };

    for (int tries = 0; tries < 5; ++tries) {
      sleep(2);
      FX_LOGS(TRACE) << "connecting";
      rv = connect(socket_fd.get(), reinterpret_cast<struct sockaddr*>(&loopback_addr),
                   sizeof(loopback_addr));
      if (rv == 0) {
        break;
      }
    }
    if (rv != 0) {
      FX_LOGS(ERROR) << "Could not connect: " << errno << ": " << strerror(errno);
      return android::binder::unique_fd();
    }

    return socket_fd;
  });
  EXPECT_EQ(status, android::OK) << "setupPreconnectedClient failed: "
                                 << android::statusToString(status);

  FX_LOGS(TRACE) << "getting root object";
  auto root_object = session->getRootObject();

  ASSERT_NE(root_object.get(), nullptr);

  FX_LOGS(TRACE) << "pinging";

  status = root_object->pingBinder();

  ASSERT_EQ(status, android::OK) << "pingBinder failed: " << android::statusToString(status);

  FX_LOGS(TRACE) << "Received ping";

  auto echo_binder = android::checked_interface_cast<IBinderEcho>(root_object);

  std::string result;

  auto binder_status = echo_binder->echo("hello binder", &result);

  ASSERT_TRUE(binder_status.isOk())
      << "echo transaction failed: " << android::statusToString(status);

  FX_LOGS(TRACE) << "Echo response: " << result;

  EXPECT_EQ(result, "hello binder");
}
