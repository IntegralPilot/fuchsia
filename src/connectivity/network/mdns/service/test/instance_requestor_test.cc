// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/agents/instance_requestor.h"

#include <lib/zx/time.h>

#include <gtest/gtest.h>

#include "src/connectivity/network/mdns/service/common/mdns_names.h"
#include "src/connectivity/network/mdns/service/common/service_instance.h"
#include "src/connectivity/network/mdns/service/common/type_converters.h"
#include "src/connectivity/network/mdns/service/test/agent_test.h"
#include "src/connectivity/network/mdns/service/test/fake_clock.h"
#include "src/lib/inet/socket_address.h"

namespace mdns::test {

class InstanceRequestorTest : public AgentTest {
 public:
  InstanceRequestorTest() = default;

 protected:
  class Subscriber : public Mdns::Subscriber {
   public:
    struct InstanceId {
      InstanceId(const DnsName& service, const DnsLabel& instance)
          : service_(service), instance_(instance) {}

      DnsName service_;
      DnsLabel instance_;
    };

    void InstanceDiscovered(const DnsName& service, const DnsLabel& instance,
                            const std::vector<inet::SocketAddress>& addresses,
                            const std::vector<std::vector<uint8_t>>& text, uint16_t srv_priority,
                            uint16_t srv_weight, const DnsName& target) override {
      instance_discovered_params_ = std::make_unique<ServiceInstance>(
          service, instance, target, addresses, text, srv_priority, srv_weight);
    }

    void InstanceChanged(const DnsName& service, const DnsLabel& instance,
                         const std::vector<inet::SocketAddress>& addresses,
                         const std::vector<std::vector<uint8_t>>& text, uint16_t srv_priority,
                         uint16_t srv_weight, const DnsName& target) override {
      instance_changed_params_ = std::make_unique<ServiceInstance>(
          service, instance, target, addresses, text, srv_priority, srv_weight);
    }

    void InstanceLost(const DnsName& service, const DnsLabel& instance) override {
      instance_lost_params_ = std::make_unique<InstanceId>(service, instance);
    }

    void Query(DnsType type_queried) override { query_param_ = type_queried; }

    std::unique_ptr<ServiceInstance> ExpectInstanceDiscoveredCalled() {
      EXPECT_TRUE(!!instance_discovered_params_);
      return std::move(instance_discovered_params_);
    }

    std::unique_ptr<ServiceInstance> ExpectInstanceChangedCalled() {
      EXPECT_TRUE(!!instance_changed_params_);
      return std::move(instance_changed_params_);
    }

    std::unique_ptr<InstanceId> ExpectInstanceLostCalled() {
      EXPECT_TRUE(!!instance_lost_params_);
      return std::move(instance_lost_params_);
    }

    void ExpectQueryCalled(DnsType type) {
      EXPECT_NE(DnsType::kInvalid, query_param_);
      EXPECT_EQ(type, query_param_);
      query_param_ = DnsType::kInvalid;
    }

    void ExpectNoOther() {
      EXPECT_FALSE(!!instance_discovered_params_);
      EXPECT_FALSE(!!instance_changed_params_);
      EXPECT_FALSE(!!instance_lost_params_);
      EXPECT_EQ(DnsType::kInvalid, query_param_);
    }

   private:
    std::unique_ptr<ServiceInstance> instance_discovered_params_;
    std::unique_ptr<ServiceInstance> instance_changed_params_;
    std::unique_ptr<InstanceId> instance_lost_params_;
    DnsType query_param_ = DnsType::kInvalid;
  };

  void ReceivePublication(InstanceRequestor& under_test, const DnsName& host_full_name,
                          const DnsName& service_name, const DnsLabel& instance_name,
                          inet::IpPort port, const std::vector<std::vector<uint8_t>>& text,
                          ReplyAddress sender_address, bool include_txt = true,
                          bool include_address = true, bool address_cache_flush = false) {
    auto service_full_name = MdnsNames::ServiceFullName(service_name);
    auto instance_full_name = MdnsNames::InstanceFullName(instance_name, service_name);

    DnsResource ptr_resource(service_full_name, DnsType::kPtr);
    ptr_resource.ptr_.pointer_domain_name_ = DnsName(instance_full_name);
    under_test.ReceiveResource(ptr_resource, MdnsResourceSection::kAnswer, sender_address);

    DnsResource srv_resource(instance_full_name, DnsType::kSrv);
    srv_resource.srv_.port_ = port;
    srv_resource.srv_.target_ = DnsName(host_full_name);
    under_test.ReceiveResource(srv_resource, MdnsResourceSection::kAdditional, sender_address);

    if (include_txt) {
      DnsResource txt_resource(instance_full_name, DnsType::kTxt);
      txt_resource.txt_.strings_ = text;
      under_test.ReceiveResource(txt_resource, MdnsResourceSection::kAdditional, sender_address);
    }

    if (include_address) {
      DnsResource a_resource(host_full_name, sender_address.socket_address().address());
      if (address_cache_flush) {
        a_resource.cache_flush_ = true;
      }
      under_test.ReceiveResource(a_resource, MdnsResourceSection::kAdditional, sender_address);
    }

    under_test.EndOfMessage();
  }

