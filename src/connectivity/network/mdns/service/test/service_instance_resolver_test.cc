// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/agents/service_instance_resolver.h"

#include <lib/zx/time.h>

#include <gtest/gtest.h>

#include "src/connectivity/network/mdns/service/common/mdns_names.h"
#include "src/connectivity/network/mdns/service/common/service_instance.h"
#include "src/connectivity/network/mdns/service/common/type_converters.h"
#include "src/connectivity/network/mdns/service/test/agent_test.h"

namespace fuchsia::net {

bool operator==(const fuchsia::net::SocketAddress& lhs, const fuchsia::net::SocketAddress& rhs) {
  if (lhs.Which() != rhs.Which()) {
    return false;
  }

  if (lhs.is_ipv4()) {
    return lhs.ipv4().address.addr == rhs.ipv4().address.addr && lhs.ipv4().port == rhs.ipv4().port;
  } else {
    return lhs.ipv6().address.addr == rhs.ipv6().address.addr &&
           lhs.ipv6().port == rhs.ipv6().port && lhs.ipv6().zone_index == rhs.ipv6().zone_index;
  }
}

}  // namespace fuchsia::net

namespace mdns {
namespace test {

class InstanceResolverTest : public AgentTest {
 public:
  InstanceResolverTest() = default;

 protected:
  void ReceivePublication(ServiceInstanceResolver& under_test, const DnsName& host_full_name,
                          const DnsName& service_name, const DnsLabel& instance_name,
                          inet::IpPort port, const std::vector<std::vector<uint8_t>>& text,
                          ReplyAddress sender_address, bool include_address) {
    auto service_full_name = MdnsNames::ServiceFullName(service_name);
    auto instance_full_name = MdnsNames::InstanceFullName(instance_name, service_name);

    DnsResource ptr_resource(service_full_name, DnsType::kPtr);
    ptr_resource.ptr_.pointer_domain_name_ = instance_full_name;
    under_test.ReceiveResource(ptr_resource, MdnsResourceSection::kAnswer, sender_address);

    DnsResource srv_resource(instance_full_name, DnsType::kSrv);
    srv_resource.srv_.port_ = port;
    srv_resource.srv_.target_ = host_full_name;
    under_test.ReceiveResource(srv_resource, MdnsResourceSection::kAnswer, sender_address);

    DnsResource txt_resource(instance_full_name, DnsType::kTxt);
    txt_resource.txt_.strings_ = text;
    under_test.ReceiveResource(txt_resource, MdnsResourceSection::kAnswer, sender_address);

    if (include_address) {
      DnsResource a_resource(host_full_name, sender_address.socket_address().address());
      under_test.ReceiveResource(a_resource, MdnsResourceSection::kAnswer, sender_address);
    }

    under_test.EndOfMessage();
  }

  void ReceiveAddress(ServiceInstanceResolver& under_test, const DnsName& host_full_name,
                      ReplyAddress sender_address) {
    DnsResource a_resource(host_full_name, sender_address.socket_address().address());
    under_test.ReceiveResource(a_resource, MdnsResourceSection::kAnswer, sender_address);

    under_test.EndOfMessage();
  }

  void ReceiveSrvAndAddress(ServiceInstanceResolver& under_test, const DnsName& host_full_name,
                            const DnsName& service_name, const DnsLabel& instance_name,
                            inet::IpPort port, ReplyAddress sender_address) {
    auto instance_full_name = MdnsNames::InstanceFullName(instance_name, service_name);

    DnsResource srv_resource(instance_full_name, DnsType::kSrv);
    srv_resource.srv_.port_ = port;
    srv_resource.srv_.target_ = host_full_name;
    under_test.ReceiveResource(srv_resource, MdnsResourceSection::kAnswer, sender_address);

    DnsResource a_resource(host_full_name, sender_address.socket_address().address());
    under_test.ReceiveResource(a_resource, MdnsResourceSection::kAnswer, sender_address);

    under_test.EndOfMessage();
  }

