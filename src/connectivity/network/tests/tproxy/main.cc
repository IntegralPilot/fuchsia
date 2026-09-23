// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <arpa/inet.h>
#include <fidl/fuchsia.netemul.sync/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <netinet/in.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/connectivity/network/tests/socket/util.h"

namespace {

constexpr char kBusName[] = "test-bus";
constexpr char kTestName[] = "waiting...";
constexpr char kFilterSetupName[] = "setup-complete";
constexpr std::array<uint8_t, 4> kSendBuf = {1, 2, 3, 4};

TEST(TransparentProxyTest, NonTransparentSocketV4) {
  fbl::unique_fd listener;
  ASSERT_TRUE(listener = fbl::unique_fd(socket(AF_INET, SOCK_DGRAM, 0))) << strerror(errno);
  const sockaddr_in bind_addr = LoopbackSockaddrV4(8001);
  ASSERT_EQ(bind(listener.get(), reinterpret_cast<const sockaddr*>(&bind_addr), sizeof(bind_addr)),
            0)
      << strerror(errno);

  fbl::unique_fd sender;
  ASSERT_TRUE(sender = fbl::unique_fd(socket(AF_INET, SOCK_DGRAM, 0))) << strerror(errno);
  const sockaddr_in sender_addr = LoopbackSockaddrV4(0);
  ASSERT_EQ(
      bind(sender.get(), reinterpret_cast<const sockaddr*>(&sender_addr), sizeof(sender_addr)), 0)
      << strerror(errno);

  // Send to a *different* port than the listener socket is bound to.
  const sockaddr_in dst_addr = LoopbackSockaddrV4(11111);
  ASSERT_EQ(sendto(sender.get(), kSendBuf.data(), sizeof(kSendBuf), 0,
                   reinterpret_cast<const sockaddr*>(&dst_addr), sizeof(dst_addr)),
            static_cast<ssize_t>(sizeof(kSendBuf)))
      << strerror(errno);

  // Because the IP_TRANSPARENT option is not enabled on the listener socket, the
  // packet will not be transparently proxied.
  timeval tv = {
      .tv_sec = std::chrono::seconds(kNegativeCheckTimeout).count(),
  };
  EXPECT_EQ(setsockopt(listener.get(), SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv)), 0)
      << strerror(errno);
  std::array<uint8_t, sizeof(kSendBuf)> recv_buf;
  EXPECT_EQ(recv(listener.get(), recv_buf.data(), sizeof(recv_buf), 0), -1) << strerror(errno);
  EXPECT_EQ(errno, EAGAIN);
}

TEST(TransparentProxyTest, NonTransparentSocketV6) {
  fbl::unique_fd listener;
  ASSERT_TRUE(listener = fbl::unique_fd(socket(AF_INET6, SOCK_DGRAM, 0))) << strerror(errno);
  const sockaddr_in6 bind_addr = LoopbackSockaddrV6(8001);
  ASSERT_EQ(bind(listener.get(), reinterpret_cast<const sockaddr*>(&bind_addr), sizeof(bind_addr)),
            0)
      << strerror(errno);

  fbl::unique_fd sender;
  ASSERT_TRUE(sender = fbl::unique_fd(socket(AF_INET6, SOCK_DGRAM, 0))) << strerror(errno);
  const sockaddr_in6 sender_addr = LoopbackSockaddrV6(0);
  ASSERT_EQ(
      bind(sender.get(), reinterpret_cast<const sockaddr*>(&sender_addr), sizeof(sender_addr)), 0)
      << strerror(errno);

  // Send to a *different* port than the listener socket is bound to.
  const sockaddr_in6 dst_addr = LoopbackSockaddrV6(11111);
  ASSERT_EQ(sendto(sender.get(), kSendBuf.data(), sizeof(kSendBuf), 0,
                   reinterpret_cast<const sockaddr*>(&dst_addr), sizeof(dst_addr)),
            static_cast<ssize_t>(sizeof(kSendBuf)))
      << strerror(errno);

  // Because the IP_TRANSPARENT option is not enabled on the listener socket, the
  // packet will not be transparently proxied.
  timeval tv = {
      .tv_sec = std::chrono::seconds(kNegativeCheckTimeout).count(),
  };
  EXPECT_EQ(setsockopt(listener.get(), SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv)), 0)
      << strerror(errno);
  std::array<uint8_t, sizeof(kSendBuf)> recv_buf;
  EXPECT_EQ(recv(listener.get(), recv_buf.data(), sizeof(recv_buf), 0), -1) << strerror(errno);
  EXPECT_EQ(errno, EAGAIN);
}