  void ReceivePtr(InstanceRequestor& under_test, const DnsName& service_name,
                  const DnsLabel& instance_name, ReplyAddress sender_address) {
    auto service_full_name = MdnsNames::ServiceFullName(service_name);
    auto instance_full_name = MdnsNames::InstanceFullName(instance_name, service_name);

    DnsResource ptr_resource(service_full_name, DnsType::kPtr);
    ptr_resource.ptr_.pointer_domain_name_ = DnsName(instance_full_name);
    under_test.ReceiveResource(ptr_resource, MdnsResourceSection::kAnswer, sender_address);

    under_test.EndOfMessage();
  }

  void ReceiveSrv(InstanceRequestor& under_test, const DnsName& host_full_name,
                  const DnsName& service_name, const DnsLabel& instance_name, inet::IpPort port,
                  ReplyAddress sender_address) {
    auto instance_full_name = MdnsNames::InstanceFullName(instance_name, service_name);

    DnsResource srv_resource(instance_full_name, DnsType::kSrv);
    srv_resource.srv_.port_ = port;
    srv_resource.srv_.target_ = DnsName(host_full_name);
    under_test.ReceiveResource(srv_resource, MdnsResourceSection::kAnswer, sender_address);

    under_test.EndOfMessage();
  }

  void ReceiveTxt(InstanceRequestor& under_test, const DnsName& service_name,
                  const DnsLabel& instance_name, const std::vector<std::vector<uint8_t>>& text,
                  ReplyAddress sender_address) {
    auto instance_full_name = MdnsNames::InstanceFullName(instance_name, service_name);

    DnsResource txt_resource(instance_full_name, DnsType::kTxt);
    txt_resource.txt_.strings_ = text;
    under_test.ReceiveResource(txt_resource, MdnsResourceSection::kAnswer, sender_address);

    under_test.EndOfMessage();
  }

  void ReceiveAddress(InstanceRequestor& under_test, const DnsName& host_full_name,
                      ReplyAddress sender_address) {
    DnsResource a_resource(host_full_name, sender_address.socket_address().address());
    under_test.ReceiveResource(a_resource, MdnsResourceSection::kAnswer, sender_address);

    under_test.EndOfMessage();
  }
};

class InstanceRequestorTestWithParam : public InstanceRequestorTest,
                                       public testing::WithParamInterface<bool> {};

const zx::duration kMinDelay = zx::sec(1);
const zx::duration kMaxDelay = zx::hour(1);
const DnsName kHostFullName("test2host.local.");
const DnsName kHostName("test2host");
const DnsName kServiceName("_testservice._tcp.");
const DnsName kServiceFullName("_testservice._tcp.local.");
const DnsName kAnyServiceFullName("_services._dns-sd._udp.local.");
const DnsLabel kInstanceName("testinstance");
const DnsName kInstanceFullName("testinstance._testservice._tcp.local.");
const inet::IpPort kPort = inet::IpPort::From_uint16_t(1234);
const std::vector<std::vector<uint8_t>> kText = fidl::To<std::vector<std::vector<uint8_t>>>(
    std::vector<std::string>{"color=red", "shape=round"});
const std::vector<std::vector<uint8_t>> kAltText = fidl::To<std::vector<std::vector<uint8_t>>>(
    std::vector<std::string>{"color=green", "shape=square"});
constexpr bool kIncludeLocal = true;
constexpr bool kExcludeLocal = false;
constexpr bool kIncludeLocalProxies = true;
constexpr bool kExcludeLocalProxies = false;
constexpr bool kFromLocalProxyHost = true;
constexpr bool kFromLocalHost = false;
const std::vector<HostAddress> kHostAddresses{
    HostAddress(inet::IpAddress(192, 168, 1, 200), 1, zx::sec(450)),
    HostAddress(inet::IpAddress(0xfe80, 200), 1, zx::sec(450))};
const std::vector<inet::SocketAddress> kSocketAddresses{
    inet::SocketAddress(inet::IpAddress(192, 168, 1, 200), kPort, 1),
    inet::SocketAddress(inet::IpAddress(0xfe80, 200), kPort, 1)};
const std::vector<inet::SocketAddress> kSocketAddressesReversed{
    inet::SocketAddress(inet::IpAddress(0xfe80, 200), kPort, 1),
    inet::SocketAddress(inet::IpAddress(192, 168, 1, 200), kPort, 1)};
constexpr zx::duration kAdditionalInterval = zx::sec(1);
constexpr uint32_t kAdditionalIntervalMultiplier = 2;
constexpr uint32_t kAdditionalMaxQueries = 3;

// Tests nominal startup behavior of the requestor.
TEST_F(InstanceRequestorTest, QuerySequence) {
  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());

