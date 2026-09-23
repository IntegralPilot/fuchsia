// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_TEST_AGENT_TEST_H_
#define SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_TEST_AGENT_TEST_H_

#include <lib/zx/time.h>

#include <memory>
#include <queue>
#include <unordered_map>

#include <gtest/gtest.h>

#include "src/connectivity/network/mdns/service/agents/mdns_agent.h"

namespace mdns {
namespace test {

class AgentTest : public ::testing::Test, public MdnsAgent::Owner {
 public:
  AgentTest() {}

 protected:
  static constexpr zx::time kInitialTime = zx::time(1000);
  static const DnsName kLocalHostName;
  static const DnsName kLocalHostFullName;
  static const DnsName kAlternateCaseLocalHostFullName;

  // Sets the agent under test. This must be called before the test gets underway, and the agent
  // must survive until the end of the test.
  void SetAgent(const MdnsAgent& agent) { agent_ = &agent; }
  void SetAgentB(const MdnsAgent& agent) { agent_b_ = &agent; }

  // Sets the host addresses returned by LocalHostAddresses.
  void SetLocalHostAddresses(std::vector<HostAddress> local_host_addresses) {
    local_host_addresses_ = std::move(local_host_addresses);
  }

  // Advances the current time (as returned by |now()|) to |time|. |time| must be greater than
  // or equal to the time currently returned by |now()|.
  void AdvanceTo(zx::time time);

  // Expects that the agent hasn't posted any new tasks.
  void ExpectNoPostTaskForTime() { EXPECT_TRUE(post_task_for_time_calls_.empty()); }

  // Expects that the agent has posted a task for a time in the given range. Returns the task
  // closure and the actual scheduled time.
  std::pair<fit::closure, zx::time> ExpectPostTaskForTime(zx::duration earliest,
                                                          zx::duration latest);
  // Calls |ExpectPostTaskForTime|, advances the time to the scheduled time of the task, and
  // invokes the task.
  void ExpectPostTaskForTimeAndInvoke(zx::duration earliest, zx::duration latest);

  // Expects that there is no outbond message.
  void ExpectNoOutboundMessage() { EXPECT_TRUE(outbound_messages_by_reply_address_.empty()); }

  // Expects that there is an outbound message targeted at |reply_address| and returns it.
  std::unique_ptr<DnsMessage> ExpectOutboundMessage(ReplyAddress reply_address);

  // Expects that the agent has not called |Renew|.
  void ExpectNoRenewCalls() { EXPECT_TRUE(renew_calls_.empty()); }

  // Expects that the agent has asked |resource| to be renewed.
  void ExpectRenewCall(DnsResource resource);

  // Expects that the agent has not called |Query|.
  void ExpectNoQueryCalls() { EXPECT_TRUE(query_calls_.empty()); }

  // Expects that the agent has called |Query|.
  void ExpectQueryCall(DnsType type, const DnsName& name, Media media, IpVersions ip_versions,
                       zx::time initial_query_time, zx::duration interval,
                       uint32_t interval_multiplier, uint32_t max_queries,
                       bool request_unicast_response);

  // Expects that the agent has not asked for any resources to be expired.
  void ExpectNoExpirations() { EXPECT_TRUE(expirations_.empty()); }

  // Expects that the agent has asked |resource| to be expired.
  void ExpectExpiration(DnsResource resource);

  // Expects that the agent has not called |RemoveAgent|.
  void ExpectNoRemoveAgentCall() { EXPECT_FALSE(remove_agent_called_); }

  // Expects that the agent has called |RemoveAgent| to remove itself.
  void ExpectRemoveAgentCall() {
    EXPECT_TRUE(remove_agent_called_);
    remove_agent_called_ = false;
  }

  // Expects that the agent has not called |MaybeSendMessages|.
  void ExpectNoMaybeSendMessagesCall() { EXPECT_FALSE(maybe_send_messages_called_); }