struct TestCase {
  std::string test_name;
  in_port_t listen_port;
  std::string dst_addr_str;
  in_port_t dst_port;
};

std::array<TestCase, 3> Ipv4TestCases() {
  return {// This packet will be proxied to socket bound to 8001, but its
          // destination address will remain the same (localhost).
          TestCase{.test_name = "Proxy to port 8001",
                   .listen_port = 8001,
                   .dst_addr_str = "127.0.0.1",
                   .dst_port = 11111},
          // This packet will be proxied to socket bound on localhost, but its
          // destination port will remain the same (22222).
          TestCase{.test_name = "Proxy to localhost",
                   .listen_port = 22222,
                   .dst_addr_str = "192.0.2.1",
                   .dst_port = 22222},
          // This packet will be proxied to socket bound to 8002; both its
          // destination address and port have been overridden.
          TestCase{.test_name = "Proxy to localhost port 8002",
                   .listen_port = 8002,
                   .dst_addr_str = "192.0.2.1",
                   .dst_port = 33333}};
}

class TransparentProxyV4Test : public testing::TestWithParam<TestCase> {};

TEST_P(TransparentProxyV4Test, TransparentSocket) {
  const auto [test_name, listen_port, dst_addr_str, dst_port] = GetParam();

  fbl::unique_fd listener;
  ASSERT_TRUE(listener = fbl::unique_fd(socket(AF_INET, SOCK_DGRAM, 0))) << strerror(errno);

  // Bind a transparent listener socket so that it can receive transparently
  // proxied packets; also enable the `IP_RECVORIGDSTADDR` socket option so we can
  // observe the original destination of such packets.
  constexpr int kEnable = 1;
  ASSERT_EQ(setsockopt(listener.get(), SOL_IP, IP_TRANSPARENT, &kEnable, sizeof(kEnable)), 0)
      << strerror(errno);
  ASSERT_EQ(setsockopt(listener.get(), SOL_IP, IP_RECVORIGDSTADDR, &kEnable, sizeof(kEnable)), 0)
      << strerror(errno);

  const sockaddr_in bind_addr = LoopbackSockaddrV4(listen_port);
  ASSERT_EQ(bind(listener.get(), reinterpret_cast<const sockaddr*>(&bind_addr), sizeof(bind_addr)),
            0)
      << strerror(errno);

  fbl::unique_fd sender;
  ASSERT_TRUE(sender = fbl::unique_fd(socket(AF_INET, SOCK_DGRAM, 0))) << strerror(errno);
  const sockaddr_in sender_addr = {
      .sin_family = AF_INET,
      .sin_port = 0,
      .sin_addr = {INADDR_ANY},
  };
  ASSERT_EQ(
      bind(sender.get(), reinterpret_cast<const sockaddr*>(&sender_addr), sizeof(sender_addr)), 0)
      << strerror(errno);

  // Send to a *different* address and/or port than the listener socket is bound to.
  sockaddr_in dst_addr = {
      .sin_family = AF_INET,
      .sin_port = htons(dst_port),
  };
  ASSERT_EQ(inet_pton(AF_INET, dst_addr_str.c_str(), &dst_addr.sin_addr.s_addr), 1)
      << strerror(errno);
  ASSERT_EQ(sendto(sender.get(), kSendBuf.data(), sizeof(kSendBuf), 0,
                   reinterpret_cast<const sockaddr*>(&dst_addr), sizeof(dst_addr)),
            static_cast<ssize_t>(sizeof(kSendBuf)))
      << strerror(errno);

  std::array<uint8_t, sizeof(kSendBuf)> recv_buf;
  iovec recv_iovec = {
      .iov_base = recv_buf.data(),
      .iov_len = sizeof(recv_buf),
  };
  sockaddr_in original_dst_addr;
  std::array<uint8_t, CMSG_SPACE(sizeof(original_dst_addr))> recv_control;
  msghdr recv_msghdr = {
      .msg_iov = &recv_iovec,
      .msg_iovlen = 1,
      .msg_control = recv_control.data(),
      .msg_controllen = sizeof(recv_control),
  };

  // The message should be received by the listener socket, along with the
  // original destination address as a control message.
  ASSERT_EQ(recvmsg(listener.get(), &recv_msghdr, 0), static_cast<ssize_t>(sizeof(kSendBuf)))
      << strerror(errno);
  EXPECT_EQ(kSendBuf, recv_buf);
  cmsghdr* cmsg = CMSG_FIRSTHDR(&recv_msghdr);
  ASSERT_NE(cmsg, nullptr);
  EXPECT_EQ(cmsg->cmsg_len, CMSG_LEN(sizeof(original_dst_addr)));
  EXPECT_EQ(cmsg->cmsg_level, SOL_IP);
  EXPECT_EQ(cmsg->cmsg_type, IP_RECVORIGDSTADDR);
  memcpy(&original_dst_addr, CMSG_DATA(cmsg), sizeof(original_dst_addr));
  EXPECT_EQ(memcmp(&original_dst_addr, &dst_addr, sizeof(dst_addr)), 0);
}