  // Queries should be send after 1s, 2s, 4s, etc, capping out an an hour.
  auto delay = kMinDelay;

  while (delay < kMaxDelay) {
    ExpectPostTaskForTimeAndInvoke(delay, delay);
    auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
    ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
    ExpectNoOtherQuestionOrResource(message.get());

    delay = delay * 2;
  }

  ExpectPostTaskForTime(kMaxDelay, kMaxDelay);
  ExpectNoOther();
}

// Tests the behavior of the requestor when a response is received.
TEST_F(InstanceRequestorTest, Response) {
  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address);

  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(ServiceInstance(kServiceName, kInstanceName, kHostName,
                            {inet::SocketAddress(sender_address.socket_address().address(), kPort)},
                            kText, 0, 0),
            *params);

  subscriber.ExpectNoOther();
}

// Tests the behavior of the requestor when responses indicate a change to an instance.
TEST_F(InstanceRequestorTest, Change) {
  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  // Respond.
  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address);

  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(ServiceInstance(kServiceName, kInstanceName, kHostName,
                            {inet::SocketAddress(sender_address.socket_address().address(), kPort)},
                            kText, 0, 0),
            *params);

  subscriber.ExpectNoOther();

  // Respond with different text.
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kAltText,
                     sender_address);

  params = subscriber.ExpectInstanceChangedCalled();
  EXPECT_EQ(ServiceInstance(kServiceName, kInstanceName, kHostName,
                            {inet::SocketAddress(sender_address.socket_address().address(), kPort)},
                            kAltText, 0, 0),
            *params);

  subscriber.ExpectNoOther();
}

