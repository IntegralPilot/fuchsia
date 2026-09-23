// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "addr.h"

#include <arpa/inet.h>

#include <gtest/gtest.h>

#if PACKET_SOCKETS
#include <netpacket/packet.h>

#include "api_abstraction.h"
#include "testutil.h"
#endif  // PACKET_SOCKETS

#define EXPECT_NO_VALUE(expr)                     \
  do {                                            \
    const std::optional val = expr;               \
    EXPECT_FALSE(val.has_value()) << val.value(); \
  } while (0)

#define EXPECT_NO_VALUE_FMT(expr, format)                 \
  do {                                                    \
    const std::optional val = expr;                       \
    EXPECT_FALSE(val.has_value()) << format(val.value()); \
  } while (0)

TEST(AddrTest, TestParseFormatNoPort) {
  {
    std::optional addr = Parse("192.168.0.1", std::nullopt);
    EXPECT_TRUE(addr.has_value());
    EXPECT_EQ(addr.value().ss_family, AF_INET);
    EXPECT_EQ(Format(addr.value()), "192.168.0.1");
  }
  {
    std::optional addr = Parse("FF01:0:0:0:0:0:0:1", std::nullopt);
    EXPECT_TRUE(addr.has_value());
    EXPECT_EQ(addr.value().ss_family, AF_INET6);
    EXPECT_EQ(Format(addr.value()), "[ff01::1]");
  }
  EXPECT_NO_VALUE_FMT(Parse("bad", std::nullopt), Format);
  EXPECT_NO_VALUE_FMT(Parse("192.168.0.", std::nullopt), Format);
  EXPECT_NO_VALUE_FMT(Parse("FF01::jk", std::nullopt), Format);
}

TEST(AddrTest, TestParseIpv4WithScopeFormat) {
  {
    auto [addr, id] = ParseIpv4WithScope("192.168.0.1%2");
    EXPECT_TRUE(addr.has_value());
    EXPECT_TRUE(id.has_value());
    char buf[INET_ADDRSTRLEN] = {};
    EXPECT_STREQ(inet_ntop(AF_INET, &addr.value(), buf, sizeof(buf)), "192.168.0.1");
    EXPECT_EQ(id.value(), 2);
  }
  {
    auto [addr, id] = ParseIpv4WithScope("192.168.0.1");
    EXPECT_TRUE(addr.has_value());
    EXPECT_NO_VALUE(id);
    char buf[INET_ADDRSTRLEN] = {};
    EXPECT_STREQ(inet_ntop(AF_INET, &addr.value(), buf, sizeof(buf)), "192.168.0.1");
  }
  {
    auto [addr, id] = ParseIpv4WithScope("FF01:0:0:0:0:0:0:1%3");
    char buf[INET_ADDRSTRLEN] = {};
    EXPECT_FALSE(addr.has_value()) << inet_ntop(AF_INET, &addr.value(), buf, sizeof(buf));
    EXPECT_TRUE(id.has_value());
    EXPECT_EQ(id.value(), 3);
  }
  {
    auto [addr, id] = ParseIpv4WithScope("192.168.0.1%abc");
    EXPECT_TRUE(addr.has_value());
    EXPECT_NO_VALUE(id);
    char buf[INET_ADDRSTRLEN] = {};
    EXPECT_STREQ(inet_ntop(AF_INET, &addr.value(), buf, sizeof(buf)), "192.168.0.1");
  }
}