INSTANTIATE_TEST_SUITE_P(TransparentProxyTests, TransparentProxyV4Test,
                         testing::ValuesIn(Ipv4TestCases()),
                         [](const testing::TestParamInfo<TestCase>& info) {
                           std::string test_name(info.param.test_name);
                           std::replace(test_name.begin(), test_name.end(), ' ', '_');
                           return test_name;
                         });

std::array<TestCase, 3> Ipv6TestCases() {
  return {// This packet will be proxied to socket bound to 8001, but its
          // destination address will remain the same (localhost).
          TestCase{.test_name = "Proxy to port 8001",
                   .listen_port = 8001,
                   .dst_addr_str = "::1",
                   .dst_port = 11111},
          // This packet will be proxied to socket bound on localhost, but its
          // destination port will remain the same (44444).
          TestCase{.test_name = "Proxy to localhost",
                   .listen_port = 44444,
                   .dst_addr_str = "2001:db8::1",
                   .dst_port = 44444},
          // This packet will be proxied to socket bound to 8002; both its
          // destination address and port have been overridden.
          TestCase{.test_name = "Proxy to localhost port 8002",
                   .listen_port = 8002,
                   .dst_addr_str = "2001:db8::1",
                   .dst_port = 55555}};
}

class TransparentProxyV6Test : public testing::TestWithParam<TestCase> {};