// Tests the behavior of the requestor an address record indicate addresses should be flushed.
TEST_F(InstanceRequestorTest, AddressCacheFlush) {
  FakeClock kayfabe;

  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  // Respond with an IPv4 address.
  ReplyAddress sender_address_0(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address_0);

  // Expect |sender_address_0|.
  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(
      ServiceInstance(kServiceName, kInstanceName, kHostName,
                      {inet::SocketAddress(sender_address_0.socket_address().address(), kPort)},
                      kText, 0, 0),
      *params);

  subscriber.ExpectNoOther();

  // Respond with an IPv6 address.
  ReplyAddress sender_address_v6(inet::SocketAddress(0xfe80, 1, inet::IpPort::From_uint16_t(5353)),
                                 inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless,
                                 IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address_v6);

  // Expect |sender_address_0| and |sender_address_v6|.
  params = subscriber.ExpectInstanceChangedCalled();
  EXPECT_EQ(
      ServiceInstance(kServiceName, kInstanceName, kHostName,
                      {inet::SocketAddress(sender_address_v6.socket_address().address(), kPort, 1),
                       inet::SocketAddress(sender_address_0.socket_address().address(), kPort)},
                      kText, 0, 0),
      *params);

  // Make sure the above address is more than one second old.
  FakeClock::Advance(zx::sec(2));

  // Respond with a second IPv4 address.
  ReplyAddress sender_address_2(
      inet::SocketAddress(192, 168, 1, 2, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address_2);

  // Expect |sender_address_0|, |sender_address_v6| and |sender_address_2|.
  params = subscriber.ExpectInstanceChangedCalled();
  EXPECT_EQ(
      ServiceInstance(kServiceName, kInstanceName, kHostName,
                      {inet::SocketAddress(sender_address_2.socket_address().address(), kPort),
                       inet::SocketAddress(sender_address_v6.socket_address().address(), kPort, 1),
                       inet::SocketAddress(sender_address_0.socket_address().address(), kPort)},
                      kText, 0, 0),
      *params);

  // Make sure the second IPv4 address is less than one second old.
  FakeClock::Advance(zx::msec(500));

  // Respond with a third IPv4 address with the cache flush bit set.
  ReplyAddress sender_address_3(
      inet::SocketAddress(192, 168, 1, 3, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address_3, true,  // include_txt
                     true,                    // include_address
                     true                     // address_cache_flush
  );

  // Expect that |sender_address_| has been flushed, |sender_address_v6| and |sender_address_2| have
  // not (a V6 address and an address sent less than a second ago), and that the new
  // |sender_address_3| appears.
  params = subscriber.ExpectInstanceChangedCalled();
  EXPECT_EQ(
      ServiceInstance(kServiceName, kInstanceName, kHostName,
                      {inet::SocketAddress(sender_address_2.socket_address().address(), kPort),
                       inet::SocketAddress(sender_address_3.socket_address().address(), kPort),
                       inet::SocketAddress(sender_address_v6.socket_address().address(), kPort, 1)},
                      kText, 0, 0),
      *params);

  subscriber.ExpectNoOther();
}

// Tests the behavior of the requestor when an expiration indicates the removal of an instance.
TEST_F(InstanceRequestorTest, Removal) {
  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  // Respond.
  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address);

  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(ServiceInstance(kServiceName, kInstanceName, kHostName,
                            {inet::SocketAddress(sender_address.socket_address().address(), kPort)},
                            kText, 0, 0),
            *params);

  subscriber.ExpectNoOther();

  // Expire the PTR resource.
  DnsResource ptr_resource(kServiceFullName, DnsType::kPtr);
  ptr_resource.ptr_.pointer_domain_name_ = DnsName(kInstanceFullName);
  ptr_resource.time_to_live_ = 0;
  under_test.ReceiveResource(ptr_resource, MdnsResourceSection::kExpired, sender_address);

  auto instance_id = subscriber.ExpectInstanceLostCalled();
  EXPECT_EQ(kInstanceName, instance_id->instance_);
  EXPECT_EQ(kServiceName, instance_id->service_);
  subscriber.ExpectNoOther();
}

// Tests that the requstor removed itself when the last subscriber is removed.
TEST_F(InstanceRequestorTest, RemoveSelf) {
  // Need to |make_shared|, because |RemoveSelf| calls |shared_from_this|.
  auto under_test = std::make_shared<InstanceRequestor>(
      this, kServiceName, Media::kBoth, IpVersions::kBoth, kExcludeLocal, kExcludeLocalProxies);
  SetAgent(*under_test);

  Subscriber subscriber;
  under_test->AddSubscriber(&subscriber);
  under_test->RemoveSubscriber(&subscriber);

  ExpectPostTaskForTimeAndInvoke(zx::sec(0), zx::sec(0));
  ExpectRemoveAgentCall();
}

// Tests the behavior of the requestor when configured for wireless-only operation.
TEST_F(InstanceRequestorTest, WirelessOnly) {
  InstanceRequestor under_test(this, kServiceName, Media::kWireless, IpVersions::kBoth,
                               kExcludeLocal, kExcludeLocalProxies);
  SetAgent(under_test);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message =
      ExpectOutboundMessage(ReplyAddress::Multicast(Media::kWireless, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  ReplyAddress sender_address0(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWired, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address0);
  subscriber.ExpectNoOther();

  ReplyAddress sender_address1(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address1);

  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(
      ServiceInstance(kServiceName, kInstanceName, kHostName,
                      {inet::SocketAddress(sender_address1.socket_address().address(), kPort)},
                      kText, 0, 0),
      *params);

  subscriber.ExpectNoOther();
}

// Tests the behavior of the requestor when configured for wired-only operation.
TEST_F(InstanceRequestorTest, WiredOnly) {
  InstanceRequestor under_test(this, kServiceName, Media::kWired, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kWired, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  ReplyAddress sender_address0(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address0);
  subscriber.ExpectNoOther();

  ReplyAddress sender_address1(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWired, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address1);

  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(
      ServiceInstance(kServiceName, kInstanceName, kHostName,
                      {inet::SocketAddress(sender_address1.socket_address().address(), kPort)},
                      kText, 0, 0),
      *params);

  subscriber.ExpectNoOther();
}

// Tests the behavior of the requestor when configured for IPv4-only operation.
TEST_F(InstanceRequestorTest, V4Only) {
  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kV4, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kV4));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  ReplyAddress sender_address0(inet::SocketAddress(0xfe80, 1, inet::IpPort::From_uint16_t(5353)),
                               inet::IpAddress(0xfe80, 100), 1, Media::kWired, IpVersions::kV6);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address0);
  subscriber.ExpectNoOther();

  ReplyAddress sender_address1(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address1);

  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(
      ServiceInstance(kServiceName, kInstanceName, kHostName,
                      {inet::SocketAddress(sender_address1.socket_address().address(), kPort)},
                      kText, 0, 0),
      *params);

  subscriber.ExpectNoOther();
}

// Tests the behavior of the requestor when configured for IPv6-only operation.
TEST_F(InstanceRequestorTest, V6Only) {
  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kV6, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kV6));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  ReplyAddress sender_address0(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWired, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address0);
  subscriber.ExpectNoOther();

  ReplyAddress sender_address1(inet::SocketAddress(0xfe80, 1, inet::IpPort::From_uint16_t(5353)),
                               inet::IpAddress(0xfe80, 100), 1, Media::kWireless, IpVersions::kV6);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address1);

  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(
      ServiceInstance(kServiceName, kInstanceName, kHostName,
                      {inet::SocketAddress(sender_address1.socket_address().address(), kPort, 1)},
                      kText, 0, 0),
      *params);

  subscriber.ExpectNoOther();
}