TEST(AddrTest, TestParseFormat) {
  {
    std::optional addr = Parse("192.168.0.1:2020");
    EXPECT_TRUE(addr.has_value());
    EXPECT_EQ(Format(addr.value()), "192.168.0.1:2020");
    EXPECT_EQ(addr.value().ss_family, AF_INET);
    const sockaddr_in& addr_in = *reinterpret_cast<struct sockaddr_in*>(&addr.value());
    union {
      uint8_t b[4];
      uint32_t x;
    } ip4 = {{192, 168, 0, 1}};
    EXPECT_EQ(addr_in.sin_addr.s_addr, ip4.x);
    EXPECT_EQ(addr_in.sin_port, htons(2020));
  }
  {
    std::optional addr = Parse("[FF01:0:0:0:0:0:0:1]:4040");
    EXPECT_TRUE(addr.has_value());
    EXPECT_EQ(Format(addr.value()), "[ff01::1]:4040");
    EXPECT_EQ(addr.value().ss_family, AF_INET6);
    const sockaddr_in6& addr_in6 = *reinterpret_cast<struct sockaddr_in6*>(&addr.value());
    const uint8_t cmp[] = {0xFF, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                           0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01};
    EXPECT_EQ(memcmp(addr_in6.sin6_addr.s6_addr, cmp, 16), 0);
    EXPECT_EQ(addr_in6.sin6_port, htons(4040));
  }
  {
    std::optional addr = Parse("[ff02::2%100]:3");
    EXPECT_TRUE(addr.has_value());
    EXPECT_EQ(Format(addr.value()), "[ff02::2%100]:3");
    EXPECT_EQ(addr.value().ss_family, AF_INET6);
    const sockaddr_in6& addr_in6 = *reinterpret_cast<struct sockaddr_in6*>(&addr.value());
    EXPECT_EQ(addr_in6.sin6_scope_id, 100u);
  }

  EXPECT_NO_VALUE_FMT(Parse(""), Format);
  EXPECT_NO_VALUE_FMT(Parse(":0"), Format);
  EXPECT_NO_VALUE_FMT(Parse("any::0"), Format);
  EXPECT_NO_VALUE_FMT(Parse("[]"), Format);
  EXPECT_NO_VALUE_FMT(Parse("[]:0"), Format);
  EXPECT_NO_VALUE_FMT(Parse("[::]"), Format);
  EXPECT_NO_VALUE_FMT(Parse("[::]::0"), Format);
}

TEST(AddrTest, TestAddrlen) {
  sockaddr_storage addr_unspec;
  addr_unspec.ss_family = AF_UNSPEC;
  EXPECT_EQ(AddrLen(addr_unspec), sizeof(sockaddr_storage));

  std::optional addr_v4 = Parse("192.168.0.1:2020");
  ASSERT_TRUE(addr_v4.has_value());
  ASSERT_EQ(addr_v4.value().ss_family, AF_INET);
  EXPECT_EQ(AddrLen(addr_v4.value()), sizeof(sockaddr_in));

  std::optional addr_v6 = Parse("[ff02::2%100]:3");
  ASSERT_TRUE(addr_v6.has_value());
  ASSERT_EQ(addr_v6.value().ss_family, AF_INET6);
  EXPECT_EQ(AddrLen(addr_v6.value()), sizeof(sockaddr_in6));
}

#if PACKET_SOCKETS
TEST(AddrTest, TestParseSockAddrLl) {
  constexpr unsigned int kIfIndex = 5;
  constexpr uint16_t kIpv4Protocol = 2048;
  TestApi api;
  EXPECT_CALL(api, if_nametoindex(testing::_)).WillOnce([](const char* ifname) {
    EXPECT_EQ(std::string(ifname), "myinterfacename");
    return kIfIndex;
  });

  std::optional actual_addr = ParseSockAddrLlFromArg("2048:myinterfacename", &api);
  const struct sockaddr_ll expected_addr = {
      .sll_family = AF_PACKET,
      .sll_protocol = htons(kIpv4Protocol),
      .sll_ifindex = kIfIndex,
  };
  ASSERT_TRUE(actual_addr.has_value());
  EXPECT_EQ(actual_addr->sll_family, expected_addr.sll_family);
  EXPECT_EQ(actual_addr->sll_protocol, expected_addr.sll_protocol);
  EXPECT_EQ(actual_addr->sll_ifindex, expected_addr.sll_ifindex);
}

TEST(AddrTest, TestFormatHardwareAddress) {
  const unsigned char addr_bytes[8] = {0x00, 0x0f, 0xff, 0x12, 0x34, 0x56, 0x00, 0x00};

  std::stringstream o1;
  o1 << PrintHardwareAddress{6, addr_bytes};
  EXPECT_EQ(o1.str(), "[00:0f:ff:12:34:56]");

  std::stringstream o2;
  o2 << PrintHardwareAddress{0, addr_bytes};
  EXPECT_EQ(o2.str(), "[]");
}
#endif  // PACKET_SOCKETS
