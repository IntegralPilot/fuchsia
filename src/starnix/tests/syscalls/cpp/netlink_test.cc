// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <arpa/inet.h>
#include <endian.h>
#include <errno.h>
#include <fcntl.h>
#include <net/ethernet.h>
#include <net/if.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <unistd.h>

#include <format>
#include <thread>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/capability.h>
#include <linux/fib_rules.h>
#include <linux/genetlink.h>
#include <linux/if_link.h>
#include <linux/if_tun.h>
#include <linux/neighbour.h>
#include <linux/netlink.h>
#include <linux/rtnetlink.h>
#include <linux/sockios.h>

#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

int GetNlaMessageParseErrorCode() {
  // TODO(https://fxbug.dev/450959280): NLA parsing errors produce different
  // error codes in Starnix, causing divergence from Linux behavior.
  if (test_helper::IsStarnix()) {
    return -EINVAL;
  }

  return -ERANGE;
}

template <int PROTOCOL>
class NetlinkTest : public ::testing::Test {
 public:
  void SetUp() override {
    nl_sock_.reset(socket(AF_NETLINK, SOCK_RAW, PROTOCOL));
    ASSERT_TRUE(nl_sock_.is_valid()) << strerror(errno);
  }

  ssize_t SendMsg(test_helper::NetlinkEncoder& encoder) const {
    iovec iov = {};
    encoder.Finalize(iov);
    struct msghdr header = {};
    header.msg_iov = &iov;
    header.msg_iovlen = 1;
    return sendmsg(nl_sock_.get(), &header, 0);
  }

  void SetStrictCheck() {
    int val = 1;
    EXPECT_THAT(setsockopt(nl_sock_.get(), SOL_NETLINK, NETLINK_GET_STRICT_CHK, &val, sizeof(val)),
                SyscallSucceeds());
  }

  fbl::unique_fd nl_sock_;
};

class NetlinkRouteTest : public NetlinkTest<NETLINK_ROUTE> {
 public:
  void CheckNetlinkAlive() {
    // Check netlink is still there and responding to observe any kernel panics
    // happening on the netlink thread.
    test_helper::NetlinkEncoder encoder(RTM_GETLINK, NLM_F_REQUEST | NLM_F_DUMP);
    struct ifinfomsg ifi = {
        .ifi_family = AF_UNSPEC,
        .ifi_type = 0,
        .ifi_index = 0,
        .ifi_flags = 0,
        .ifi_change = 0,
    };
    encoder.Write(ifi);
    ASSERT_THAT(SendMsg(encoder), SyscallSucceeds());

    char msg_buf_[4096];
    ssize_t len = recv(nl_sock_.get(), msg_buf_, sizeof(msg_buf_), 0);
    ASSERT_GT(len, 0) << strerror(errno);
    const nlmsghdr* nlmsg = reinterpret_cast<const nlmsghdr*>(msg_buf_);
    ASSERT_TRUE(MY_NLMSG_OK(nlmsg, len));
    // Accept either NEWLINK or DONE from the system. If there are no links
    // installed, then DONE is seen instead of NEWLINK.
    ASSERT_THAT(nlmsg->nlmsg_type,
                testing::AnyOf(testing::Eq(RTM_NEWLINK), testing::Eq(NLMSG_DONE)));
  }
};

class NetlinkGenericTest : public NetlinkTest<NETLINK_GENERIC> {
 public:
  std::optional<uint16_t> GetFamily(test_helper::NetlinkEncoder& encoder,
                                    const std::string_view& family_name) {
    encoder.StartMessage(GENL_ID_CTRL, NLM_F_REQUEST);
    const uint32_t sequence = encoder.sequence();
    encoder.BeginGenetlinkHeader(CTRL_CMD_GETFAMILY);
    encoder.BeginNla(CTRL_ATTR_FAMILY_NAME);
    encoder.WriteString(family_name);
    encoder.EndNla();
    iovec iov = {};
    encoder.Finalize(iov);
    struct msghdr header = {};
    header.msg_iov = &iov;
    header.msg_iovlen = 1;

    if (static_cast<size_t>(sendmsg(nl_sock_.get(), &header, 0)) != iov.iov_len) {
      ADD_FAILURE() << "sendmsg failed" << strerror(errno);
      return std::nullopt;
    };

    struct {
      nlmsghdr hdr;
      genlmsghdr genl;
      // Family ID
      nlattr id_attr;
      uint16_t id;
    } input;
    iov.iov_len = sizeof(input);
    iov.iov_base = &input;

    ssize_t received = recvmsg(nl_sock_.get(), &header, MSG_TRUNC);
    if (received < 0) {
      ADD_FAILURE() << "recvmsg failed " << strerror(errno);
      return std::nullopt;
    }
    if (static_cast<size_t>(received) < sizeof(input)) {
      ADD_FAILURE() << " short received message " << received;
      return std::nullopt;
    }
    if (input.hdr.nlmsg_seq != sequence) {
      ADD_FAILURE() << " unexpected message sequence, " << input.hdr.nlmsg_seq << " want "
                    << sequence;
      return std::nullopt;
    }
    return input.id;
  }
};

// Regression test for Syzkaller netlink crash in https://fxbug.dev/390646804.
//
// Crash was caused by the RTM_NEWPREFIX message being unimplemented and falling
// through a panic.
TEST_F(NetlinkRouteTest, RtmNewPrefixDoesntCrash) {
  test_helper::NetlinkEncoder encoder(RTM_NEWPREFIX, 0);
  struct prefixmsg pfx = {};
  encoder.Write(pfx);
  ASSERT_THAT(SendMsg(encoder), SyscallSucceeds());
  CheckNetlinkAlive();
}

