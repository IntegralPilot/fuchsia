// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/test/agent_test.h"

#include <iostream>

#include "src/connectivity/network/mdns/service/common/formatters.h"
#include "src/connectivity/network/mdns/service/encoding/dns_formatting.h"

namespace mdns {
namespace test {

const DnsName AgentTest::kLocalHostName = DnsName("testhost");
const DnsName AgentTest::kLocalHostFullName = DnsName("testhost.local.");
const DnsName AgentTest::kAlternateCaseLocalHostFullName = DnsName("tEsThOsT.loCal.");

bool DefaultCacheFlush(DnsType type) {
  switch (type) {
    case DnsType::kPtr:
    case DnsType::kA:
    case DnsType::kAaaa:
      return false;
    default:
      return true;
  }
}

void AgentTest::PostTaskForTime(MdnsAgent* agent, fit::closure task, zx::time target_time) {
  EXPECT_TRUE(agent_ == agent || agent_b_ == agent);
  post_task_for_time_calls_.push(
      PostTaskForTimeCall{.task_ = std::move(task), .target_time_ = target_time});
}

void AgentTest::SendQuestion(std::shared_ptr<DnsQuestion> question, ReplyAddress reply_address) {
  EXPECT_NE(nullptr, question);
  auto& message = outbound_messages_by_reply_address_[reply_address];
  if (message == nullptr) {
    message = std::make_unique<DnsMessage>();
  }
  message->questions_.push_back(question);
}

void AgentTest::SendResource(std::shared_ptr<DnsResource> resource, MdnsResourceSection section,
                             const ReplyAddress& reply_address) {
  EXPECT_NE(nullptr, resource);

  if (section == MdnsResourceSection::kExpired) {
    expirations_.push_back(resource);
    return;
  }

  auto& message = outbound_messages_by_reply_address_[reply_address];
  if (message == nullptr) {
    message = std::make_unique<DnsMessage>();
  }

  switch (section) {
    case MdnsResourceSection::kAnswer:
      message->answers_.push_back(resource);
      break;
    case MdnsResourceSection::kAuthority:
      message->authorities_.push_back(resource);
      break;
    case MdnsResourceSection::kAdditional:
      message->additionals_.push_back(resource);
      break;
    case MdnsResourceSection::kExpired:
      EXPECT_TRUE(false);
      break;
  }
}

void AgentTest::SendAddresses(MdnsResourceSection section, const ReplyAddress& reply_address) {
  SendResource(address_placeholder_, section, reply_address);
}

void AgentTest::Renew(const DnsResource& resource, Media media, IpVersions ip_versions) {
  renew_calls_.push_back(RenewCall{.resource_ = std::make_shared<DnsResource>(resource)});
}

void AgentTest::Query(DnsType type, const DnsName& name, Media media, IpVersions ip_versions,
                      zx::time initial_query_time, zx::duration interval,
                      uint32_t interval_multiplier, uint32_t max_queries,
                      bool request_unicast_response) {
  query_calls_.push_back(QueryCall{
      .type_ = type,
      .name_ = name,
      .media_ = media,
      .ip_versions_ = ip_versions,
      .initial_query_time_ = initial_query_time,
      .interval_ = interval,
      .interval_multiplier_ = interval_multiplier,
      .max_queries_ = max_queries,
      .request_unicast_response_ = request_unicast_response,
  });
}

void AgentTest::ExpectRenewCall(DnsResource resource) {
  for (auto iter = renew_calls_.begin(); iter != renew_calls_.end(); ++iter) {
    if (iter->resource_ && iter->resource_->type_ == resource.type_ &&
        iter->resource_->name_ == resource.name_ &&
        iter->resource_->time_to_live_ == resource.time_to_live_) {
      renew_calls_.erase(iter);
      return;
    }
  }

  EXPECT_TRUE(false) << "Renew not called for resource " << resource.type_ << " " << resource.name_;
}

void AgentTest::ExpectQueryCall(DnsType type, const DnsName& name, Media media,
                                IpVersions ip_versions, zx::time initial_query_time,
                                zx::duration interval, uint32_t interval_multiplier,
                                uint32_t max_queries, bool request_unicast_response) {
  for (auto iter = query_calls_.begin(); iter != query_calls_.end(); ++iter) {
    if (iter->type_ == type && iter->name_ == name && iter->media_ == media &&
        iter->ip_versions_ == ip_versions) {
      EXPECT_EQ(initial_query_time, iter->initial_query_time_);
      EXPECT_EQ(interval, iter->interval_);
      EXPECT_EQ(interval_multiplier, iter->interval_multiplier_);
      EXPECT_EQ(max_queries, iter->max_queries_);
      EXPECT_EQ(request_unicast_response, iter->request_unicast_response_);
      query_calls_.erase(iter);
      return;
    }
  }

  EXPECT_TRUE(false) << "Query not called for resource " << type << " " << name;
}

void AgentTest::ExpectExpiration(DnsResource resource) {
  for (auto iter = expirations_.begin(); iter != expirations_.end(); ++iter) {
    if (*iter && **iter == resource) {
      expirations_.erase(iter);
      return;
    }
  }

  EXPECT_TRUE(false) << "Resource not expired.";
}

void AgentTest::RemoveAgent(std::shared_ptr<MdnsAgent> agent) {
  EXPECT_TRUE(agent_ == agent.get() || agent_b_ == agent.get());
  remove_agent_called_ = true;
}

void AgentTest::MaybeSendMessages() { maybe_send_messages_called_ = true; }

uint64_t AgentTest::DeferMessages() {
  defer_messages_called_ = true;
  return 1;
}

void AgentTest::UndeferMessages(uint64_t sequence_number) {
  undefer_messages_called_ = sequence_number;
}

void AgentTest::AddLocalServiceInstance(const ServiceInstance& instance, bool from_proxy) {
  add_local_service_instance_called_ = true;
  add_local_service_instance_instance_ = instance;
  add_local_service_instance_from_proxy_ = from_proxy;
}

void AgentTest::ChangeLocalServiceInstance(const ServiceInstance& instance, bool from_proxy) {
  change_local_service_instance_called_ = true;
  change_local_service_instance_instance_ = instance;
  change_local_service_instance_from_proxy_ = from_proxy;
}

void AgentTest::AdvanceTo(zx::time time) {
  EXPECT_LE(now_, time);
  now_ = time;
}

std::pair<fit::closure, zx::time> AgentTest::ExpectPostTaskForTime(zx::duration earliest,
                                                                   zx::duration latest) {
  EXPECT_FALSE(post_task_for_time_calls_.empty());
  auto task = std::move(post_task_for_time_calls_.front().task_);
  EXPECT_NE(nullptr, task);
  auto time = post_task_for_time_calls_.front().target_time_;
  post_task_for_time_calls_.pop();
  EXPECT_LE(now_ + earliest, time);
  EXPECT_GE(now_ + latest, time);
  return std::pair(std::move(task), time);
}

void AgentTest::ExpectPostTaskForTimeAndInvoke(zx::duration earliest, zx::duration latest) {
  auto [task, time] = ExpectPostTaskForTime(earliest, latest);
  ExpectNoOther();

  AdvanceTo(time);
  task();
}

std::unique_ptr<DnsMessage> AgentTest::ExpectOutboundMessage(ReplyAddress reply_address) {
  auto message = std::move(outbound_messages_by_reply_address_[reply_address]);
  EXPECT_NE(nullptr, message) << "Expected output message not found for " << reply_address;
  outbound_messages_by_reply_address_.erase(reply_address);
  return message;
}

void AgentTest::ExpectNoOther() {
  ExpectNoPostTaskForTime();
  ExpectNoOutboundMessage();
  ExpectNoRenewCalls();
  ExpectNoExpirations();
  ExpectNoRemoveAgentCall();
}

void AgentTest::ExpectQuestion(DnsMessage* message, const DnsName& name, DnsType type,
                               DnsClass dns_class, bool unicast_response) {
  EXPECT_NE(nullptr, message);
  if (!message) {
    return;
  }

  for (auto i = message->questions_.begin(); i != message->questions_.end(); ++i) {
    if ((*i)->name_ == name && (*i)->type_ == type && (*i)->class_ == dns_class &&
        (*i)->unicast_response_ == unicast_response) {
      message->questions_.erase(i);
      return;
    }
  }

  EXPECT_TRUE(false) << "No matching question with name " << name << " and type " << type
                     << " in message.";
}

std::shared_ptr<DnsResource> AgentTest::ExpectResource(DnsMessage* message,
                                                       MdnsResourceSection section,
                                                       const DnsName& name, DnsType type,
                                                       DnsClass dns_class) {
  return ExpectResource(message, section, name, type, dns_class, DefaultCacheFlush(type));
}

std::shared_ptr<DnsResource> AgentTest::ExpectResource(DnsMessage* message,
                                                       MdnsResourceSection section,
                                                       const DnsName& name, DnsType type,
                                                       DnsClass dns_class, bool cache_flush) {
  EXPECT_NE(nullptr, message);
  if (!message) {
    return nullptr;
  }

  std::vector<std::shared_ptr<DnsResource>>* collection;
  switch (section) {
    case MdnsResourceSection::kAnswer:
      collection = &message->answers_;
      break;
    case MdnsResourceSection::kAuthority:
      collection = &message->authorities_;
      break;
    case MdnsResourceSection::kAdditional:
      collection = &message->additionals_;
      break;
    case MdnsResourceSection::kExpired:
      EXPECT_TRUE(false);
      return nullptr;
  }

  for (auto i = collection->begin(); i != collection->end(); ++i) {
    if ((*i)->name_ == name && (*i)->type_ == type && (*i)->class_ == dns_class &&
        (*i)->cache_flush_ == cache_flush) {
      auto result = std::move(*i);
      collection->erase(i);
      return result;
    }
  }

  EXPECT_TRUE(false) << "No matching resource with name " << name << " and type " << type
                     << " in section " << section << " of message.";
  return nullptr;
}

std::vector<std::shared_ptr<DnsResource>> AgentTest::ExpectResources(DnsMessage* message,
                                                                     MdnsResourceSection section,
                                                                     const DnsName& name,
                                                                     DnsType type,
                                                                     DnsClass dns_class) {
  return ExpectResources(message, section, name, type, dns_class, DefaultCacheFlush(type));
}

std::vector<std::shared_ptr<DnsResource>> AgentTest::ExpectResources(
    DnsMessage* message, MdnsResourceSection section, const DnsName& name, DnsType type,
    DnsClass dns_class, bool cache_flush) {
  EXPECT_NE(nullptr, message);

  std::vector<std::shared_ptr<DnsResource>>* collection;
  switch (section) {
    case MdnsResourceSection::kAnswer:
      collection = &message->answers_;
      break;
    case MdnsResourceSection::kAuthority:
      collection = &message->authorities_;
      break;
    case MdnsResourceSection::kAdditional:
      collection = &message->additionals_;
      break;
    case MdnsResourceSection::kExpired:
      EXPECT_TRUE(false);
      return std::vector<std::shared_ptr<DnsResource>>();
  }

  std::vector<std::shared_ptr<DnsResource>> result;
  for (auto i = collection->begin(); i != collection->end();) {
    if ((*i)->name_ == name && (*i)->type_ == type && (*i)->class_ == dns_class &&
        (*i)->cache_flush_ == cache_flush) {
      result.push_back(std::move(*i));
      i = collection->erase(i);
    } else {
      ++i;
    }
  }

  EXPECT_FALSE(result.empty()) << "No matching resource with name " << name << " and type " << type
                               << " in section " << section << " of message.";
  return result;
}

void AgentTest::ExpectAddressPlaceholder(DnsMessage* message, MdnsResourceSection section) {
  ExpectResource(message, section, kLocalHostFullName, DnsType::kA);
}

void AgentTest::ExpectAddresses(DnsMessage* message, MdnsResourceSection section,
                                const DnsName& host_full_name,
                                const std::vector<inet::IpAddress>& addresses) {
  bool expect_v4 = false;
  bool expect_v6 = false;
  for (const auto& address : addresses) {
    if (address.is_v4()) {
      expect_v4 = true;
    } else {
      expect_v6 = true;
    }
  }

  if (expect_v4) {
    auto resources = ExpectResources(message, section, host_full_name, DnsType::kA);
    for (const auto& address : addresses) {
      if (address.is_v4()) {
        ExpectAddress(resources, address);
      }
    }
  }

  if (expect_v6) {
    auto resources = ExpectResources(message, section, host_full_name, DnsType::kAaaa);
    for (const auto& address : addresses) {
      if (address.is_v6()) {
        ExpectAddress(resources, address);
      }
    }
  }
}

void AgentTest::ExpectAddress(std::vector<std::shared_ptr<DnsResource>>& resources,
                              inet::IpAddress address) {
  for (auto i = resources.begin(); i != resources.end(); ++i) {
    if ((*i)->a_.address_.address_ == address) {
      resources.erase(i);
      return;
    }
  }

  EXPECT_TRUE(false) << "No matching address " << address;
}

void AgentTest::ExpectNoOtherQuestionOrResource(DnsMessage* message) {
  EXPECT_NE(nullptr, message);
  EXPECT_TRUE(message->questions_.empty());
  EXPECT_TRUE(message->answers_.empty());
  EXPECT_TRUE(message->authorities_.empty());
  EXPECT_TRUE(message->additionals_.empty());
}

}  // namespace test
}  // namespace mdns
