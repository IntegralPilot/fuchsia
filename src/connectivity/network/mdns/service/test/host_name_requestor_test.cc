// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/agents/host_name_requestor.h"

#include <lib/zx/time.h>

#include <gtest/gtest.h>

#include "src/connectivity/network/mdns/service/test/agent_test.h"
#include "src/connectivity/network/mdns/service/test/fake_clock.h"

namespace mdns::test {

class HostNameRequestorTest : public AgentTest {
 public:
  HostNameRequestorTest() = default;

 protected:
  class Subscriber : public Mdns::HostNameSubscriber {
   public:
    void AddressesChanged(std::vector<HostAddress> addresses) override {
      addresses_changed_called_ = true;
      addresses_ = std::move(addresses);
    }

    bool addresses_changed_called() {
      bool result = addresses_changed_called_;
      addresses_changed_called_ = false;
      return result;
    }

    std::vector<HostAddress> addresses() { return std::move(addresses_); }

   private:
    bool addresses_changed_called_ = false;
    std::vector<HostAddress> addresses_;
  };
};

bool VerifyAddresses(std::vector<inet::IpAddress> expected, std::vector<HostAddress> value) {
  for (const auto& host_address : value) {
    auto iter = std::find(expected.begin(), expected.end(), host_address.address());
    if (iter == expected.end()) {
      std::cerr << "Address " << host_address.address() << " was not expected.\n";
      return false;
    }

    expected.erase(iter);
  }

  if (!expected.empty()) {
    std::cerr << expected.size() << " expected addresses not found.\n";
    return false;
  }

  return true;
}

bool VerifyAddresses(std::vector<HostAddress> expected, std::vector<HostAddress> value) {
  for (const auto& host_address : value) {
    auto iter = std::find(expected.begin(), expected.end(), host_address);
    if (iter == expected.end()) {
      std::cerr << "Address " << host_address.address() << " was not expected.\n";
      return false;
    }

    expected.erase(iter);
  }

  if (!expected.empty()) {
    std::cerr << expected.size() << " expected addresses not found.\n";
    return false;
  }

  return true;
}

const DnsName kHostName("testhostname");
const DnsName kHostFullName("testhostname.local.");
const std::vector<inet::IpAddress> kAddresses{inet::IpAddress(192, 168, 1, 200),
                                              inet::IpAddress(0xfe80, 200)};
const std::vector<inet::IpAddress> kOtherAddresses{inet::IpAddress(192, 168, 1, 201),
                                                   inet::IpAddress(0xfe80, 201)};
const std::vector<HostAddress> kHostAddresses{
    HostAddress(inet::IpAddress(192, 168, 1, 200), 1, zx::sec(450)),
    HostAddress(inet::IpAddress(0xfe80, 200), 1, zx::sec(450))};
constexpr uint32_t kInterfaceId = 1;
constexpr bool kIncludeLocal = true;
constexpr bool kExcludeLocal = false;
constexpr bool kIncludeLocalProxies = true;
constexpr bool kExcludeLocalProxies = false;

// Tests nominal startup behavior of the requestor.
TEST_F(HostNameRequestorTest, Nominal) {
  HostNameRequestor under_test(this, kHostName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  // Expect A and AAAA questions on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kHostFullName, DnsType::kA);
  ExpectQuestion(message.get(), kHostFullName, DnsType::kAaaa);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectNoOther();
}

// Tests behavior of the requestor when a host responds to the initial questions.
TEST_F(HostNameRequestorTest, Response) {
  HostNameRequestor under_test(this, kHostName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);
  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Expect A and AAAA questions on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kHostFullName, DnsType::kA);
  ExpectQuestion(message.get(), kHostFullName, DnsType::kAaaa);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectNoOther();

  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Respond.
  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 200, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), kInterfaceId, Media::kWireless, IpVersions::kV4);
  for (const auto& address : kAddresses) {
    under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                               sender_address);
  }

  under_test.EndOfMessage();

  // Expect to see the addresses from the response.
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(VerifyAddresses(kAddresses, subscriber.addresses()));

  for (const auto& address : kAddresses) {
    ExpectRenewCall(DnsResource(kHostFullName, address));
  }

  ExpectNoOther();
}