TEST_F(NetlinkRouteTest, NoCapabilities) {
  if (!test_helper::HasCapability(CAP_NET_ADMIN)) {
    GTEST_SKIP() << "Needs CAP_NET_ADMIN";
  }

  test_helper::UnsetCapabilityEffective(CAP_NET_ADMIN);

  test_helper::NetlinkEncoder encoder(RTM_NEWRULE,
                                      NLM_F_REQUEST | NLM_F_CREATE | NLM_F_EXCL | NLM_F_ACK);

  // FIB Rule Header.
  fib_rule_hdr frh = {
      .family = AF_INET,         // IPv4 rule
      .dst_len = 0,              // Match any destination
      .src_len = 32,             // Match a /32 source address
      .table = RT_TABLE_UNSPEC,  // We will specify the table in an attribute
      .action = FR_ACT_TO_TBL,   // Action is to jump to a new table
  };
  encoder.Write(frh);

  // Add Attribute: Source Address (FRA_SRC)
  in_addr src_addr;
  inet_pton(AF_INET, "192.0.2.1", &src_addr);
  encoder.AddRtAttr(FRA_SRC, src_addr);

  // Add Attribute: Table ID (FRA_TABLE)
  uint32_t table_id = 100;
  encoder.AddRtAttr(FRA_TABLE, table_id);

  // Send the message.
  ASSERT_THAT(SendMsg(encoder), SyscallSucceeds());

  // Receive and verify the reply.
  char recv_buf[4096];
  ssize_t result;
  ASSERT_GT(result = recv(nl_sock_.get(), &recv_buf, sizeof(recv_buf), 0), 0) << strerror(errno);

  // Verify that we've received exactly one message that contains the `EPERM`
  // error.
  nlmsghdr* nlmsg = reinterpret_cast<nlmsghdr*>(recv_buf);
  uint32_t len = static_cast<uint32_t>(result);
  ASSERT_TRUE(NLMSG_OK(nlmsg, len));
  EXPECT_EQ(nlmsg->nlmsg_type, NLMSG_ERROR);
  nlmsgerr* err_data = reinterpret_cast<nlmsgerr*>(NLMSG_DATA(nlmsg));
  EXPECT_EQ(err_data->error, -EPERM);

  nlmsg = NLMSG_NEXT(nlmsg, len);
  EXPECT_FALSE(NLMSG_OK(nlmsg, len));

  // Restore the capability.
  test_helper::SetCapabilityEffective(CAP_NET_ADMIN);
}

void SetLoopbackIfAddr(in_addr_t addr) {
  constexpr char kLoopbackIfName[] = "lo";

  fbl::unique_fd fd;
  ASSERT_TRUE(fd = fbl::unique_fd(socket(AF_INET, SOCK_DGRAM, 0))) << strerror(errno);
  ifreq ifr;
  *(reinterpret_cast<sockaddr_in*>(&ifr.ifr_addr)) = sockaddr_in{
      .sin_family = AF_INET,
      .sin_addr = {.s_addr = addr},
  };
  strncpy(ifr.ifr_name, kLoopbackIfName, IFNAMSIZ);
  ASSERT_EQ(ioctl(fd.get(), SIOCSIFADDR, &ifr), 0) << strerror(errno);
}

TEST(RouteNetlinkSocket, AddDropMulticastGroup) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping suite.";
  }

  fbl::unique_fd nlsock(socket(AF_NETLINK, SOCK_RAW, NETLINK_ROUTE));
  ASSERT_TRUE(nlsock) << strerror(errno);

  struct sockaddr_nl addr = {};
  addr.nl_family = AF_NETLINK;
  struct sockaddr* sa = reinterpret_cast<struct sockaddr*>(&addr);
  ASSERT_EQ(bind(nlsock.get(), sa, sizeof(addr)), 0) << strerror(errno);

  int group = RTNLGRP_IPV4_IFADDR;
  ASSERT_EQ(setsockopt(nlsock.get(), SOL_NETLINK, NETLINK_ADD_MEMBERSHIP, &group, sizeof(group)), 0)
      << strerror(errno);

  ASSERT_NO_FATAL_FAILURE(SetLoopbackIfAddr(inet_addr("127.0.0.2")));

  sleep(1);

  char buf[4096] = {};

  // Should observe 2 messages (removing old address, adding new address)
  // because we're in the corresponding multicast group.
  ssize_t len = recv(nlsock.get(), buf, sizeof(buf), 0);
  ASSERT_GT(len, 0) << strerror(errno);

  nlmsghdr* nlmsg = reinterpret_cast<nlmsghdr*>(buf);

  ASSERT_TRUE(MY_NLMSG_OK(nlmsg, len));
  ASSERT_EQ(nlmsg->nlmsg_type, RTM_DELADDR);
  rtmsg* rtm = reinterpret_cast<rtmsg*>(NLMSG_DATA(nlmsg));
  ASSERT_EQ(rtm->rtm_family, AF_INET);

  nlmsg = NLMSG_NEXT(nlmsg, len);

  if (MY_NLMSG_OK(nlmsg, len)) {
    // The next message is already in the buffer, so we don't need to do
    // anything here.
  } else {
    // Need to receive again.
    len = recv(nlsock.get(), buf, sizeof(buf), 0);
    ASSERT_GT(len, 0) << strerror(errno);
    nlmsg = reinterpret_cast<nlmsghdr*>(buf);
    ASSERT_TRUE(MY_NLMSG_OK(nlmsg, len));
  }

  // Assert that the content of the second message indicates the new loopback
  // address being added.
  ASSERT_EQ(nlmsg->nlmsg_type, RTM_NEWADDR);
  rtm = reinterpret_cast<rtmsg*>(NLMSG_DATA(nlmsg));
  ASSERT_EQ(rtm->rtm_family, AF_INET);

  // Now we should have run out of messages.
  nlmsg = NLMSG_NEXT(nlmsg, len);
  ASSERT_FALSE(MY_NLMSG_OK(nlmsg, len));

  // Drop the multicast group membership so that we won't get notified about
  // further address changes.
  ASSERT_EQ(setsockopt(nlsock.get(), SOL_NETLINK, NETLINK_DROP_MEMBERSHIP, &group, sizeof(group)),
            0)
      << strerror(errno);

  // Restore the usual loopback address.
  ASSERT_NO_FATAL_FAILURE(SetLoopbackIfAddr(inet_addr("127.0.0.1")));

  // Should not observe a message because we're not in any multicast group.
  ASSERT_EQ(recv(nlsock.get(), buf, sizeof(buf), MSG_DONTWAIT), -1);
  ASSERT_EQ(errno, EAGAIN);
}