// Tests the behavior of the requestor when configured to discover services on the local host.
TEST_F(InstanceRequestorTest, LocalInstance) {
  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kBoth, kIncludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);
  SetLocalHostAddresses(kHostAddresses);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  // Expect to see an added local service instance.
  under_test.OnAddLocalServiceInstance(
      ServiceInstance(kServiceName, kInstanceName, kLocalHostName, kSocketAddresses, kText),
      kFromLocalHost);

  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(
      ServiceInstance(kServiceName, kInstanceName, kLocalHostName, kSocketAddressesReversed, kText),
      *params);

  // |OnChangeLocalServiceInstance| should do nothing if the instance doesn't change.
  under_test.OnChangeLocalServiceInstance(
      ServiceInstance(kServiceName, kInstanceName, kLocalHostName, kSocketAddresses, kText),
      kFromLocalHost);
  subscriber.ExpectNoOther();

  // If the instance does change, we should see the notification.
  under_test.OnChangeLocalServiceInstance(
      ServiceInstance(kServiceName, kInstanceName, kLocalHostName, kSocketAddresses, kAltText),
      false);
  params = subscriber.ExpectInstanceChangedCalled();
  EXPECT_EQ(ServiceInstance(kServiceName, kInstanceName, kLocalHostName, kSocketAddressesReversed,
                            kAltText),
            *params);
  subscriber.ExpectNoOther();

  // |OnRemoveLocalServiceInstance| should notify of a removal.
  under_test.OnRemoveLocalServiceInstance(kServiceName, kInstanceName, kFromLocalHost);
  auto lost_params = subscriber.ExpectInstanceLostCalled();
  EXPECT_EQ(kServiceName, lost_params->service_);
  EXPECT_EQ(kInstanceName, lost_params->instance_);

  // Expect that local proxy host instances are ignored.
  under_test.OnAddLocalServiceInstance(
      ServiceInstance(kServiceName, kInstanceName, kHostName, kSocketAddresses, kText),
      kFromLocalProxyHost);
  subscriber.ExpectNoOther();
}

// Tests the behavior of the requestor when configured to discover services on a local proxy host.
TEST_F(InstanceRequestorTest, LocalProxyInstance) {
  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kIncludeLocalProxies);
  SetAgent(under_test);
  SetLocalHostAddresses(kHostAddresses);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  // Expect to see an added local service instance.
  under_test.OnAddLocalServiceInstance(
      ServiceInstance(kServiceName, kInstanceName, kHostName, kSocketAddresses, kText),
      kFromLocalProxyHost);

  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(
      ServiceInstance(kServiceName, kInstanceName, kHostName, kSocketAddressesReversed, kText),
      *params);

  // |OnChangeLocalServiceInstance| should do nothing if the instance doesn't change.
  under_test.OnChangeLocalServiceInstance(
      ServiceInstance(kServiceName, kInstanceName, kHostName, kSocketAddresses, kText),
      kFromLocalProxyHost);
  subscriber.ExpectNoOther();

  // If the instance does change, we should see the notification.
  under_test.OnChangeLocalServiceInstance(
      ServiceInstance(kServiceName, kInstanceName, kHostName, kSocketAddresses, kAltText),
      kFromLocalProxyHost);
  params = subscriber.ExpectInstanceChangedCalled();
  EXPECT_EQ(
      ServiceInstance(kServiceName, kInstanceName, kHostName, kSocketAddressesReversed, kAltText),
      *params);
  subscriber.ExpectNoOther();

  // |OnRemoveLocalServiceInstance| should notify of a removal.
  under_test.OnRemoveLocalServiceInstance(kServiceName, kInstanceName, kFromLocalProxyHost);
  auto lost_params = subscriber.ExpectInstanceLostCalled();
  EXPECT_EQ(kServiceName, lost_params->service_);
  EXPECT_EQ(kInstanceName, lost_params->instance_);

  // Expect that local host instances are ignored.
  under_test.OnAddLocalServiceInstance(
      ServiceInstance(kServiceName, kInstanceName, kLocalHostName, kSocketAddresses, kText),
      kFromLocalHost);
  subscriber.ExpectNoOther();
}