// Tests behavior of the requestor when a host responds multiple times to the initial questions.
TEST_F(HostNameRequestorTest, Duplicate) {
  HostNameRequestor under_test(this, kHostName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);
  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Expect A and AAAA questions on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kHostFullName, DnsType::kA);
  ExpectQuestion(message.get(), kHostFullName, DnsType::kAaaa);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectNoOther();

  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Respond 4 times.
  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 200, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), kInterfaceId, Media::kWireless, IpVersions::kV4);
  for (size_t i = 0; i < 4; ++i) {
    for (const auto& address : kAddresses) {
      under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                                 sender_address);
    }
  }

  under_test.EndOfMessage();

  // Expect to see the addresses just once. The renewals happen for each response.
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(VerifyAddresses(kAddresses, subscriber.addresses()));
  for (size_t i = 0; i < 4; ++i) {
    for (const auto& address : kAddresses) {
      ExpectRenewCall(DnsResource(kHostFullName, address));
    }
  }

  ExpectNoOther();

  // Respond again with the same addresses.
  for (const auto& address : kAddresses) {
    under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                               sender_address);
  }

  under_test.EndOfMessage();

  // Expect no new addresses. We still see the renewals.
  EXPECT_FALSE(subscriber.addresses_changed_called());
  for (const auto& address : kAddresses) {
    ExpectRenewCall(DnsResource(kHostFullName, address));
  }

  ExpectNoOther();
}

// Tests behavior of the requestor when the addresses change.
TEST_F(HostNameRequestorTest, Change) {
  HostNameRequestor under_test(this, kHostName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);
  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Expect A and AAAA questions on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kHostFullName, DnsType::kA);
  ExpectQuestion(message.get(), kHostFullName, DnsType::kAaaa);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectNoOther();

  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Respond.
  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 200, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), kInterfaceId, Media::kWireless, IpVersions::kV4);
  for (const auto& address : kAddresses) {
    under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                               sender_address);
  }

  under_test.EndOfMessage();

  // Expect to get the addresses.
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(VerifyAddresses(kAddresses, subscriber.addresses()));
  for (const auto& address : kAddresses) {
    ExpectRenewCall(DnsResource(kHostFullName, address));
  }

  ExpectNoOther();

  // Respond with different addresses.
  for (const auto& address : kOtherAddresses) {
    under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                               sender_address);
  }

  under_test.EndOfMessage();

  // Expect to see the new addresses and the old ones.
  auto combined_addresses = kAddresses;
  std::copy(kOtherAddresses.begin(), kOtherAddresses.end(), std::back_inserter(combined_addresses));
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(VerifyAddresses(combined_addresses, subscriber.addresses()));
  for (const auto& address : kOtherAddresses) {
    ExpectRenewCall(DnsResource(kHostFullName, address));
  }

  ExpectNoOther();
}

// Tests the behavior of the requestor when an address record indicates addresses should be flushed.
TEST_F(HostNameRequestorTest, AddressCacheFlush) {
  FakeClock kayfabe;

  HostNameRequestor under_test(this, kHostName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);
  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Expect A and AAAA questions on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kHostFullName, DnsType::kA);
  ExpectQuestion(message.get(), kHostFullName, DnsType::kAaaa);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectNoOther();

  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Respond.
  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 200, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), kInterfaceId, Media::kWireless, IpVersions::kV4);
  for (const auto& address : kAddresses) {
    under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                               sender_address);
  }

  under_test.EndOfMessage();

  // Expect to get the addresses.
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(VerifyAddresses(kAddresses, subscriber.addresses()));
  for (const auto& address : kAddresses) {
    ExpectRenewCall(DnsResource(kHostFullName, address));
  }

  ExpectNoOther();

  // Make sure the above addresses are more than one second old.
  FakeClock::Advance(zx::sec(2));

  // Respond with different addresses with the first cache flush bit set for each resource type.
  bool cache_flush_a = true;
  bool cache_flush_aaaa = true;
  for (const auto& address : kOtherAddresses) {
    auto resource = DnsResource(kHostFullName, address);
    if (resource.type_ == DnsType::kA) {
      resource.cache_flush_ = cache_flush_a;
      cache_flush_a = false;
    } else {
      FX_DCHECK(resource.type_ == DnsType::kAaaa);
      resource.cache_flush_ = cache_flush_aaaa;
      cache_flush_aaaa = false;
    }

    under_test.ReceiveResource(resource, MdnsResourceSection::kAnswer, sender_address);
  }

  under_test.EndOfMessage();

  // Expect to see the new addresses.
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(VerifyAddresses(kOtherAddresses, subscriber.addresses()));
  for (const auto& address : kOtherAddresses) {
    ExpectRenewCall(DnsResource(kHostFullName, address));
  }

  ExpectNoOther();
}