TEST_P(TransparentProxyV6Test, TransparentSocket) {
  const auto [test_name, listen_port, dst_addr_str, dst_port] = GetParam();

  fbl::unique_fd listener;
  ASSERT_TRUE(listener = fbl::unique_fd(socket(AF_INET6, SOCK_DGRAM, 0))) << strerror(errno);

  // Bind a transparent listener socket so that it can receive transparently
  // proxied packets.
  //
  // TODO(https://fxbug.dev/337872703): also enable the `IPV6_RECVORIGDSTADDR`
  // socket option once it is implemented so we can observe the original
  // destination of proxied IPv6 packets.
  constexpr int kEnable = 1;
  ASSERT_EQ(setsockopt(listener.get(), SOL_IP, IP_TRANSPARENT, &kEnable, sizeof(kEnable)), 0)
      << strerror(errno);

  const sockaddr_in6 bind_addr = LoopbackSockaddrV6(listen_port);
  ASSERT_EQ(bind(listener.get(), reinterpret_cast<const sockaddr*>(&bind_addr), sizeof(bind_addr)),
            0)
      << strerror(errno);

  fbl::unique_fd sender;
  ASSERT_TRUE(sender = fbl::unique_fd(socket(AF_INET6, SOCK_DGRAM, 0))) << strerror(errno);
  const sockaddr_in6 sender_addr = {
      .sin6_family = AF_INET6,
      .sin6_port = 0,
      .sin6_addr = IN6ADDR_ANY_INIT,
  };
  ASSERT_EQ(
      bind(sender.get(), reinterpret_cast<const sockaddr*>(&sender_addr), sizeof(sender_addr)), 0)
      << strerror(errno);

  // Send to a *different* address and/or port than the listener socket is bound to.
  sockaddr_in6 dst_addr = {
      .sin6_family = AF_INET6,
      .sin6_port = htons(dst_port),
  };
  ASSERT_EQ(inet_pton(AF_INET6, dst_addr_str.c_str(), &dst_addr.sin6_addr), 1) << strerror(errno);
  ASSERT_EQ(sendto(sender.get(), kSendBuf.data(), sizeof(kSendBuf), 0,
                   reinterpret_cast<const sockaddr*>(&dst_addr), sizeof(dst_addr)),
            static_cast<ssize_t>(sizeof(kSendBuf)))
      << strerror(errno);

  std::array<uint8_t, sizeof(kSendBuf)> recv_buf;
  iovec recv_iovec = {
      .iov_base = recv_buf.data(),
      .iov_len = sizeof(recv_buf),
  };
  msghdr recv_msghdr = {
      .msg_iov = &recv_iovec,
      .msg_iovlen = 1,
      .msg_control = nullptr,
      .msg_controllen = 0,
  };

  // The message should be received by the listener socket.
  //
  // TODO(https://fxbug.dev/337872703): verify that the original destination
  // address is received as a control message.
  ASSERT_EQ(recvmsg(listener.get(), &recv_msghdr, 0), static_cast<ssize_t>(sizeof(kSendBuf)))
      << strerror(errno);
  EXPECT_EQ(kSendBuf, recv_buf);
}

INSTANTIATE_TEST_SUITE_P(TransparentProxyTests, TransparentProxyV6Test,
                         testing::ValuesIn(Ipv6TestCases()),
                         [](const testing::TestParamInfo<TestCase>& info) {
                           std::string test_name(info.param.test_name);
                           std::replace(test_name.begin(), test_name.end(), ' ', '_');
                           return test_name;
                         });

class TransparentProxyDualStackTest : public testing::TestWithParam<TestCase> {};