class RouteNetlinkSocketTestWithInterface : public NetlinkRouteTest {
 public:
  void SetUp() override {
    NetlinkRouteTest::SetUp();

    // TODO(https://fxbug.dev/317285180) don't skip on baseline
    if (!test_helper::HasSysAdmin()) {
      GTEST_SKIP() << "Not running with sysadmin capabilities, skipping suite.";
    }
    const char* dev_tun;
    if (test_helper::IsStarnix()) {
      dev_tun = "/dev/tun";
    } else {
      dev_tun = "/dev/net/tun";
    }
    tun_ = fbl::unique_fd(open(dev_tun, O_RDWR));
    ifreq ifr{};
    // Use IFF_TAP because these tests exercise features that are only available
    // on Ethernet-like interfaces.
    ifr.ifr_flags = IFF_NO_PI | IFF_TAP;

    strncpy(ifr.ifr_name, kTestIfName, IFNAMSIZ);

    auto result = ioctl(tun_.get(), TUNSETIFF, &ifr);
    ASSERT_EQ(result, 0) << strerror(errno);
    auto s = fbl::unique_fd(socket(AF_INET, SOCK_DGRAM, 0));
    // Wait for the tun device to show up.
    ifr.ifr_flags = 0;
    int remaining_retries = 10;
    while (ioctl(s.get(), SIOCGIFFLAGS, &ifr) == -1) {
      ASSERT_EQ(errno, ENODEV);
      ASSERT_GT(remaining_retries--, 0);
      std::this_thread::sleep_for(std::chrono::milliseconds(100));
    }
    ifr.ifr_flags |= IFF_UP;
    ASSERT_EQ(ioctl(s.get(), SIOCSIFFLAGS, &ifr), 0) << strerror(errno);
  }

  void TearDown() override {
    tun_.reset();
    // Wait for the device to go away so that it does not linger into the next test.
    while (if_nametoindex(kTestIfName) > 0) {
      std::this_thread::sleep_for(std::chrono::milliseconds(100));
    }
  }
  static constexpr char kTestIfName[] = "netlink_test";

 private:
  fbl::unique_fd tun_;
};

struct RouteNetlinkSocketNewAddrParam {
  uint8_t family;
  const char* addr;
  uint8_t prefix;
  bool expect_subnet_route;
};

class RouteNetlinkSocketNewAddr
    : public RouteNetlinkSocketTestWithInterface,
      public testing::WithParamInterface<RouteNetlinkSocketNewAddrParam> {};

TEST_P(RouteNetlinkSocketNewAddr, AddSubnetRoute) {
  const auto [family, addr, prefix, expect_subnet_route] = GetParam();

  int group = family == AF_INET ? RTNLGRP_IPV4_ROUTE : RTNLGRP_IPV6_ROUTE;
  ASSERT_EQ(setsockopt(nl_sock_.get(), SOL_NETLINK, NETLINK_ADD_MEMBERSHIP, &group, sizeof(group)),
            0)
      << strerror(errno);

  {
    test_helper::NetlinkEncoder encoder(RTM_NEWADDR, NLM_F_REQUEST | NLM_F_ACK);
    struct ifaddrmsg ifa = {
        .ifa_family = family,
        .ifa_prefixlen = prefix,
        .ifa_flags = IFA_F_PERMANENT,
        .ifa_scope = RT_SCOPE_UNIVERSE,
        .ifa_index = if_nametoindex(kTestIfName),
    };
    uint8_t addrbuf[sizeof(in6_addr)];
    ASSERT_EQ(inet_pton(family, addr, &addrbuf), 1);
    auto addr = std::span(addrbuf, family == AF_INET ? sizeof(in_addr) : sizeof(in6_addr));
    encoder.Write(ifa);
    encoder.BeginNla(IFA_ADDRESS);
    encoder.WriteSpan(addr);
    encoder.EndNla();
    encoder.BeginNla(IFA_LOCAL);
    encoder.WriteSpan(addr);
    encoder.EndNla();
    ASSERT_THAT(SendMsg(encoder), SyscallSucceeds());
  }

  // Use a timeout instead of MSG_DONTWAIT to avoid timing issues with delays.
  struct timeval timeout = {.tv_sec = 0, .tv_usec = 100'000};
  ASSERT_EQ(setsockopt(nl_sock_.get(), SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)), 0);

  bool ack_seen = false;
  bool target_route_seen = false;
  char buf[4096] = {};

  while (true) {
    // If we don't expect a route, we'll continue waiting for EAGAIN to prove
    // it doesn't arrive. Otherwise, short circuit the loop.
    if (ack_seen && target_route_seen) {
      break;
    }
    ssize_t len = recv(nl_sock_.get(), buf, sizeof(buf), 0);
    if (len == 0) {
      break;
    }
    if (len == -1) {
      ASSERT_EQ(errno, EAGAIN);
      break;
    }
    rtmsg* rtm = nullptr;
    for (nlmsghdr* nlh = reinterpret_cast<nlmsghdr*>(buf); MY_NLMSG_OK(nlh, len);
         nlh = NLMSG_NEXT(nlh, len)) {
      switch (nlh->nlmsg_type) {
        case NLMSG_ERROR: {
          auto* errmsg = reinterpret_cast<nlmsgerr*>(NLMSG_DATA(nlh));
          ASSERT_EQ(errmsg->error, 0);
          ack_seen = true;
          break;
        }
        case RTM_NEWROUTE: {
          rtm = reinterpret_cast<rtmsg*>(NLMSG_DATA(nlh));
          // Linux and Fuchsia does local delivery differently, we ignore those by
          // filtering out routes that are not in the main table.
          if (rtm->rtm_table != RT_TABLE_MAIN) {
            continue;
          }
          if (rtm->rtm_dst_len == prefix) {
            target_route_seen = true;
          }
          break;
        }
        default:
          FAIL() << "unknown message type: " << nlh->nlmsg_type;
      }
    }
  }

  ASSERT_EQ(expect_subnet_route, target_route_seen);
}