// Tests behavior of the requestor when host address records expire.
TEST_F(HostNameRequestorTest, Expired) {
  HostNameRequestor under_test(this, kHostName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);
  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Expect A and AAAA questions on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kHostFullName, DnsType::kA);
  ExpectQuestion(message.get(), kHostFullName, DnsType::kAaaa);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectNoOther();

  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Respond.
  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 200, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), kInterfaceId, Media::kWireless, IpVersions::kV4);
  for (const auto& address : kAddresses) {
    under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                               sender_address);
  }

  under_test.EndOfMessage();

  // Expect to get the addresses.
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(VerifyAddresses(kAddresses, subscriber.addresses()));
  for (const auto& address : kAddresses) {
    ExpectRenewCall(DnsResource(kHostFullName, address));
  }

  ExpectNoOther();

  // Send expirations for received response.
  for (const auto& address : kAddresses) {
    auto resource = DnsResource(kHostFullName, address);
    resource.time_to_live_ = 0;
    under_test.ReceiveResource(resource, MdnsResourceSection::kExpired, sender_address);
  }

  under_test.EndOfMessage();

  // Expect an empty address list.
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(subscriber.addresses().empty());
  ExpectNoOther();
}

// Tests the the requestor quits after the last subscriber is removed.
TEST_F(HostNameRequestorTest, Quit) {
  // Need to |make_shared|, because |RemoveSelf| calls |shared_from_this|.
  auto under_test = std::make_shared<HostNameRequestor>(
      this, kHostName, Media::kBoth, IpVersions::kBoth, kExcludeLocal, kExcludeLocalProxies);
  SetAgent(*under_test);

  under_test->Start(kLocalHostFullName);

  // Expect A and AAAA questions on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kHostFullName, DnsType::kA);
  ExpectQuestion(message.get(), kHostFullName, DnsType::kAaaa);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectNoOther();

  // Add and remove a subscriber.
  Subscriber subscriber;
  under_test->AddSubscriber(&subscriber);

  under_test->RemoveSubscriber(&subscriber);

  // Removing the last subscriber posts a task that quits the agent, matching |InstanceRequestor|.
  // Invoke the task and expect the requestor to remove itself.
  ExpectPostTaskForTimeAndInvoke(zx::sec(0), zx::sec(0));
  ExpectRemoveAgentCall();
  ExpectNoOther();
}

// Tests behavior of the requestor when configured for wireless operation only.
TEST_F(HostNameRequestorTest, WirelessOnly) {
  HostNameRequestor under_test(this, kHostName, Media::kWireless, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);
  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Expect A and AAAA questions on start.
  auto message =
      ExpectOutboundMessage(ReplyAddress::Multicast(Media::kWireless, IpVersions::kBoth));
  ExpectQuestion(message.get(), kHostFullName, DnsType::kA);
  ExpectQuestion(message.get(), kHostFullName, DnsType::kAaaa);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectNoOther();

  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Respond on wired.
  ReplyAddress sender_address0(
      inet::SocketAddress(192, 168, 1, 200, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), kInterfaceId, Media::kWired, IpVersions::kV4);
  for (const auto& address : kAddresses) {
    under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                               sender_address0);
  }

  under_test.EndOfMessage();

  // Expect no addresses.
  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Respond on wireless.
  ReplyAddress sender_address1(
      inet::SocketAddress(192, 168, 1, 200, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), kInterfaceId, Media::kWireless, IpVersions::kV4);
  for (const auto& address : kAddresses) {
    under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                               sender_address1);
  }

  under_test.EndOfMessage();

  // Expect addresses.
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(VerifyAddresses(kAddresses, subscriber.addresses()));

  for (const auto& address : kAddresses) {
    ExpectRenewCall(DnsResource(kHostFullName, address));
  }

  ExpectNoOther();
}