TEST_P(InstanceRequestorTestWithParam, InstanceIgnoreNetwork) {
  bool include_local = GetParam();
  bool include_local_proxies = !include_local;
  DnsName host_name = include_local ? kLocalHostName : kHostName;

  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kBoth, include_local,
                               include_local_proxies);
  SetAgent(under_test);
  SetLocalHostAddresses(kHostAddresses);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  // Expect to see an added local service instance.
  under_test.OnAddLocalServiceInstance(
      ServiceInstance(kServiceName, kInstanceName, host_name, kSocketAddresses, kText),
      include_local_proxies);

  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(
      ServiceInstance(kServiceName, kInstanceName, host_name, kSocketAddressesReversed, kText),
      *params);
  subscriber.ExpectNoOther();

  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);

  // Expect that publications received over the network referring to the local
  // (proxy) instances are ignored.
  ReceivePublication(under_test, host_name, kServiceName, kInstanceName, kPort, kAltText,
                     sender_address);
  subscriber.ExpectNoOther();

  // Expect that individual records received over the network referring to the
  // local (proxy) instance or host are ignored.
  ReceiveSrv(under_test, host_name, kServiceName, kInstanceName, inet::IpPort::From_uint16_t(4321),
             sender_address);
  subscriber.ExpectNoOther();

  ReceiveTxt(under_test, kServiceName, kInstanceName, kAltText, sender_address);
  subscriber.ExpectNoOther();

  ReceiveAddress(under_test, host_name, sender_address);
  subscriber.ExpectNoOther();

  // Expire the PTR resource over the network and expect that it is ignored.
  DnsResource ptr_resource(kServiceFullName, DnsType::kPtr);
  ptr_resource.ptr_.pointer_domain_name_ = DnsName(kInstanceFullName);
  ptr_resource.time_to_live_ = 0;
  under_test.ReceiveResource(ptr_resource, MdnsResourceSection::kExpired, sender_address);
  subscriber.ExpectNoOther();

  // Expire the SRV resource over the network and expect that it is ignored.
  DnsResource srv_resource(kInstanceFullName, DnsType::kSrv);
  srv_resource.time_to_live_ = 0;
  under_test.ReceiveResource(srv_resource, MdnsResourceSection::kExpired, sender_address);
  subscriber.ExpectNoOther();

  // Expect that legitimate local changes to the local (proxy) instance are
  // still handled.
  under_test.OnChangeLocalServiceInstance(
      ServiceInstance(kServiceName, kInstanceName, host_name, kSocketAddresses, kAltText),
      include_local_proxies);
  params = subscriber.ExpectInstanceChangedCalled();
  EXPECT_EQ(
      ServiceInstance(kServiceName, kInstanceName, host_name, kSocketAddressesReversed, kAltText),
      *params);
  subscriber.ExpectNoOther();

  // Expect that local removals are still handled.
  under_test.OnRemoveLocalServiceInstance(kServiceName, kInstanceName, include_local_proxies);
  auto lost_params = subscriber.ExpectInstanceLostCalled();
  EXPECT_EQ(kServiceName, lost_params->service_);
  EXPECT_EQ(kInstanceName, lost_params->instance_);
  subscriber.ExpectNoOther();
}

INSTANTIATE_TEST_SUITE_P(LocalInstanceIgnoreNetwork, InstanceRequestorTestWithParam,
                         testing::Values(true, false));

// Tests the behavior of a requestor for any service when a response is received.
TEST_F(InstanceRequestorTest, AnyResponse) {
  InstanceRequestor under_test(this, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kAnyServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address);

  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(ServiceInstance(kServiceName, kInstanceName, kHostName,
                            {inet::SocketAddress(sender_address.socket_address().address(), kPort)},
                            kText, 0, 0),
            *params);

  subscriber.ExpectNoOther();
}

// Tests the behavior of the requestor when a response containing no addresses is received.
TEST_F(InstanceRequestorTest, ResponseSansAddresses) {
  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  // Receive a response with no address records.
  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address, true, false);  // yes TXT record, no A record.

  subscriber.ExpectNoOther();

  // Expect a A/AAAA queries.
  ExpectQueryCall(DnsType::kA, kHostFullName, Media::kBoth, IpVersions::kBoth, now(),
                  kAdditionalInterval, kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectQueryCall(DnsType::kAaaa, kHostFullName, Media::kBoth, IpVersions::kBoth, now(),
                  kAdditionalInterval, kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);

  // Receive a response with only an address record.
  ReceiveAddress(under_test, kHostFullName, sender_address);

  // Expect discovery of the instance to be reported to the client.
  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(ServiceInstance(kServiceName, kInstanceName, kHostName,
                            {inet::SocketAddress(sender_address.socket_address().address(), kPort)},
                            kText, 0, 0),
            *params);

  subscriber.ExpectNoOther();
}

// Tests the behavior of the requestor when a response containing no TXT resource is received.
TEST_F(InstanceRequestorTest, ResponseSansTxt) {
  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  // Receive a response with no TXT record.
  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address, false, true);  // no TXT record, yes A record.

  // Expect the instance to be reported to the client with no text.
  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(ServiceInstance(kServiceName, kInstanceName, kHostName,
                            {inet::SocketAddress(sender_address.socket_address().address(), kPort)},
                            {}, 0, 0),
            *params);

  // Expect a TXT query call.
  ExpectQueryCall(DnsType::kTxt, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);

  // Receive a response with only a TXT record.
  ReceiveTxt(under_test, kServiceName, kInstanceName, kText, sender_address);

  // Expect change of the instance to be reported to the client with text.
  params = subscriber.ExpectInstanceChangedCalled();
  EXPECT_EQ(ServiceInstance(kServiceName, kInstanceName, kHostName,
                            {inet::SocketAddress(sender_address.socket_address().address(), kPort)},
                            kText, 0, 0),
            *params);

  subscriber.ExpectNoOther();
}