INSTANTIATE_TEST_SUITE_P(
    RouteNetlinkSocket, RouteNetlinkSocketNewAddr,
    testing::Values(
        RouteNetlinkSocketNewAddrParam{
            .family = AF_INET, .addr = "192.0.2.1", .prefix = 0, .expect_subnet_route = false},
        RouteNetlinkSocketNewAddrParam{
            .family = AF_INET, .addr = "192.0.2.1", .prefix = 1, .expect_subnet_route = true},
        RouteNetlinkSocketNewAddrParam{
            .family = AF_INET, .addr = "192.0.2.1", .prefix = 32, .expect_subnet_route = false},
        RouteNetlinkSocketNewAddrParam{
            .family = AF_INET6, .addr = "2001:db8::1", .prefix = 0, .expect_subnet_route = true},
        RouteNetlinkSocketNewAddrParam{
            .family = AF_INET6, .addr = "2001:db8::1", .prefix = 1, .expect_subnet_route = true},
        RouteNetlinkSocketNewAddrParam{
            .family = AF_INET6, .addr = "2001:db8::1", .prefix = 128, .expect_subnet_route = true}),
    [](const testing::TestParamInfo<RouteNetlinkSocketNewAddr::ParamType>& info) {
      return std::format("{}_prefix_{}", info.param.family == AF_INET ? "v4" : "v6",
                         info.param.prefix);
    });

struct RouteNetlinkSocketGetNeighParam {
  std::string test_case_name;
  ndmsg header;
  std::optional<std::pair<std::string, uint8_t>> addr_and_family;
  int expected_err;
};

class RouteNetlinkSocketGetNeigh
    : public NetlinkRouteTest,
      public testing::WithParamInterface<RouteNetlinkSocketGetNeighParam> {};

TEST_P(RouteNetlinkSocketGetNeigh, GetNeighErrors) {
  test_helper::NetlinkEncoder encoder(RTM_GETNEIGH, NLM_F_REQUEST);

  const auto& [name, header, addr_and_family, expected_err] = GetParam();

  // Neighbor header.
  encoder.Write(header);

  // Add attribute: destination address (NDA_DST)
  if (addr_and_family) {
    const auto& [addr, addr_family] = *addr_and_family;
    switch (addr_family) {
      case AF_INET: {
        in_addr dst_addr;
        inet_pton(AF_INET, addr.c_str(), &dst_addr);
        encoder.AddRtAttr(NDA_DST, dst_addr);
        break;
      }
      case AF_INET6: {
        in6_addr dst_addr;
        inet_pton(AF_INET6, addr.c_str(), &dst_addr);
        encoder.AddRtAttr(NDA_DST, dst_addr);
        break;
      }
      default:
        FAIL() << "unknown address family: " << addr_family;
    }
  }

  // Send the message.
  ASSERT_THAT(SendMsg(encoder), SyscallSucceeds());

  // Receive and verify the reply.
  char recv_buf[4096];
  ssize_t result;
  ASSERT_GT(result = recv(nl_sock_.get(), &recv_buf, sizeof(recv_buf), 0), 0) << strerror(errno);

  // Verify that we've received exactly one message that contains the expected
  // error.
  nlmsghdr* nlmsg = reinterpret_cast<nlmsghdr*>(recv_buf);
  uint32_t len = static_cast<uint32_t>(result);
  ASSERT_TRUE(NLMSG_OK(nlmsg, len));
  EXPECT_EQ(nlmsg->nlmsg_type, NLMSG_ERROR);
  nlmsgerr* err_data = reinterpret_cast<nlmsgerr*>(NLMSG_DATA(nlmsg));
  EXPECT_EQ(err_data->error, expected_err);

  nlmsg = NLMSG_NEXT(nlmsg, len);
  EXPECT_FALSE(NLMSG_OK(nlmsg, len));
}