// Tests behavior of the requestor when configured for wired operation only.
TEST_F(HostNameRequestorTest, WiredOnly) {
  HostNameRequestor under_test(this, kHostName, Media::kWired, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);
  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Expect A and AAAA questions on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kWired, IpVersions::kBoth));
  ExpectQuestion(message.get(), kHostFullName, DnsType::kA);
  ExpectQuestion(message.get(), kHostFullName, DnsType::kAaaa);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectNoOther();

  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Respond on wireless.
  ReplyAddress sender_address0(
      inet::SocketAddress(192, 168, 1, 200, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), kInterfaceId, Media::kWireless, IpVersions::kV4);
  for (const auto& address : kAddresses) {
    under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                               sender_address0);
  }

  under_test.EndOfMessage();

  // Expect no addresses.
  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Respond on wired.
  ReplyAddress sender_address1(
      inet::SocketAddress(192, 168, 1, 200, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), kInterfaceId, Media::kWired, IpVersions::kV4);
  for (const auto& address : kAddresses) {
    under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                               sender_address1);
  }

  under_test.EndOfMessage();

  // Expect addresses.
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(VerifyAddresses(kAddresses, subscriber.addresses()));

  for (const auto& address : kAddresses) {
    ExpectRenewCall(DnsResource(kHostFullName, address));
  }

  ExpectNoOther();
}

// Tests behavior of the requestor when configured for IPv4 operation only.
TEST_F(HostNameRequestorTest, V4Only) {
  HostNameRequestor under_test(this, kHostName, Media::kBoth, IpVersions::kV4, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);
  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Expect A and AAAA questions on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kV4));
  ExpectQuestion(message.get(), kHostFullName, DnsType::kA);
  ExpectQuestion(message.get(), kHostFullName, DnsType::kAaaa);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectNoOther();

  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Respond on IPv6.
  ReplyAddress sender_address0(inet::SocketAddress(0xfe80, 200, inet::IpPort::From_uint16_t(5353)),
                               inet::IpAddress(0xfe80, 100), kInterfaceId, Media::kWireless,
                               IpVersions::kV6);
  for (const auto& address : kAddresses) {
    under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                               sender_address0);
  }

  under_test.EndOfMessage();

  // Expect no addresses.
  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Respond on IPv4.
  ReplyAddress sender_address1(
      inet::SocketAddress(192, 168, 1, 200, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), kInterfaceId, Media::kWired, IpVersions::kV4);
  for (const auto& address : kAddresses) {
    under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                               sender_address1);
  }

  under_test.EndOfMessage();

  // Expect addresses.
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(VerifyAddresses(kAddresses, subscriber.addresses()));

  for (const auto& address : kAddresses) {
    ExpectRenewCall(DnsResource(kHostFullName, address));
  }

  ExpectNoOther();
}

// Tests behavior of the requestor when configured for IPv6 operation only.
TEST_F(HostNameRequestorTest, V6Only) {
  HostNameRequestor under_test(this, kHostName, Media::kBoth, IpVersions::kV6, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);
  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Expect A and AAAA questions on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kV6));
  ExpectQuestion(message.get(), kHostFullName, DnsType::kA);
  ExpectQuestion(message.get(), kHostFullName, DnsType::kAaaa);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectNoOther();

  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Respond on IPv4.
  ReplyAddress sender_address0(
      inet::SocketAddress(192, 168, 1, 200, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), kInterfaceId, Media::kWireless, IpVersions::kV4);
  for (const auto& address : kAddresses) {
    under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                               sender_address0);
  }

  under_test.EndOfMessage();

  // Expect no addresses.
  EXPECT_FALSE(subscriber.addresses_changed_called());

  // Respond on IPv6.
  ReplyAddress sender_address1(inet::SocketAddress(0xfe80, 200, inet::IpPort::From_uint16_t(5353)),
                               inet::IpAddress(0xfe80, 100), kInterfaceId, Media::kWired,
                               IpVersions::kV6);
  for (const auto& address : kAddresses) {
    under_test.ReceiveResource(DnsResource(kHostFullName, address), MdnsResourceSection::kAnswer,
                               sender_address1);
  }

  under_test.EndOfMessage();

  // Expect addresses.
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(VerifyAddresses(kAddresses, subscriber.addresses()));

  for (const auto& address : kAddresses) {
    ExpectRenewCall(DnsResource(kHostFullName, address));
  }

  ExpectNoOther();
}