  void ReceiveTxt(ServiceInstanceResolver& under_test, const DnsName& service_name,
                  const DnsLabel& instance_name, const std::vector<std::vector<uint8_t>>& text,
                  ReplyAddress sender_address) {
    auto instance_full_name = MdnsNames::InstanceFullName(instance_name, service_name);
    DnsResource txt_resource(instance_full_name, DnsType::kTxt);
    txt_resource.txt_.strings_ = text;
    under_test.ReceiveResource(txt_resource, MdnsResourceSection::kAnswer, sender_address);

    under_test.EndOfMessage();
  }
};

const DnsName kHostName("test2host");
const DnsName kHostFullName("test2host.local.");
const DnsName kServiceName("_testservice._tcp.");
const DnsLabel kInstanceName("testinstance");
const inet::IpPort kPort = inet::IpPort::From_uint16_t(1234);
const std::vector<std::vector<uint8_t>> kText = fidl::To<std::vector<std::vector<uint8_t>>>(
    std::vector<std::string>{"color=red", "shape=round"});
const std::vector<std::vector<uint8_t>> kEmptyText =
    fidl::To<std::vector<std::vector<uint8_t>>>(std::vector<std::string>{});
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
constexpr zx::duration kAdditionalInterval = zx::sec(1);
constexpr uint32_t kAdditionalIntervalMultiplier = 2;
constexpr uint32_t kAdditionalMaxQueries = 3;

// Tests the behavior of the resolver when configured to discover services on the local host.
TEST_F(InstanceResolverTest, LocalInstance) {
  fuchsia::net::mdns::ServiceInstance instance_from_callback;
  bool callback_called = false;

  ServiceInstanceResolver under_test(
      this, kServiceName, kInstanceName, now(), Media::kBoth, IpVersions::kBoth, kIncludeLocal,
      kExcludeLocalProxies,
      [&instance_from_callback, &callback_called](fuchsia::net::mdns::ServiceInstance instance) {
        instance_from_callback = std::move(instance);
        callback_called = true;
      });

  SetAgent(under_test);
  SetLocalHostAddresses(kHostAddresses);

  under_test.Start(kLocalHostFullName);

  // Expect a SRV and TXT questions on start.
  ExpectQueryCall(DnsType::kSrv, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectQueryCall(DnsType::kTxt, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectPostTaskForTime(zx::sec(0), zx::sec(0));
  ExpectNoOther();

  under_test.OnAddLocalServiceInstance(
      ServiceInstance(kServiceName, kInstanceName, kLocalHostName, kSocketAddresses, kText),
      kFromLocalHost);

  EXPECT_TRUE(callback_called);
  EXPECT_EQ(kServiceName.to_string(), instance_from_callback.service());
  EXPECT_EQ(kInstanceName, instance_from_callback.instance());
  EXPECT_EQ(kLocalHostName.to_string(), instance_from_callback.target());
  EXPECT_EQ(0, instance_from_callback.srv_priority());
  EXPECT_EQ(0, instance_from_callback.srv_weight());
  EXPECT_EQ(fidl::To<std::vector<fuchsia::net::SocketAddress>>(kSocketAddresses),
            instance_from_callback.addresses());
  EXPECT_EQ(kText, instance_from_callback.text_strings());
}

// Tests the behavior of the resolver when configured to discover services on a local proxy host.
TEST_F(InstanceResolverTest, LocalProxyInstance) {
  fuchsia::net::mdns::ServiceInstance instance_from_callback;
  bool callback_called = false;

  ServiceInstanceResolver under_test(
      this, kServiceName, kInstanceName, now(), Media::kBoth, IpVersions::kBoth, kExcludeLocal,
      kIncludeLocalProxies,
      [&instance_from_callback, &callback_called](fuchsia::net::mdns::ServiceInstance instance) {
        instance_from_callback = std::move(instance);
        callback_called = true;
      });

  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  // Expect a SRV and TXT questions on start.
  ExpectQueryCall(DnsType::kSrv, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectQueryCall(DnsType::kTxt, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectPostTaskForTime(zx::sec(0), zx::sec(0));
  ExpectNoOther();

  under_test.OnAddLocalServiceInstance(
      ServiceInstance(kServiceName, kInstanceName, kHostName, kSocketAddresses, kText),
      kFromLocalProxyHost);

  EXPECT_TRUE(callback_called);
  EXPECT_EQ(kServiceName.to_string(), instance_from_callback.service());
  EXPECT_EQ(kInstanceName, instance_from_callback.instance());
  EXPECT_EQ(kHostName.to_string(), instance_from_callback.target());
  EXPECT_EQ(0, instance_from_callback.srv_priority());
  EXPECT_EQ(0, instance_from_callback.srv_weight());
  EXPECT_EQ(fidl::To<std::vector<fuchsia::net::SocketAddress>>(kSocketAddresses),
            instance_from_callback.addresses());
  EXPECT_EQ(kText, instance_from_callback.text_strings());
}

// Regression test for b/274710801
TEST_F(InstanceResolverTest, LocalProxyInstanceFail) {
  const DnsLabel kAnotherInstanceName("another-testinstance");

  fuchsia::net::mdns::ServiceInstance instance_from_callback;
  bool callback_called = false;

  ServiceInstanceResolver under_test(
      this, kServiceName, kInstanceName, now(), Media::kBoth, IpVersions::kBoth, kIncludeLocal,
      kIncludeLocalProxies,
      [&instance_from_callback, &callback_called](fuchsia::net::mdns::ServiceInstance instance) {
        instance_from_callback = std::move(instance);
        callback_called = true;
      });

  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  // Expect a SRV and TXT questions on start.
  ExpectQueryCall(DnsType::kSrv, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectQueryCall(DnsType::kTxt, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectPostTaskForTime(zx::sec(0), zx::sec(0));
  ExpectNoOther();

  // |ServiceInstanceResolver| |under_test| test is created with |kInstanceName| while we are trying
  // to add local service instance of name |kAnotherInstanceName|. We dont expect the callback to
  // get triggered.
  under_test.OnAddLocalServiceInstance(
      ServiceInstance(kServiceName, kAnotherInstanceName, kHostName, kSocketAddresses, kText),
      kFromLocalProxyHost);

  EXPECT_FALSE(callback_called);
}

TEST_F(InstanceResolverTest, ResponseWithoutAaaa) {
  fuchsia::net::mdns::ServiceInstance instance_from_callback;
  bool callback_called = false;

  ServiceInstanceResolver under_test(
      this, kServiceName, kInstanceName, now(), Media::kBoth, IpVersions::kBoth, kExcludeLocal,
      kIncludeLocalProxies,
      [&instance_from_callback, &callback_called](fuchsia::net::mdns::ServiceInstance instance) {
        instance_from_callback = std::move(instance);
        callback_called = true;
      });

  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  // Expect a SRV & TXT questions on start.
  ExpectQueryCall(DnsType::kSrv, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectQueryCall(DnsType::kTxt, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectPostTaskForTime(zx::sec(0), zx::sec(0));
  ExpectNoOther();

  // Receive publication with no aaaa record.
  ReplyAddress sender_address(inet::SocketAddress(0xfe80, 1, inet::IpPort::From_uint16_t(5353)),
                              inet::IpAddress(0xfe80, 100), 1, Media::kWireless, IpVersions::kV6);
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kText,
                     sender_address, false);

  // Expect a aaaa query call.
  ExpectQueryCall(DnsType::kAaaa, kHostFullName, Media::kBoth, IpVersions::kBoth, now(),
                  kAdditionalInterval, kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);

  // Receive a response with only an aaaa record.
  ReceiveAddress(under_test, kHostFullName, sender_address);
  ExpectPostTaskForTime(zx::sec(0), zx::sec(0));
  ExpectNoOther();

  EXPECT_TRUE(callback_called);
  EXPECT_EQ(kServiceName.to_string(), instance_from_callback.service());
  EXPECT_EQ(kInstanceName, instance_from_callback.instance());
  EXPECT_EQ(kHostName.to_string(), instance_from_callback.target());
  EXPECT_EQ(kText, instance_from_callback.text_strings());
}

TEST_F(InstanceResolverTest, WaitsForTxtRecord) {
  fuchsia::net::mdns::ServiceInstance instance_from_callback;
  bool callback_called = false;

  ServiceInstanceResolver under_test(
      this, kServiceName, kInstanceName, now(), Media::kBoth, IpVersions::kBoth, kExcludeLocal,
      kIncludeLocalProxies,
      [&instance_from_callback, &callback_called](fuchsia::net::mdns::ServiceInstance instance) {
        instance_from_callback = std::move(instance);
        callback_called = true;
      });

  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  // Expect SRV & TXT questions on start.
  ExpectQueryCall(DnsType::kSrv, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectQueryCall(DnsType::kTxt, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectPostTaskForTime(zx::sec(0), zx::sec(0));
  ExpectNoOther();

  // Receive SRV and AAAA first (without TXT).
  ReplyAddress sender_address(inet::SocketAddress(0xfe80, 1, inet::IpPort::From_uint16_t(5353)),
                              inet::IpAddress(0xfe80, 100), 1, Media::kWireless, IpVersions::kV6);
  ReceiveSrvAndAddress(under_test, kHostFullName, kServiceName, kInstanceName, kPort,
                       sender_address);

  // Callback should not be called yet because TXT record has not been received.
  EXPECT_FALSE(callback_called);

  // Now receive TXT record in a separate message.
  ReceiveTxt(under_test, kServiceName, kInstanceName, kText, sender_address);
  ExpectPostTaskForTime(zx::sec(0), zx::sec(0));
  ExpectNoOther();

  EXPECT_TRUE(callback_called);
  EXPECT_EQ(kServiceName.to_string(), instance_from_callback.service());
  EXPECT_EQ(kInstanceName, instance_from_callback.instance());
  EXPECT_EQ(kHostName.to_string(), instance_from_callback.target());
  EXPECT_EQ(kText, instance_from_callback.text_strings());
}

TEST_F(InstanceResolverTest, DuplicateAddressesEliminated) {
  fuchsia::net::mdns::ServiceInstance instance_from_callback;
  bool callback_called = false;

  ServiceInstanceResolver under_test(
      this, kServiceName, kInstanceName, now(), Media::kBoth, IpVersions::kBoth, kExcludeLocal,
      kIncludeLocalProxies,
      [&instance_from_callback, &callback_called](fuchsia::net::mdns::ServiceInstance instance) {
        instance_from_callback = std::move(instance);
        callback_called = true;
      });

  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  // Expect SRV & TXT questions on start.
  ExpectQueryCall(DnsType::kSrv, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectQueryCall(DnsType::kTxt, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectPostTaskForTime(zx::sec(0), zx::sec(0));
  ExpectNoOther();

  ReplyAddress sender_address(inet::SocketAddress(0xfe80, 1, inet::IpPort::From_uint16_t(5353)),
                              inet::IpAddress(0xfe80, 100), 1, Media::kWireless, IpVersions::kV6);
  // Receive SRV and first AAAA address.
  ReceiveSrvAndAddress(under_test, kHostFullName, kServiceName, kInstanceName, kPort,
                       sender_address);

  // Receive duplicate AAAA address before TXT.
  ReceiveAddress(under_test, kHostFullName, sender_address);

  EXPECT_FALSE(callback_called);

  // Now receive TXT record, completing resolution.
  ReceiveTxt(under_test, kServiceName, kInstanceName, kText, sender_address);
  ExpectPostTaskForTime(zx::sec(0), zx::sec(0));
  ExpectNoOther();

  EXPECT_TRUE(callback_called);
  ASSERT_EQ(instance_from_callback.addresses().size(), 1UL);
}

TEST_F(InstanceResolverTest, EmptyTxtRecord) {
  fuchsia::net::mdns::ServiceInstance instance_from_callback;
  bool callback_called = false;

  ServiceInstanceResolver under_test(
      this, kServiceName, kInstanceName, now(), Media::kBoth, IpVersions::kBoth, kExcludeLocal,
      kIncludeLocalProxies,
      [&instance_from_callback, &callback_called](fuchsia::net::mdns::ServiceInstance instance) {
        instance_from_callback = std::move(instance);
        callback_called = true;
      });

  SetAgent(under_test);

  under_test.Start(kLocalHostFullName);

  // Expect SRV & TXT questions on start.
  ExpectQueryCall(DnsType::kSrv, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectQueryCall(DnsType::kTxt, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectPostTaskForTime(zx::sec(0), zx::sec(0));
  ExpectNoOther();

  ReplyAddress sender_address(inet::SocketAddress(0xfe80, 1, inet::IpPort::From_uint16_t(5353)),
                              inet::IpAddress(0xfe80, 100), 1, Media::kWireless, IpVersions::kV6);
  // Receive publication with empty text strings.
  ReceivePublication(under_test, kHostFullName, kServiceName, kInstanceName, kPort, kEmptyText,
                     sender_address, true);
  ExpectPostTaskForTime(zx::sec(0), zx::sec(0));
  ExpectNoOther();

  EXPECT_TRUE(callback_called);
  EXPECT_EQ(kServiceName.to_string(), instance_from_callback.service());
  EXPECT_EQ(kInstanceName, instance_from_callback.instance());
  EXPECT_EQ(kHostName.to_string(), instance_from_callback.target());
  EXPECT_TRUE(instance_from_callback.has_text_strings());
  EXPECT_TRUE(instance_from_callback.text_strings().empty());
}

TEST_F(InstanceResolverTest, NoTxtRecordTimesOut) {
  fuchsia::net::mdns::ServiceInstance instance_from_callback;
  bool callback_called = false;
  zx::duration timeout = zx::sec(10);

  auto under_test = std::make_shared<ServiceInstanceResolver>(
      this, kServiceName, kInstanceName, now() + timeout, Media::kBoth, IpVersions::kBoth,
      kExcludeLocal, kIncludeLocalProxies,
      [&instance_from_callback, &callback_called](fuchsia::net::mdns::ServiceInstance instance) {
        instance_from_callback = std::move(instance);
        callback_called = true;
      });

  SetAgent(*under_test);

  under_test->Start(kLocalHostFullName);

  // Expect SRV & TXT questions on start.
  ExpectQueryCall(DnsType::kSrv, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  ExpectQueryCall(DnsType::kTxt, MdnsNames::InstanceFullName(kInstanceName, kServiceName),
                  Media::kBoth, IpVersions::kBoth, now(), kAdditionalInterval,
                  kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
  // Start() posts the timeout task for timeout.
  auto [timeout_task, scheduled_time] = ExpectPostTaskForTime(timeout, timeout);
  ExpectNoOther();

  ReplyAddress sender_address(inet::SocketAddress(0xfe80, 1, inet::IpPort::From_uint16_t(5353)),
                              inet::IpAddress(0xfe80, 100), 1, Media::kWireless, IpVersions::kV6);
  // Receive SRV and AAAA without TXT.
  ReceiveSrvAndAddress(*under_test, kHostFullName, kServiceName, kInstanceName, kPort,
                       sender_address);

  // Callback should not be called yet because no TXT record was received.
  EXPECT_FALSE(callback_called);

  // Advance time and invoke the timeout task.
  AdvanceTo(scheduled_time);
  timeout_task();
  ExpectRemoveAgentCall();
  ExpectNoOther();

  // Resolution completes on timeout and delivers whatever records were received.
  EXPECT_TRUE(callback_called);
  EXPECT_EQ(kServiceName.to_string(), instance_from_callback.service());
  EXPECT_EQ(kInstanceName, instance_from_callback.instance());
  EXPECT_EQ(kHostName.to_string(), instance_from_callback.target());
  EXPECT_FALSE(instance_from_callback.has_text_strings());
}

}  // namespace test
}  // namespace mdns