INSTANTIATE_TEST_SUITE_P(
    RouteNetlinkSocket, RouteNetlinkSocketGetNeigh,
    testing::Values(
        RouteNetlinkSocketGetNeighParam{
            .test_case_name = "invalid_state",
            .header =
                ndmsg{
                    .ndm_family = AF_INET,
                    .ndm_ifindex = static_cast<int32_t>(if_nametoindex("lo")),
                    .ndm_state = NUD_REACHABLE,
                    .ndm_type = RTN_UNSPEC,
                },
            .addr_and_family = std::optional(std::make_pair("192.0.2.1", AF_INET)),
            .expected_err = -EINVAL,
        },
        RouteNetlinkSocketGetNeighParam{
            .test_case_name = "invalid_type",
            .header =
                ndmsg{
                    .ndm_family = AF_INET,
                    .ndm_ifindex = static_cast<int32_t>(if_nametoindex("lo")),
                    .ndm_state = NUD_NONE,
                    .ndm_type = RTN_BROADCAST,
                },
            .addr_and_family = std::optional(std::make_pair("192.0.2.1", AF_INET)),
            .expected_err = -EINVAL,
        },
        RouteNetlinkSocketGetNeighParam{
            .test_case_name = "missing_address",
            .header =
                ndmsg{
                    .ndm_family = AF_INET,
                    .ndm_ifindex = static_cast<int32_t>(if_nametoindex("lo")),
                    .ndm_state = NUD_NONE,
                    .ndm_type = RTN_UNSPEC,
                },
            .addr_and_family = std::nullopt,
            .expected_err = -EINVAL,
        },
        RouteNetlinkSocketGetNeighParam{
            .test_case_name = "invalid_address_family",
            .header =
                ndmsg{
                    .ndm_family = AF_LOCAL,
                    .ndm_ifindex = static_cast<int32_t>(if_nametoindex("lo")),
                    .ndm_state = NUD_NONE,
                    .ndm_type = RTN_UNSPEC,
                },
            .addr_and_family = std::optional(std::make_pair("192.0.2.1", AF_INET)),
            .expected_err = -EAFNOSUPPORT,
        },
        RouteNetlinkSocketGetNeighParam{
            .test_case_name = "address_family_mismatch",
            .header =
                ndmsg{
                    .ndm_family = AF_INET6,
                    .ndm_ifindex = static_cast<int32_t>(if_nametoindex("lo")),
                    .ndm_state = NUD_NONE,
                    .ndm_type = RTN_UNSPEC,
                },
            .addr_and_family = std::optional(std::make_pair("192.0.2.1", AF_INET)),
            .expected_err = -EINVAL,
        },
        RouteNetlinkSocketGetNeighParam{
            .test_case_name = "interface_not_found",
            .header =
                ndmsg{
                    .ndm_family = AF_INET,
                    .ndm_ifindex = 99999,
                    .ndm_state = NUD_NONE,
                    .ndm_type = RTN_UNSPEC,
                },
            .addr_and_family = std::optional(std::make_pair("192.0.2.1", AF_INET)),
            .expected_err = -ENODEV,
        },
        RouteNetlinkSocketGetNeighParam{
            .test_case_name = "neighbor_not_found",
            .header =
                ndmsg{
                    .ndm_family = AF_INET,
                    .ndm_ifindex = static_cast<int32_t>(if_nametoindex("lo")),
                    .ndm_state = NUD_NONE,
                    .ndm_type = RTN_UNSPEC,
                },
            .addr_and_family = std::optional(std::make_pair("192.0.2.1", AF_INET)),
            .expected_err = -ENOENT,
        }),
    [](const testing::TestParamInfo<RouteNetlinkSocketGetNeigh::ParamType>& info) {
      return info.param.test_case_name;
    });

TEST_F(RouteNetlinkSocketTestWithInterface, CreateAndProbeNeighbor) {
  int ifindex = if_nametoindex(kTestIfName);
  ASSERT_GT(ifindex, 0);

  const char* kNeighborAddrStr = "192.168.0.10";
  in_addr neighbor_addr;
  ASSERT_EQ(inet_pton(AF_INET, kNeighborAddrStr, &neighbor_addr), 1);

  uint8_t lladdr[ETH_ALEN] = {0x02, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E};

  auto receive_ack = [this](uint32_t seq) {
    char recv_buf[4096];
    ssize_t len = recv(nl_sock_.get(), &recv_buf, sizeof(recv_buf), 0);
    ASSERT_GT(len, 0) << strerror(errno);

    nlmsghdr* nlmsg = reinterpret_cast<nlmsghdr*>(recv_buf);
    ASSERT_TRUE(NLMSG_OK(nlmsg, static_cast<uint32_t>(len)));
    EXPECT_EQ(nlmsg->nlmsg_type, NLMSG_ERROR);
    EXPECT_EQ(nlmsg->nlmsg_seq, seq);
    nlmsgerr* err_data = reinterpret_cast<nlmsgerr*>(NLMSG_DATA(nlmsg));
    EXPECT_EQ(err_data->error, 0);
  };

  auto get_and_check_state = [this](int ifindex, const in_addr& addr,
                                    uint16_t allowable_states_mask) {
    test_helper::NetlinkEncoder encoder(RTM_GETNEIGH, NLM_F_REQUEST);
    uint32_t seq = encoder.sequence();
    encoder.Write(ndmsg{
        .ndm_family = AF_INET,
        .ndm_ifindex = ifindex,
    });
    encoder.AddRtAttr(NDA_DST, addr);
    ASSERT_THAT(SendMsg(encoder), SyscallSucceeds());

    char recv_buf[4096];
    ssize_t len = recv(nl_sock_.get(), &recv_buf, sizeof(recv_buf), 0);
    ASSERT_GT(len, 0) << strerror(errno);

    nlmsghdr* nlmsg = reinterpret_cast<nlmsghdr*>(recv_buf);
    ASSERT_TRUE(NLMSG_OK(nlmsg, static_cast<uint32_t>(len)));

    ASSERT_EQ(nlmsg->nlmsg_type, RTM_NEWNEIGH);
    EXPECT_EQ(nlmsg->nlmsg_seq, seq);

    ndmsg* ndm = reinterpret_cast<ndmsg*>(NLMSG_DATA(nlmsg));
    EXPECT_EQ(ndm->ndm_state, ndm->ndm_state & allowable_states_mask);
  };

  // Create a static neighbor entry and check that it's in the PERMANENT state.

  {
    test_helper::NetlinkEncoder encoder(RTM_NEWNEIGH,
                                        NLM_F_REQUEST | NLM_F_CREATE | NLM_F_REPLACE | NLM_F_ACK);
    uint32_t seq = encoder.sequence();
    encoder.Write(ndmsg{
        .ndm_family = AF_INET,
        .ndm_ifindex = ifindex,
        .ndm_state = NUD_PERMANENT,
    });
    encoder.AddRtAttr(NDA_DST, neighbor_addr);
    encoder.AddRtAttr(NDA_LLADDR, lladdr);
    ASSERT_THAT(SendMsg(encoder), SyscallSucceeds());
    ASSERT_NO_FATAL_FAILURE(receive_ack(seq));
  }

  ASSERT_NO_FATAL_FAILURE(get_and_check_state(ifindex, neighbor_addr, NUD_PERMANENT));

  // Manually probe the neighbor entry and check that it's in the PROBE or
  // FAILED state. The latter can theoretically arise if the test is descheduled
  // for too long between the probe request and state check.

  {
    test_helper::NetlinkEncoder encoder(RTM_NEWNEIGH, NLM_F_REQUEST | NLM_F_REPLACE | NLM_F_ACK);
    uint32_t seq = encoder.sequence();
    encoder.Write(ndmsg{
        .ndm_family = AF_INET,
        .ndm_ifindex = ifindex,
        .ndm_state = NUD_PROBE,
    });
    encoder.AddRtAttr(NDA_DST, neighbor_addr);
    ASSERT_THAT(SendMsg(encoder), SyscallSucceeds());
    ASSERT_NO_FATAL_FAILURE(receive_ack(seq));
  }

  ASSERT_NO_FATAL_FAILURE(get_and_check_state(ifindex, neighbor_addr, NUD_PROBE | NUD_FAILED));

  // No need to clean up the neighbor; the interface is reset after test
  // completion.
}