// Tests the behavior of the requestor when a response containing only PTR is received.
TEST_F(InstanceRequestorTest, ResponsePtrOnly) {
  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  // Receive a response only a PTR record.
  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePtr(under_test, kServiceName, kInstanceName, sender_address);

  subscriber.ExpectNoOther();

  // Expect an SRV query calls.
  ExpectQueryCall(DnsType::kSrv, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);

  // Receive a response with only an SRV record.
  ReceiveSrv(under_test, kHostFullName, kServiceName, kInstanceName, kPort, sender_address);

  subscriber.ExpectNoOther();

  // Expect a A, AAAA and TXT query calls.
  ExpectQueryCall(DnsType::kA, kHostFullName, Media::kBoth, IpVersions::kBoth, now(),
                  kAdditionalInterval, kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectQueryCall(DnsType::kAaaa, kHostFullName, Media::kBoth, IpVersions::kBoth, now(),
                  kAdditionalInterval, kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectQueryCall(DnsType::kTxt, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);

  // Receive a response with only an address record.
  ReceiveAddress(under_test, kHostFullName, sender_address);

  // Expect the instance to be reported to the client with no text.
  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(ServiceInstance(kServiceName, kInstanceName, kHostName,
                            {inet::SocketAddress(sender_address.socket_address().address(), kPort)},
                            {}, 0, 0),
            *params);

  subscriber.ExpectNoOther();

  // Receive a response with only a TXT record.
  ReceiveTxt(under_test, kServiceName, kInstanceName, kText, sender_address);

  // Expect change of the instance to be reported to the client with text.
  params = subscriber.ExpectInstanceChangedCalled();
  EXPECT_EQ(ServiceInstance(kServiceName, kInstanceName, kHostName,
                            {inet::SocketAddress(sender_address.socket_address().address(), kPort)},
                            kText, 0, 0),
            *params);

  subscriber.ExpectNoOther();
}

// Regression test: subscribers that unsubscribe synchronously from their |InstanceLost| callback
// (as the FIDL subscriber implementations do when a listener proxy call fails at encode time)
// must not invalidate the subscriber set iteration in |RemoveInstance|.
TEST_F(InstanceRequestorTest, SubscriberUnsubscribesDuringInstanceLost) {
  // Need to |make_shared|, because removing the last subscriber posts a task that calls
  // |shared_from_this|.
  auto under_test = std::make_shared<InstanceRequestor>(
      this, kServiceName, Media::kBoth, IpVersions::kBoth, kExcludeLocal, kExcludeLocalProxies);
  SetAgent(*under_test);

  class UnsubscribingSubscriber : public Subscriber {
   public:
    explicit UnsubscribingSubscriber(InstanceRequestor& requestor) : requestor_(requestor) {}

    void InstanceLost(const DnsName& service, const DnsLabel& instance) override {
      Subscriber::InstanceLost(service, instance);
      requestor_.RemoveSubscriber(this);
    }

   private:
    InstanceRequestor& requestor_;
  };

  UnsubscribingSubscriber subscriber_a(*under_test);
  UnsubscribingSubscriber subscriber_b(*under_test);
  under_test->AddSubscriber(&subscriber_a);
  under_test->AddSubscriber(&subscriber_b);

  under_test->Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);

  subscriber_a.ExpectQueryCalled(DnsType::kPtr);
  subscriber_b.ExpectQueryCalled(DnsType::kPtr);

  // Receive a response with only a PTR record, so the instance is known but not yet reported.
  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePtr(*under_test, kServiceName, kInstanceName, sender_address);

  // Expect an SRV query.
  ExpectQueryCall(DnsType::kSrv, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);

  // The requestor also asked for the received PTR resource to be renewed.
  ExpectRenewCall(DnsResource(kServiceFullName, DnsType::kPtr));

  // Expire the PTR resource. Both subscribers unsubscribe from inside |InstanceLost|.
  DnsResource ptr_resource(kServiceFullName, DnsType::kPtr);
  ptr_resource.ptr_.pointer_domain_name_ = DnsName(kInstanceFullName);
  ptr_resource.time_to_live_ = 0;
  under_test->ReceiveResource(ptr_resource, MdnsResourceSection::kExpired, sender_address);

  auto instance_id_a = subscriber_a.ExpectInstanceLostCalled();
  EXPECT_EQ(kServiceName, instance_id_a->service_);
  EXPECT_EQ(kInstanceName, instance_id_a->instance_);
  auto instance_id_b = subscriber_b.ExpectInstanceLostCalled();
  EXPECT_EQ(kServiceName, instance_id_b->service_);
  EXPECT_EQ(kInstanceName, instance_id_b->instance_);

  // Removing the last subscriber posts a task that quits the agent.
  ExpectPostTaskForTimeAndInvoke(zx::sec(0), zx::sec(0));
  ExpectRemoveAgentCall();
}

// Regression test: a subscriber that unsubscribes synchronously from its |InstanceDiscovered|
// callback must not receive further callbacks from |ReportAllDiscoveries|.
TEST_F(InstanceRequestorTest, SubscriberUnsubscribesDuringReportAllDiscoveries) {
  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  Subscriber subscriber_a;
  under_test.AddSubscriber(&subscriber_a);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);

  subscriber_a.ExpectQueryCalled(DnsType::kPtr);

  // Discover two instances.
  const DnsLabel kInstanceName2("testinstance2");
  ReplyAddress sender_address(
      inet::SocketAddress(192, 168, 1, 1, inet::IpPort::From_uint16_t(5353)),
      inet::IpAddress(192, 168, 1, 100), 1, Media::kWireless, IpVersions::kV4);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address);
  subscriber_a.ExpectInstanceDiscoveredCalled();
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName2, kPort, kText,
                     sender_address);
  subscriber_a.ExpectInstanceDiscoveredCalled();

  // A subscriber that unsubscribes on its first callback must get exactly one callback.
  class UnsubscribingSubscriber : public Subscriber {
   public:
    explicit UnsubscribingSubscriber(InstanceRequestor& requestor) : requestor_(requestor) {}

    void InstanceDiscovered(const DnsName& service, const DnsLabel& instance,
                            const std::vector<inet::SocketAddress>& addresses,
                            const std::vector<std::vector<uint8_t>>& text, uint16_t srv_priority,
                            uint16_t srv_weight, const DnsName& target) override {
      ++discovered_count_;
      requestor_.RemoveSubscriber(this);
    }

    size_t discovered_count() const { return discovered_count_; }

   private:
    InstanceRequestor& requestor_;
    size_t discovered_count_ = 0;
  };

  UnsubscribingSubscriber subscriber_b(under_test);
  under_test.AddSubscriber(&subscriber_b);

  EXPECT_EQ(1u, subscriber_b.discovered_count());
}