  // Expects that the agent has called |MaybeSendMessages|.
  void ExpectMaybeSendMessagesCall() {
    EXPECT_TRUE(maybe_send_messages_called_);
    maybe_send_messages_called_ = false;
  }

  // Expects that the agent has not called |AddLocalServiceInstance|.
  void ExpectNoAddLocalServiceInstanceCall() const {
    EXPECT_FALSE(add_local_service_instance_called_);
  }

  // Expects that the agent has called |AddLocalServiceInstance|.
  void ExpectAddLocalServiceInstanceCall(const ServiceInstance& instance, bool from_proxy) {
    EXPECT_TRUE(add_local_service_instance_called_);
    if (add_local_service_instance_called_) {
      EXPECT_EQ(instance, add_local_service_instance_instance_);
      EXPECT_EQ(from_proxy, add_local_service_instance_from_proxy_);
      add_local_service_instance_called_ = false;
    }
  }

  // Expects that the agent has not called |ChangeLocalServiceInstance|.
  void ExpectNoChangeLocalServiceInstanceCall() const {
    EXPECT_FALSE(change_local_service_instance_called_);
  }

  // Expects that the agent has called |ChangeLocalServiceInstance|.
  void ExpectChangeLocalServiceInstanceCall(const ServiceInstance& instance, bool from_proxy) {
    EXPECT_TRUE(change_local_service_instance_called_);
    EXPECT_EQ(instance, change_local_service_instance_instance_);
    EXPECT_EQ(from_proxy, change_local_service_instance_from_proxy_);
    change_local_service_instance_called_ = false;
  }

  // Expects that nothing else has happened. Subclasses can override this to ensure that nothing
  // specific to a particular agent type has happened. Overrides should call this implementation.
  virtual void ExpectNoOther();

  // Expects that |message| contains a question with the given parameters.
  void ExpectQuestion(DnsMessage* message, const DnsName& name, DnsType type,
                      DnsClass dns_class = DnsClass::kIn, bool unicast_response = false);

  // Expects that |message| contains a resource in |section| with the given parameters and returns
  // it.
  std::shared_ptr<DnsResource> ExpectResource(DnsMessage* message, MdnsResourceSection section,
                                              const DnsName& name, DnsType type,
                                              DnsClass dns_class = DnsClass::kIn);
  std::shared_ptr<DnsResource> ExpectResource(DnsMessage* message, MdnsResourceSection section,
                                              const DnsName& name, DnsType type, DnsClass dns_class,
                                              bool cache_flush);

  // Expects that |message| contains one or more resources in |section| with the given parameters
  // and returns them.
  std::vector<std::shared_ptr<DnsResource>> ExpectResources(DnsMessage* message,
                                                            MdnsResourceSection section,
                                                            const DnsName& name, DnsType type,
                                                            DnsClass dns_class = DnsClass::kIn);
  std::vector<std::shared_ptr<DnsResource>> ExpectResources(DnsMessage* message,
                                                            MdnsResourceSection section,
                                                            const DnsName& name, DnsType type,
                                                            DnsClass dns_class, bool cache_flush);

  // Expects that |message| contains an address placeholder resource in |section|.
  void ExpectAddressPlaceholder(DnsMessage* message, MdnsResourceSection section);

  // Expects that |message| contains resources for |addresses| in |section|.
  void ExpectAddresses(DnsMessage* message, MdnsResourceSection section,
                       const DnsName& host_full_name,
                       const std::vector<inet::IpAddress>& addresses);

  // Expect that |address| appears in |resources| and remove it.
  void ExpectAddress(std::vector<std::shared_ptr<DnsResource>>& resources, inet::IpAddress address);

  // Expects that |message| contains no questions or resources.
  void ExpectNoOtherQuestionOrResource(DnsMessage* message);

  void ExpectDeferMessagesCall() {
    EXPECT_TRUE(defer_messages_called_);
    defer_messages_called_ = false;
  }