struct RouteNetlinkSocketStrictOptParam {
  bool strict;
  std::optional<int> expected_err;
};

class RouteNetlinkSocketStrictOpt
    : public NetlinkRouteTest,
      public testing::WithParamInterface<RouteNetlinkSocketStrictOptParam> {};

TEST_P(RouteNetlinkSocketStrictOpt, MalformedDumpRequest) {
  const auto [strict, expected_err] = GetParam();
  if (strict) {
    SetStrictCheck();  // Relaxed is the default.
  }
  // Construct a bad Netlink message where the header and attribute don't match
  // the message type in the Netlink header.
  test_helper::NetlinkEncoder encoder(RTM_GETROUTE, NLM_F_REQUEST | NLM_F_DUMP);
  encoder.Write(ifinfomsg{
      .ifi_family = AF_INET,
  });
  uint32_t mask = RTEXT_FILTER_VF;
  encoder.AddRtAttr(IFLA_EXT_MASK, mask);
  ASSERT_THAT(SendMsg(encoder), SyscallSucceeds());
  struct {
    nlmsghdr hdr;
    // Note: if `nlmsg_type` is
    // * `NLMSG_DONE`: the response is `NLM_F_MULTIPART` and this is the raw
    //   error payload
    // * `NLMSG_ERROR`: this is the first field of `nlmsgerr`
    // * anything else: we don't access this field
    //
    // Linux returns the former when an error arises while handling a dump
    // request whereas Starnix returns the latter.
    //
    // TODO(https://fxbug.dev/491914658): this test only needs to handle
    // `NLMSG_DONE` once this behavior is aligned.
    int err;
  } response;
  ssize_t received = recv(nl_sock_.get(), &response, sizeof(response), 0);
  ASSERT_THAT(received, SyscallSucceeds());
  ASSERT_EQ(static_cast<size_t>(received), sizeof(response));
  if (expected_err) {
    ASSERT_TRUE(response.hdr.nlmsg_type == NLMSG_ERROR || response.hdr.nlmsg_type == NLMSG_DONE);
    EXPECT_EQ(response.err, *expected_err);
  } else {
    ASSERT_NE(response.hdr.nlmsg_type, NLMSG_ERROR);
    EXPECT_TRUE(response.hdr.nlmsg_type != NLMSG_DONE || response.err == 0);
  }
}

INSTANTIATE_TEST_SUITE_P(
    RouteNetlinkSocket, RouteNetlinkSocketStrictOpt,
    testing::Values(
        RouteNetlinkSocketStrictOptParam{
            .strict = true,
            .expected_err = std::make_optional(-EINVAL),
        },
        RouteNetlinkSocketStrictOptParam{
            .strict = false,
            .expected_err = std::nullopt,
        }),
    [](const testing::TestParamInfo<RouteNetlinkSocketStrictOpt::ParamType>& info) {
      return info.param.strict ? "strict" : "relaxed";
    });

TEST(NetlinkSocket, RecvMsg) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping suite.";
  }
  int fd = socket(AF_NETLINK, SOCK_RAW, NETLINK_GENERIC);
  ASSERT_GT(fd, 0);
  test_helper::NetlinkEncoder encoder(GENL_ID_CTRL, NLM_F_REQUEST);
  encoder.BeginGenetlinkHeader(CTRL_CMD_GETFAMILY);
  encoder.BeginNla(CTRL_ATTR_FAMILY_NAME);
  encoder.Write(TASKSTATS_GENL_NAME);
  encoder.EndNla();
  iovec iov = {};
  encoder.Finalize(iov);
  struct msghdr header = {};
  header.msg_iov = &iov;
  header.msg_iovlen = 1;

  ASSERT_EQ(sendmsg(fd, &header, 0), static_cast<ssize_t>(iov.iov_len));
  iov.iov_len = 0;
  ssize_t received = recvmsg(fd, &header, MSG_PEEK | MSG_TRUNC);
  ASSERT_GT(static_cast<size_t>(received), sizeof(nlmsghdr));
  struct {
    nlmsghdr hdr;
    genlmsghdr genl;
    // Family ID
    nlattr id_attr;
    __u16 id;
    char padding;
    // Family name
    nlattr name_attr;
    char name[sizeof(TASKSTATS_GENL_NAME)];
    char padding_0;
    // We should get one multicast group.
    // It doesn't seem to matter what the ID
    // or name of the group is.
    nlattr multicast_group_attr;
  } input;
  iov.iov_len = sizeof(input);
  iov.iov_base = &input;
  received = recvmsg(fd, &header, 0);

  ASSERT_EQ(static_cast<size_t>(received), sizeof(input));
  ASSERT_EQ(input.id_attr.nla_type, CTRL_ATTR_FAMILY_ID);
  ASSERT_EQ(input.genl.cmd, CTRL_CMD_NEWFAMILY);
  ASSERT_EQ(input.name_attr.nla_type, CTRL_ATTR_FAMILY_NAME);
  ASSERT_FALSE(memcmp(input.name, TASKSTATS_GENL_NAME, sizeof(input.name)));
  ASSERT_EQ(input.multicast_group_attr.nla_type, CTRL_ATTR_MCAST_GROUPS);
  struct {
    nlmsghdr hdr;
    genlmsghdr genl;
  } input_2;

  // Connect to TASKSTATS
  encoder.StartMessage(input.id, NLM_F_REQUEST);
  // We don't parse commands currently, so this number is arbitrary.
  encoder.BeginGenetlinkHeader(42);
  encoder.Finalize(iov);
  ASSERT_EQ(sendmsg(fd, &header, 0), static_cast<ssize_t>(iov.iov_len));
  iov.iov_base = &input_2;
  iov.iov_len = sizeof(input_2);
  // TASKSTATS payload
  received = recvmsg(fd, &header, 0);
  ASSERT_EQ(static_cast<size_t>(received), sizeof(input_2));
  ASSERT_EQ(input_2.hdr.nlmsg_type, input.id);
  // ACK payload
  received = recvmsg(fd, &header, 0);
  ASSERT_EQ(static_cast<size_t>(received), sizeof(input_2));
  ASSERT_EQ(input_2.hdr.nlmsg_type, NLMSG_ERROR);
}