// Tests discovery of the local host.
TEST_F(HostNameRequestorTest, LocalHost) {
  HostNameRequestor under_test(this, kLocalHostName, Media::kBoth, IpVersions::kBoth, kIncludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);
  SetLocalHostAddresses(kHostAddresses);

  under_test.Start(kLocalHostFullName);

  // Expect no questions, because we're requesting the local host.
  ExpectNoOther();

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(VerifyAddresses(kHostAddresses, subscriber.addresses()));
}

// Tests discovery of a local proxy host.
TEST_F(HostNameRequestorTest, LocalProxyHost) {
  HostNameRequestor under_test(this, kHostName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kIncludeLocalProxies);
  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  // Expect A and AAAA questions on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kHostFullName, DnsType::kA);
  ExpectQuestion(message.get(), kHostFullName, DnsType::kAaaa);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectNoOther();

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);
  EXPECT_FALSE(subscriber.addresses_changed_called());

  under_test.OnAddProxyHost(kHostFullName, kHostAddresses);
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(VerifyAddresses(kHostAddresses, subscriber.addresses()));

  under_test.OnRemoveProxyHost(kHostFullName);
  EXPECT_TRUE(subscriber.addresses_changed_called());
  EXPECT_TRUE(VerifyAddresses(std::vector<HostAddress>(), subscriber.addresses()));
}

// Tests that a local proxy host is not discovered if that feature is off.
TEST_F(HostNameRequestorTest, NoLocalProxyHost) {
  HostNameRequestor under_test(this, kHostName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  // Expect A and AAAA questions on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kHostFullName, DnsType::kA);
  ExpectQuestion(message.get(), kHostFullName, DnsType::kAaaa);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectNoOther();

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);
  EXPECT_FALSE(subscriber.addresses_changed_called());

  under_test.OnAddProxyHost(kHostFullName, kHostAddresses);
  EXPECT_FALSE(subscriber.addresses_changed_called());
}

// Regression test: subscribers that unsubscribe synchronously from their |AddressesChanged|
// callback must not invalidate the subscriber set iteration in |MaybeSendAddressesChanged|, and
// removal of the last subscriber must quit the agent through a posted task.
TEST_F(HostNameRequestorTest, SubscriberUnsubscribesDuringAddressesChanged) {
  // Need to |make_shared|, because removing the last subscriber posts a task that calls
  // |shared_from_this|.
  auto under_test = std::make_shared<HostNameRequestor>(
      this, kHostName, Media::kBoth, IpVersions::kBoth, kExcludeLocal, kExcludeLocalProxies);
  SetAgent(*under_test);

  under_test->Start(kLocalHostFullName);

  class UnsubscribingSubscriber : public Subscriber {
   public:
    explicit UnsubscribingSubscriber(HostNameRequestor& requestor) : requestor_(requestor) {}

    void AddressesChanged(std::vector<HostAddress> addresses) override {
      Subscriber::AddressesChanged(std::move(addresses));
      requestor_.RemoveSubscriber(this);
    }

   private:
    HostNameRequestor& requestor_;
  };

  UnsubscribingSubscriber subscriber_a(*under_test);
  UnsubscribingSubscriber subscriber_b(*under_test);
  under_test->AddSubscriber(&subscriber_a);
  under_test->AddSubscriber(&subscriber_b);

  // Expect A and AAAA questions on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kHostFullName, DnsType::kA);
  ExpectQuestion(message.get(), kHostFullName, DnsType::kAaaa);
  ExpectNoOtherQuestionOrResource(message.get());

  // Respond. Both subscribers unsubscribe from inside |AddressesChanged|.
  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 200, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), kInterfaceId, Media::kWireless, IpVersions::kV4);
  under_test->ReceiveResource(DnsResource(kHostFullName, kAddresses[0]),
                              MdnsResourceSection::kAnswer, sender_address);
  under_test->EndOfMessage();

  EXPECT_TRUE(subscriber_a.addresses_changed_called());
  EXPECT_TRUE(subscriber_b.addresses_changed_called());
  ExpectRenewCall(DnsResource(kHostFullName, kAddresses[0]));

  // Removing the last subscriber posts a task that quits the agent.
  ExpectPostTaskForTimeAndInvoke(zx::sec(0), zx::sec(0));
  ExpectRemoveAgentCall();
}

}  // namespace mdns::test