// Tests the behavior of the requestor when a response containing no addresses is received on V6.
TEST_F(InstanceRequestorTest, ResponseSansAddressesV6) {
  InstanceRequestor under_test(this, kServiceName, Media::kBoth, IpVersions::kBoth, kExcludeLocal,
                               kExcludeLocalProxies);
  SetAgent(under_test);

  Subscriber subscriber;
  under_test.AddSubscriber(&subscriber);

  under_test.Start(kLocalHostFullName);

  // Expect a PTR question on start.
  auto message = ExpectOutboundMessage(ReplyAddress::Multicast(Media::kBoth, IpVersions::kBoth));
  ExpectQuestion(message.get(), kServiceFullName, DnsType::kPtr);
  ExpectNoOtherQuestionOrResource(message.get());
  ExpectPostTaskForTime(kMinDelay, kMinDelay);
  ExpectNoOther();

  subscriber.ExpectQueryCalled(DnsType::kPtr);
  subscriber.ExpectNoOther();

  // Receive a response with no address records.
  ReplyAddress sender_address(inet::SocketAddress(0xfe80, 1, inet::IpPort::From_uint16_t(5353)),
                              inet::IpAddress(0xfe80, 100), 1, Media::kWireless, IpVersions::kV6);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address, true, false);  // yes TXT record, no A record.

  subscriber.ExpectNoOther();

  // Expect a A/AAAA queries.
  ExpectQueryCall(DnsType::kA, kHostFullName, Media::kBoth, IpVersions::kBoth, now(),
                  kAdditionalInterval, kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectQueryCall(DnsType::kAaaa, kHostFullName, Media::kBoth, IpVersions::kBoth, now(),
                  kAdditionalInterval, kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);

  // Receive a response with only an address record.
  ReceiveAddress(under_test, kHostFullName, sender_address);

  // Expect discovery of the instance to be reported to the client.
  auto params = subscriber.ExpectInstanceDiscoveredCalled();
  if (!params) {
    return;
  }
  EXPECT_EQ(
      ServiceInstance(kServiceName, kInstanceName, kHostName,
                      {inet::SocketAddress(sender_address.socket_address().address(), kPort, 1)},
                      kText, 0, 0),
      *params);

  subscriber.ExpectNoOther();
}

}  // namespace mdns::test