TEST(NetlinkSocket, FamilyMissing) {
  int fd = socket(AF_NETLINK, SOCK_RAW, NETLINK_GENERIC);
  ASSERT_GT(fd, 0);
  test_helper::NetlinkEncoder encoder(GENL_ID_CTRL, NLM_F_REQUEST);
  encoder.BeginGenetlinkHeader(CTRL_CMD_GETFAMILY);
  encoder.BeginNla(CTRL_ATTR_FAMILY_NAME);
  encoder.Write("Hyainailouridae");
  encoder.EndNla();
  iovec iov = {};
  encoder.Finalize(iov);
  struct msghdr header = {};
  header.msg_iov = &iov;
  header.msg_iovlen = 1;

  ASSERT_EQ(sendmsg(fd, &header, 0), static_cast<ssize_t>(iov.iov_len));

  nlmsghdr* orig_nlmsghdr = static_cast<nlmsghdr*>(iov.iov_base);
  iov.iov_len = 0;
  ssize_t received = recvmsg(fd, &header, MSG_PEEK | MSG_TRUNC);
  ASSERT_GT(static_cast<size_t>(received), sizeof(nlmsghdr));
  struct {
    nlmsghdr hdr;
    nlmsgerr err;
  } input;
  iov.iov_len = sizeof(input);
  iov.iov_base = &input;
  received = recvmsg(fd, &header, 0);

  ASSERT_EQ(static_cast<size_t>(received), sizeof(input));
  ASSERT_EQ(input.hdr.nlmsg_type, NLMSG_ERROR);
  ASSERT_EQ(input.err.error, -ENOENT);
  ASSERT_FALSE(memcmp(&input.err.msg, orig_nlmsghdr, sizeof(nlmsghdr)));
}

TEST(NetlinkSocket, NlctrlFamily) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping suite.";
  }

  int fd = socket(AF_NETLINK, SOCK_RAW, NETLINK_GENERIC);
  ASSERT_GT(fd, 0);
  constexpr char kNlctrl[] = "nlctrl";
  test_helper::NetlinkEncoder encoder(GENL_ID_CTRL, NLM_F_REQUEST);
  encoder.BeginGenetlinkHeader(CTRL_CMD_GETFAMILY);
  encoder.BeginNla(CTRL_ATTR_FAMILY_NAME);
  encoder.Write(kNlctrl);
  encoder.EndNla();
  iovec iov = {};
  encoder.Finalize(iov);
  struct msghdr header = {};
  header.msg_iov = &iov;
  header.msg_iovlen = 1;

  ASSERT_EQ(sendmsg(fd, &header, 0), static_cast<ssize_t>(iov.iov_len));

  iov.iov_len = 0;
  ssize_t received = recvmsg(fd, &header, MSG_PEEK | MSG_TRUNC);
  ASSERT_GT(static_cast<size_t>(received), sizeof(nlmsghdr));
  struct {
    nlmsghdr hdr;
    genlmsghdr genl;
    // Family ID
    nlattr id_attr;
    __u16 id;
    char padding;
    // Family name
    nlattr name_attr;
    char name[sizeof(kNlctrl)];
    char padding_0;
  } input;
  iov.iov_len = sizeof(input);
  iov.iov_base = &input;
  received = recvmsg(fd, &header, 0);

  ASSERT_EQ(static_cast<size_t>(received), sizeof(input));
  ASSERT_EQ(input.id_attr.nla_type, CTRL_ATTR_FAMILY_ID);
  ASSERT_EQ(input.id, GENL_ID_CTRL);
  ASSERT_EQ(input.genl.cmd, CTRL_CMD_NEWFAMILY);
  ASSERT_EQ(input.name_attr.nla_type, CTRL_ATTR_FAMILY_NAME);
  ASSERT_FALSE(memcmp(input.name, "nlctrl", sizeof(input.name)));
}

// Regression test for syzkaller finding on https://fxbug.dev/387599168.
TEST_F(NetlinkRouteTest, IflaCacheInfoMissingBody) {
  SetStrictCheck();
  // The reproducer sends a message type 0x16, which is RTM_GETADDR.
  test_helper::NetlinkEncoder encoder(RTM_GETADDR, NLM_F_REQUEST);
  encoder.Write(ifaddrmsg{
      .ifa_family = AF_INET6,
  });

  // Correct size for cache info is 16.
  uint16_t cacheinfo = 0;

  encoder.AddRtAttr(IFA_CACHEINFO, cacheinfo);
  // We expect the syscall to succeed, e.g. not crash.
  ASSERT_THAT(SendMsg(encoder), SyscallSucceeds());

  struct {
    nlmsghdr hdr;
    nlmsgerr err;
  } response;
  ssize_t received = recv(nl_sock_.get(), &response, sizeof(response), 0);
  ASSERT_THAT(received, SyscallSucceeds());
  ASSERT_EQ(static_cast<size_t>(received), sizeof(response));
  EXPECT_EQ(response.hdr.nlmsg_type, NLMSG_ERROR);
  ASSERT_THAT(response.err.error, GetNlaMessageParseErrorCode());
}