TEST_P(TransparentProxyDualStackTest, TransparentSocket) {
  const auto [test_name, listen_port, dst_addr_str, dst_port] = GetParam();

  fbl::unique_fd listener;
  ASSERT_TRUE(listener = fbl::unique_fd(socket(AF_INET6, SOCK_DGRAM, 0))) << strerror(errno);

  // Bind a transparent listener socket so that it can receive transparently
  // proxied packets.
  //
  // TODO(https://fxbug.dev/337872703): also enable the `IPV6_RECVORIGDSTADDR`
  // socket option once it is implemented so we can observe the original
  // destination of proxied IPv6 packets.
  constexpr int kEnable = 1;
  ASSERT_EQ(setsockopt(listener.get(), SOL_IP, IP_TRANSPARENT, &kEnable, sizeof(kEnable)), 0)
      << strerror(errno);

  int v6_only;
  socklen_t optlen = sizeof(v6_only);
  ASSERT_EQ(getsockopt(listener.get(), SOL_IPV6, IPV6_V6ONLY, &v6_only, &optlen), 0)
      << strerror(errno);
  EXPECT_EQ(v6_only, false);

  // Bind to the IPv6 "any" address (::) so that we can receive both IPv4 and IPv6
  // packets.
  const sockaddr_in6 bind_addr = {
      .sin6_family = AF_INET6,
      .sin6_port = htons(listen_port),
      .sin6_addr = IN6ADDR_ANY_INIT,
  };
  ASSERT_EQ(bind(listener.get(), reinterpret_cast<const sockaddr*>(&bind_addr), sizeof(bind_addr)),
            0)
      << strerror(errno);

  fbl::unique_fd sender;
  ASSERT_TRUE(sender = fbl::unique_fd(socket(AF_INET, SOCK_DGRAM, 0))) << strerror(errno);
  const sockaddr_in sender_addr = {
      .sin_family = AF_INET,
      .sin_port = 0,
      .sin_addr = {INADDR_ANY},
  };
  ASSERT_EQ(
      bind(sender.get(), reinterpret_cast<const sockaddr*>(&sender_addr), sizeof(sender_addr)), 0)
      << strerror(errno);

  // Send to a *different* address and/or port than the listener socket is bound to.
  sockaddr_in dst_addr = {
      .sin_family = AF_INET,
      .sin_port = htons(dst_port),
  };
  ASSERT_EQ(inet_pton(AF_INET, dst_addr_str.c_str(), &dst_addr.sin_addr.s_addr), 1)
      << strerror(errno);
  ASSERT_EQ(sendto(sender.get(), kSendBuf.data(), sizeof(kSendBuf), 0,
                   reinterpret_cast<const sockaddr*>(&dst_addr), sizeof(dst_addr)),
            static_cast<ssize_t>(sizeof(kSendBuf)))
      << strerror(errno);

  std::array<uint8_t, sizeof(kSendBuf)> recv_buf;
  iovec recv_iovec = {
      .iov_base = recv_buf.data(),
      .iov_len = sizeof(recv_buf),
  };
  msghdr recv_msghdr = {
      .msg_iov = &recv_iovec,
      .msg_iovlen = 1,
      .msg_control = nullptr,
      .msg_controllen = 0,
  };

  // The message should be received by the listener socket.
  //
  // TODO(https://fxbug.dev/337872703): verify that the original destination
  // address is received as an IPv4-mapped IPv6 address in a control message.
  ASSERT_EQ(recvmsg(listener.get(), &recv_msghdr, 0), static_cast<ssize_t>(sizeof(kSendBuf)))
      << strerror(errno);
  EXPECT_EQ(kSendBuf, recv_buf);
}

INSTANTIATE_TEST_SUITE_P(TransparentProxyTests, TransparentProxyDualStackTest,
                         testing::ValuesIn(Ipv4TestCases()),
                         [](const testing::TestParamInfo<TestCase>& info) {
                           std::string test_name(info.param.test_name);
                           std::replace(test_name.begin(), test_name.end(), ' ', '_');
                           return test_name;
                         });

}  // namespace

int main(int argc, char** argv) {
  // Make sure the transparent proxy has been configured before running any tests
  // by waiting on the synchronization bus for the message that setup is complete.
  zx::result client_end = component::Connect<fuchsia_netemul_sync::SyncManager>();
  if (!client_end.is_ok()) {
    ZX_PANIC("failed to connect to sync manager: %s", client_end.status_string());
  }
  fidl::WireSyncClient sync_manager{std::move(*client_end)};

  auto bus_endpoints = fidl::CreateEndpoints<fuchsia_netemul_sync::Bus>();
  if (bus_endpoints.is_error()) {
    ZX_PANIC("error creating bus endpoints: %s", zx_status_get_string(bus_endpoints.error_value()));
  }
  {
    fidl::Status result =
        sync_manager->BusSubscribe(kBusName, kTestName, std::move(bus_endpoints->server));
    ZX_ASSERT_MSG(result.status() == ZX_OK, "error getting bus: %s", result.status_string());
  }

  fidl::WireSyncClient bus{std::move(bus_endpoints->client)};
  {
    std::array<fidl::StringView, 1> clients = {fidl::StringView(kFilterSetupName)};
    fidl::Status result = bus->WaitForClients(
        fidl::VectorView<fidl::StringView>::FromExternal(clients), /* no timeout */ 0);
    ZX_ASSERT_MSG(result.status() == ZX_OK, "error waiting for filter setup to complete: %s",
                  result.status_string());
  }

  testing::InitGoogleTest(&argc, argv);
  return RUN_ALL_TESTS();
}