  void ExpectUndeferMessagesCall(uint64_t seq) {
    EXPECT_EQ(seq, undefer_messages_called_);
    undefer_messages_called_ = 0;
  }

 private:
  struct PostTaskForTimeCall {
    fit::closure task_;
    zx::time target_time_;
  };

  struct RenewCall {
    std::shared_ptr<DnsResource> resource_;
  };

  struct QueryCall {
    DnsType type_;
    DnsName name_;
    Media media_;
    IpVersions ip_versions_;
    zx::time initial_query_time_;
    zx::duration interval_;
    uint32_t interval_multiplier_;
    uint32_t max_queries_;
    bool request_unicast_response_;
  };

  struct ReplyAddressHash {
    std::size_t operator()(const ReplyAddress& reply_address) const noexcept {
      return std::hash<inet::SocketAddress>{}(reply_address.socket_address()) ^
             (std::hash<inet::IpAddress>{}(reply_address.interface_address()) << 1) ^
             (std::hash<uint32_t>{}(reply_address.interface_id()) << 2) ^
             (std::hash<Media>{}(reply_address.media()) << 3) ^
             (std::hash<IpVersions>{}(reply_address.ip_versions()) << 4);
    }
  };

  // |MdnsAgent::Owner| implementation.
 protected:
  zx::time now() override { return now_; }

  void MaybeSendMessages() override;

 private:
  void PostTaskForTime(MdnsAgent* agent, fit::closure task, zx::time target_time) override;

  void SendQuestion(std::shared_ptr<DnsQuestion> question, ReplyAddress reply_address) override;

  void SendResource(std::shared_ptr<DnsResource> resource, MdnsResourceSection section,
                    const ReplyAddress& reply_address) override;

  void SendAddresses(MdnsResourceSection section, const ReplyAddress& reply_address) override;

  void Renew(const DnsResource& resource, Media media, IpVersions ip_versions) override;

  void Query(DnsType type, const DnsName& name, Media media, IpVersions ip_versions,
             zx::time initial_query_time, zx::duration interval, uint32_t interval_multiplier,
             uint32_t max_queries, bool request_unicast_response) override;

  void RemoveAgent(std::shared_ptr<MdnsAgent> agent) override;

  uint64_t DeferMessages() override;

  void UndeferMessages(uint64_t sequence_number) override;

  void AddLocalServiceInstance(const ServiceInstance& instance, bool from_proxy) override;

  void ChangeLocalServiceInstance(const ServiceInstance& instance, bool from_proxy) override;

  std::vector<HostAddress> LocalHostAddresses() override { return local_host_addresses_; }

  const MdnsAgent* agent_;
  const MdnsAgent* agent_b_;
  std::vector<HostAddress> local_host_addresses_;
  std::shared_ptr<DnsResource> address_placeholder_ =
      std::make_shared<DnsResource>(kLocalHostFullName, DnsType::kA);

  zx::time now_ = kInitialTime;

  std::queue<PostTaskForTimeCall> post_task_for_time_calls_;
  std::unordered_map<ReplyAddress, std::unique_ptr<DnsMessage>, ReplyAddressHash>
      outbound_messages_by_reply_address_;
  std::vector<RenewCall> renew_calls_;
  std::vector<QueryCall> query_calls_;
  std::vector<std::shared_ptr<DnsResource>> expirations_;
  bool remove_agent_called_ = false;
  bool maybe_send_messages_called_ = false;
  bool add_local_service_instance_called_ = false;
  ServiceInstance add_local_service_instance_instance_;
  bool add_local_service_instance_from_proxy_;
  bool change_local_service_instance_called_ = false;
  ServiceInstance change_local_service_instance_instance_;
  bool change_local_service_instance_from_proxy_;
  bool defer_messages_called_ = false;
  uint64_t undefer_messages_called_ = 0;
};

}  // namespace test
}  // namespace mdns

#endif  // SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_TEST_AGENT_TEST_H_