// Netlink messages that are malformed, are expected to generate an error
// response, as opposed to failing the syscall.
TEST_F(NetlinkRouteTest, MalformedMessageCausesErrorReturn) {
  test_helper::NetlinkEncoder encoder(RTM_GETADDR, NLM_F_REQUEST);
  // The body should contain struct ifaddrmsg. But instead just write a single
  // field from it.
  ifaddrmsg ifa = {
      .ifa_family = AF_INET6,
  };
  encoder.Write(ifa.ifa_family);
  // We expect the syscall to succeed, e.g. not crash.
  ASSERT_THAT(SendMsg(encoder), SyscallSucceeds());
  struct {
    nlmsghdr hdr;
    nlmsgerr err;
  } response;
  ssize_t received = recv(nl_sock_.get(), &response, sizeof(response), 0);
  ASSERT_THAT(received, SyscallSucceeds());
  ASSERT_EQ(static_cast<size_t>(received), sizeof(response));
  EXPECT_EQ(response.hdr.nlmsg_type, NLMSG_ERROR);
  EXPECT_EQ(response.err.error, -EINVAL);
}

TEST_F(NetlinkRouteTest, MessageWithIncompleteHeader) {
  struct nlmsghdr header = {.nlmsg_len = sizeof(uint32_t)};
  ASSERT_THAT(send(nl_sock_.get(), &header.nlmsg_len, sizeof(header.nlmsg_len), 0),
              SyscallSucceeds());
  // A message with an incomplete header generates no response.
  nlmsghdr response;
  ASSERT_THAT(recv(nl_sock_.get(), &response, sizeof(response), MSG_TRUNC | MSG_DONTWAIT),
              SyscallFailsWithErrno(EAGAIN));
  // Following message still works.
  CheckNetlinkAlive();
}

TEST_F(NetlinkRouteTest, HeaderOnlyMessageWithBadLength) {
  struct nlmsghdr header = {
      .nlmsg_len = sizeof(nlmsghdr) + sizeof(ifaddrmsg),
      .nlmsg_type = RTM_GETADDR,
      .nlmsg_flags = NLM_F_REQUEST | NLM_F_ACK,
      .nlmsg_seq = 123456,
      .nlmsg_pid = static_cast<uint32_t>(getpid()),
  };
  ASSERT_THAT(send(nl_sock_.get(), &header.nlmsg_len, sizeof(header.nlmsg_len), 0),
              SyscallSucceeds());
  // This is a malformed message it should not generate a response.
  nlmsghdr response;
  ASSERT_THAT(recv(nl_sock_.get(), &response, sizeof(response), MSG_TRUNC | MSG_DONTWAIT),
              SyscallFailsWithErrno(EAGAIN));
  // Following message still works.
  CheckNetlinkAlive();
}

// Regression test for syzkaller finding on https://fxbug.dev/387662319.
TEST_F(NetlinkRouteTest, IncompleteRouteFlow) {
  SetStrictCheck();
  test_helper::NetlinkEncoder encoder(RTM_GETROUTE, NLM_F_REQUEST);
  encoder.Write(rtmsg{
      .rtm_family = AF_INET,
  });
  // Correct size for flow info is a u32.
  uint16_t bad_attr = 0;
  encoder.AddRtAttr(RTA_FLOW, bad_attr);
  ASSERT_THAT(SendMsg(encoder), SyscallSucceeds());
  struct {
    nlmsghdr hdr;
    nlmsgerr err;
  } response;
  ssize_t received = recv(nl_sock_.get(), &response, sizeof(response), 0);
  ASSERT_THAT(received, SyscallSucceeds());
  ASSERT_EQ(static_cast<size_t>(received), sizeof(response));
  EXPECT_EQ(response.hdr.nlmsg_type, NLMSG_ERROR);
  ASSERT_THAT(response.err.error, GetNlaMessageParseErrorCode());
}

// Regression test for syzkaller finding on https://fxbug.dev/393764263.
TEST_F(NetlinkGenericTest, PartialCtrlAttrPolicy) {
  test_helper::NetlinkEncoder encoder(GENL_ID_CTRL, 0);
  encoder.Write(genlmsghdr{
      // CTRL_CMD_GET_POLICY. Not available in header.
      .cmd = 10,
  });
  encoder.Write(nlattr{
      .nla_len = 8,
      .nla_type = 9,  // CTRL_ATTR_OP_POLICY. Not available in header.
  });
  encoder.Write(nlattr{
      // Invalid length less than 4.
      .nla_len = 2,
  });
  ASSERT_THAT(SendMsg(encoder), SyscallSucceeds());
}

// Regression test for syzkaller finding on https://fxbug.dev/389503570.
TEST_F(NetlinkGenericTest, ShortTaskstatsMessage) {
  test_helper::NetlinkEncoder encoder;
  std::optional<uint16_t> taskstats = GetFamily(encoder, TASKSTATS_GENL_NAME);
  ASSERT_TRUE(taskstats.has_value());
  encoder.StartMessage(*taskstats, NLM_F_DUMP | NLM_F_ACK);
  ASSERT_THAT(SendMsg(encoder), SyscallSucceeds());
  // This message generates a response. We're not fully asserting on it because
  // the implementation is stubbed. Expecting a response as opposed to a starnix
  // crash covers the regression for now. See https://fxbug.dev/339675153.
  uint8_t buffer[10];
  ASSERT_THAT(recv(nl_sock_.get(), &buffer, sizeof(buffer), MSG_TRUNC), SyscallSucceeds());
}

}  // namespace
