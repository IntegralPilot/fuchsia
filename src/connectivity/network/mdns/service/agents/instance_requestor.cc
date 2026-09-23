// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/agents/instance_requestor.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <iterator>

#include "src/connectivity/network/mdns/service/common/mdns_names.h"
#include "src/connectivity/network/mdns/service/common/types.h"
#include "src/connectivity/network/mdns/service/encoding/dns_formatting.h"

namespace mdns {
namespace {

constexpr zx::duration kMaxQueryInterval = zx::hour(1);
constexpr zx::duration kAdditionalInterval = zx::sec(1);
constexpr uint32_t kAdditionalIntervalMultiplier = 2;
constexpr uint32_t kAdditionalMaxQueries = 3;

}  // namespace

InstanceRequestor::InstanceRequestor(MdnsAgent::Owner* owner, const DnsName& service_name,
                                     Media media, IpVersions ip_versions, bool include_local,
                                     bool include_local_proxies)
    : MdnsAgent(owner),
      service_name_(service_name),
      service_full_name_(MdnsNames::ServiceFullName(service_name)),
      media_(media),
      ip_versions_(ip_versions),
      include_local_(include_local),
      include_local_proxies_(include_local_proxies),
      question_(std::make_shared<DnsQuestion>(service_full_name_, DnsType::kPtr)) {}

InstanceRequestor::InstanceRequestor(MdnsAgent::Owner* owner, Media media, IpVersions ip_versions,
                                     bool include_local, bool include_local_proxies)
    : MdnsAgent(owner),
      service_name_(""),
      service_full_name_(MdnsNames::kAnyServiceFullName),
      media_(media),
      ip_versions_(ip_versions),
      include_local_(include_local),
      include_local_proxies_(include_local_proxies),
      question_(std::make_shared<DnsQuestion>(service_full_name_, DnsType::kPtr)) {}

void InstanceRequestor::AddSubscriber(Mdns::Subscriber* subscriber) {
  subscribers_.insert(subscriber);
  if (started()) {
    ReportAllDiscoveries(subscriber);
  }
}

void InstanceRequestor::RemoveSubscriber(Mdns::Subscriber* subscriber) {
  subscribers_.erase(subscriber);
  if (subscribers_.empty()) {
    PostTaskForTime([this, own_this = shared_from_this()]() { Quit(); }, now());
  }
}

void InstanceRequestor::Start(const DnsName& local_host_full_name) {
  MdnsAgent::Start(local_host_full_name);
  SendQuery();
}

void InstanceRequestor::ReceiveResource(const DnsResource& resource, MdnsResourceSection section,
                                        ReplyAddress sender_address) {
  if (!sender_address.Matches(media_) || !sender_address.Matches(ip_versions_)) {
    return;
  }

  switch (resource.type_) {
    case DnsType::kPtr:
      if (resource.name_ == service_full_name_ ||
          service_full_name_ == MdnsNames::kAnyServiceFullName) {
        // PTR records referring to local instances are ignored in |ReceivePtrResource|.
        ReceivePtrResource(resource, section);
      }
      break;
    case DnsType::kSrv: {
      auto iter = instance_infos_by_full_name_.find(resource.name_);
      if (iter != instance_infos_by_full_name_.end() && !iter->second.is_local_) {
        ReceiveSrvResource(resource, section, &iter->second);
      }
    } break;
    case DnsType::kTxt: {
      auto iter = instance_infos_by_full_name_.find(resource.name_);
      if (iter != instance_infos_by_full_name_.end() && !iter->second.is_local_) {
        ReceiveTxtResource(resource, section, &iter->second);
      }
    } break;
    case DnsType::kA: {
      auto iter = target_infos_by_full_name_.find(resource.name_);
      if (iter != target_infos_by_full_name_.end() && !iter->second.is_local_) {
        ReceiveAResource(resource, section, &iter->second, sender_address.interface_id());
      }
    } break;
    case DnsType::kAaaa: {
      auto iter = target_infos_by_full_name_.find(resource.name_);
      if (iter != target_infos_by_full_name_.end() && !iter->second.is_local_) {
        ReceiveAaaaResource(resource, section, &iter->second, sender_address.interface_id());
      }
    } break;
    default:
      break;
  }
}

void InstanceRequestor::EndOfMessage() {
  // Report updates.
  for (auto& [instance_full_name, instance_info] : instance_infos_by_full_name_) {
    if (instance_info.target_.empty()) {
      // We haven't yet seen an SRV record for this instance.
      if (!instance_info.srv_queried_) {
        // We have not received an SRV resource, and we haven't asked for one explicitly. Ask for
        // one now.
        Query(DnsType::kSrv, instance_full_name, media_, ip_versions_, now(), kAdditionalInterval,
              kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
        instance_info.srv_queried_ = true;
      }

      continue;
    }

    auto iter = target_infos_by_full_name_.find(instance_info.target_full_name_);
    if (iter == target_infos_by_full_name_.end()) {
      FX_DCHECK(false) << "Failed to find target for SRV target name "
                       << instance_info.target_full_name_;
      FX_LOGS(ERROR) << "Failed to find target for SRV target name "
                     << instance_info.target_full_name_;
      continue;
    }

    const DnsName& target_full_name = iter->first;
    TargetInfo& target_info = iter->second;

    // Keep this target info around.
    target_info.keep_ = true;

    if (!instance_info.dirty_ && !target_info.dirty_) {
      // Both the instance info and target info are clean.
      continue;
    }

    if (!instance_info.txt_received_ && !instance_info.txt_queried_) {
      // We have not received a TXT resource, and we haven't asked for one explicitly. Ask for one
      // now. We proceed anyway, which means the client may first get the instance without text
      // and later get an |InstanceChanged| with text.
      Query(DnsType::kTxt, instance_full_name, media_, ip_versions_, now(), kAdditionalInterval,
            kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
      instance_info.txt_queried_ = true;
    }

    if (target_info.addresses_.empty()) {
      // No addresses. This happens when an instance doesn't include A/AAAA records along with the
      // PTR and SRV.
      if (!target_info.addresses_queried_) {
        // If we haven't explicitly queried for addresses yet, do so. When they arrive, we'll end
        // up back in this method with a non-empty |target_info.addresses_|. If they never arrive,
        // the instance won't be reported to the client.
        //
        // Note that these queries will be coalesced into a single message due to the identical
        // schedule.
        auto when = now();
        Query(DnsType::kA, target_full_name, media_, ip_versions_, when, kAdditionalInterval,
              kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
        Query(DnsType::kAaaa, target_full_name, media_, ip_versions_, when, kAdditionalInterval,
              kAdditionalIntervalMultiplier, kAdditionalMaxQueries, true);
        target_info.addresses_queried_ = true;
      }

      continue;
    }

    // Something has changed.
    std::vector<Mdns::Subscriber*> subscribers(subscribers_.begin(), subscribers_.end());
    if (instance_info.new_) {
      instance_info.new_ = false;
      for (auto subscriber : subscribers) {
        subscriber->InstanceDiscovered(
            instance_info.service_name_, instance_info.instance_name_,
            target_info.addresses_.AddressesWithPort(instance_info.port_), instance_info.text_,
            instance_info.srv_priority_, instance_info.srv_weight_, instance_info.target_);
      }
    } else {
      for (auto subscriber : subscribers) {
        subscriber->InstanceChanged(instance_info.service_name_, instance_info.instance_name_,
                                    target_info.addresses_.AddressesWithPort(instance_info.port_),
                                    instance_info.text_, instance_info.srv_priority_,
                                    instance_info.srv_weight_, instance_info.target_);
      }
    }

    instance_info.dirty_ = false;
  }

  // Clean up |target_infos_by_full_name_|.
  for (auto iter = target_infos_by_full_name_.begin(); iter != target_infos_by_full_name_.end();) {
    if (iter->second.keep_) {
      iter->second.dirty_ = false;
      iter->second.keep_ = false;
      ++iter;
    } else {
      // No instances reference this target. Get rid of it.
      iter = target_infos_by_full_name_.erase(iter);
    }
  }
}

void InstanceRequestor::ReportAllDiscoveries(Mdns::Subscriber* subscriber) {
  for (const auto& [_, instance_info] : instance_infos_by_full_name_) {
    if (instance_info.target_.empty()) {
      // We haven't yet seen an SRV record for this instance.
      continue;
    }

    auto iter = target_infos_by_full_name_.find(instance_info.target_full_name_);
    FX_DCHECK(iter != target_infos_by_full_name_.end());
    TargetInfo& target_info = iter->second;

    if (target_info.addresses_.empty()) {
      // No addresses yet.
      continue;
    }

    subscriber->InstanceDiscovered(instance_info.service_name_, instance_info.instance_name_,
                                   target_info.addresses_.AddressesWithPort(instance_info.port_),
                                   instance_info.text_, instance_info.srv_priority_,
                                   instance_info.srv_weight_, instance_info.target_);

    // |InstanceDiscovered| may delete the subscriber synchronously (removing it from
    // |subscribers_|). Stop reporting in that case so no further calls are made through the
    // dangling pointer.
    if (subscribers_.find(subscriber) == subscribers_.end()) {
      return;
    }
  }
}

void InstanceRequestor::SendQuery() {
  SendQuestion(question_, ReplyAddress::Multicast(media_, ip_versions_));
  // |Query| may delete the subscriber synchronously (removing it from |subscribers_|), so
  // iterate over a snapshot to keep the loop iterator valid.
  std::vector<Mdns::Subscriber*> subscribers(subscribers_.begin(), subscribers_.end());
  for (const auto& subscriber : subscribers) {
    subscriber->Query(question_->type_);
  }

  if (query_delay_ == zx::sec(0)) {
    query_delay_ = zx::sec(1);
  } else {
    query_delay_ = query_delay_ * 2;
    if (query_delay_ > kMaxQueryInterval) {
      query_delay_ = kMaxQueryInterval;
    }
  }

  PostTaskForTime([this]() { SendQuery(); }, now() + query_delay_);
}

void InstanceRequestor::ReceivePtrResource(const DnsResource& resource,
                                           MdnsResourceSection section) {
  const DnsName& instance_full_name = resource.ptr_.pointer_domain_name_;

  DnsLabel instance_name;
  DnsName service_name;
  if (!MdnsNames::SplitInstanceFullName(instance_full_name, &instance_name, &service_name)) {
    return;
  }

  if (resource.time_to_live_ == 0) {
    RemoveInstance(instance_full_name);
    return;
  }

  if (instance_infos_by_full_name_.find(instance_full_name) == instance_infos_by_full_name_.end()) {
    auto [iter, inserted] =
        instance_infos_by_full_name_.emplace(instance_full_name, InstanceInfo{});
    FX_DCHECK(inserted);
    iter->second.service_name_ = service_name;
    iter->second.instance_name_ = instance_name;
  }

  Renew(resource, media_, ip_versions_);
}

void InstanceRequestor::ReceiveSrvResource(const DnsResource& resource, MdnsResourceSection section,
                                           InstanceInfo* instance_info) {
  if (resource.time_to_live_ == 0) {
    RemoveInstance(resource.name_);
    return;
  }

  if (instance_info->target_.empty() ||
      MdnsNames::HostFullName(instance_info->target_) != resource.srv_.target_) {
    instance_info->target_ = MdnsNames::HostNameFromFullName(resource.srv_.target_);
    instance_info->target_full_name_ = resource.srv_.target_;
    instance_info->dirty_ = true;

    if (target_infos_by_full_name_.find(instance_info->target_full_name_) ==
        target_infos_by_full_name_.end()) {
      target_infos_by_full_name_.emplace(instance_info->target_full_name_, TargetInfo{});
    }
  }

  if (instance_info->srv_priority_ != resource.srv_.priority_) {
    instance_info->srv_priority_ = resource.srv_.priority_;
    instance_info->dirty_ = true;
  }

  if (instance_info->srv_weight_ != resource.srv_.weight_) {
    instance_info->srv_weight_ = resource.srv_.weight_;
    instance_info->dirty_ = true;
  }

  if (instance_info->port_ != resource.srv_.port_) {
    instance_info->port_ = resource.srv_.port_;
    instance_info->dirty_ = true;
  }

  Renew(resource, media_, ip_versions_);
}

void InstanceRequestor::ReceiveTxtResource(const DnsResource& resource, MdnsResourceSection section,
                                           InstanceInfo* instance_info) {
  if (resource.time_to_live_ == 0) {
    if (!instance_info->text_.empty()) {
      instance_info->text_.clear();
      instance_info->dirty_ = true;
    }

    return;
  }

  instance_info->txt_received_ = true;

  if (instance_info->text_.size() != resource.txt_.strings_.size()) {
    instance_info->text_.resize(resource.txt_.strings_.size());
    instance_info->dirty_ = true;
  }

  for (size_t i = 0; i < instance_info->text_.size(); ++i) {
    if (instance_info->text_[i] != resource.txt_.strings_[i]) {
      instance_info->text_[i] = resource.txt_.strings_[i];
      instance_info->dirty_ = true;
    }
  }

  Renew(resource, media_, ip_versions_);
}

void InstanceRequestor::ReceiveAResource(const DnsResource& resource, MdnsResourceSection section,
                                         TargetInfo* target_info, uint32_t scope_id) {
  inet::SocketAddress address(resource.a_.address_.address_, inet::IpPort(), scope_id);

  if (resource.time_to_live_ == 0) {
    if (target_info->addresses_.erase(address)) {
      target_info->dirty_ = true;
    }

    return;
  }

  if (resource.cache_flush_) {
    target_info->addresses_.EraseV4AddressesOlderThanOneSecond();
  }

  if (target_info->addresses_.insert(address)) {
    target_info->dirty_ = true;
  }

  Renew(resource, media_, ip_versions_);
}

void InstanceRequestor::ReceiveAaaaResource(const DnsResource& resource,
                                            MdnsResourceSection section, TargetInfo* target_info,
                                            uint32_t scope_id) {
  inet::SocketAddress address(resource.aaaa_.address_.address_, inet::IpPort(), scope_id);

  if (resource.time_to_live_ == 0) {
    if (target_info->addresses_.erase(address)) {
      target_info->dirty_ = true;
    }

    return;
  }

  if (resource.cache_flush_) {
    target_info->addresses_.EraseV6AddressesOlderThanOneSecond();
  }

  if (target_info->addresses_.insert(address)) {
    target_info->dirty_ = true;
  }

  Renew(resource, media_, ip_versions_);
}

void InstanceRequestor::RemoveInstance(const DnsName& instance_full_name) {
  auto iter = instance_infos_by_full_name_.find(instance_full_name);
  if (iter != instance_infos_by_full_name_.end()) {
    // We've received a record via a transport referring to a local instance. It's invalid, and we
    // ignore it.
    if (iter->second.is_local_) {
      return;
    }

    // |InstanceLost| may delete the subscriber synchronously (removing it from |subscribers_|),
    // so iterate over a snapshot to keep the loop iterator valid.
    std::vector<Mdns::Subscriber*> subscribers(subscribers_.begin(), subscribers_.end());
    for (auto subscriber : subscribers) {
      subscriber->InstanceLost(iter->second.service_name_, iter->second.instance_name_);
    }

    instance_infos_by_full_name_.erase(iter);
  }
}

void InstanceRequestor::RemoveLocalInstance(const DnsName& instance_full_name) {
  auto iter = instance_infos_by_full_name_.find(instance_full_name);
  if (iter != instance_infos_by_full_name_.end()) {
    // |InstanceLost| may delete the subscriber synchronously (removing it from |subscribers_|),
    // so iterate over a snapshot to keep the loop iterator valid.
    std::vector<Mdns::Subscriber*> subscribers(subscribers_.begin(), subscribers_.end());
    for (auto subscriber : subscribers) {
      subscriber->InstanceLost(iter->second.service_name_, iter->second.instance_name_);
    }

    instance_infos_by_full_name_.erase(iter);
  }
}

void InstanceRequestor::OnAddLocalServiceInstance(const Mdns::ServiceInstance& instance,
                                                  bool from_proxy) {
  if (from_proxy ? !include_local_proxies_ : !include_local_) {
    return;
  }

  if (instance.service_name_ != service_name_ &&
      service_full_name_ != MdnsNames::kAnyServiceFullName) {
    return;
  }

  if (instance.addresses_.empty()) {
    FX_LOGS(ERROR) << "OnAddLocalServiceInstance called with empty address list.";
    return;
  }

  auto instance_full_name =
      MdnsNames::InstanceFullName(instance.instance_name_, instance.service_name_);
  auto [instance_iter, inserted] =
      instance_infos_by_full_name_.insert(std::make_pair(instance_full_name, InstanceInfo()));
  auto& instance_info = instance_iter->second;
  instance_info.service_name_ = instance.service_name_;
  instance_info.instance_name_ = instance.instance_name_;
  instance_info.target_ = instance.target_name_;
  instance_info.target_full_name_ = MdnsNames::HostFullName(instance.target_name_);
  instance_info.port_ = instance.addresses_[0].port();
  instance_info.text_ = instance.text_;
  instance_info.srv_priority_ = instance.srv_priority_;
  instance_info.srv_weight_ = instance.srv_weight_;
  instance_info.new_ = inserted;
  instance_info.is_local_ = true;

  auto [target_iter, _] = target_infos_by_full_name_.insert(
      std::make_pair(instance_info.target_full_name_, TargetInfo{.is_local_ = true}));
  auto& target_info = target_iter->second;

  if (target_info.addresses_.Replace(instance.addresses_)) {
    target_info.dirty_ = true;
  }

  EndOfMessage();
}

void InstanceRequestor::OnChangeLocalServiceInstance(const Mdns::ServiceInstance& instance,
                                                     bool from_proxy) {
  if (from_proxy ? !include_local_proxies_ : !include_local_) {
    return;
  }

  if (instance.service_name_ != service_name_ &&
      service_full_name_ != MdnsNames::kAnyServiceFullName) {
    return;
  }

  FX_DCHECK(!instance.addresses_.empty());

  auto instance_full_name =
      MdnsNames::InstanceFullName(instance.instance_name_, instance.service_name_);
  auto iter = instance_infos_by_full_name_.find(instance_full_name);
  if (iter == instance_infos_by_full_name_.end()) {
    return;
  }

  auto& instance_info = iter->second;
  FX_DCHECK(instance_info.instance_name_ == instance.instance_name_);
  if (instance_info.target_ != instance.target_name_) {
    instance_info.target_ = instance.target_name_;
    instance_info.target_full_name_ = MdnsNames::HostFullName(instance.target_name_);
    instance_info.dirty_ = true;
  }

  if (instance_info.port_ != instance.addresses_[0].port()) {
    instance_info.port_ = instance.addresses_[0].port();
    instance_info.dirty_ = true;
  }

  if (instance_info.text_ != instance.text_) {
    instance_info.text_ = instance.text_;
    instance_info.dirty_ = true;
  }

  if (instance_info.srv_priority_ != instance.srv_priority_) {
    instance_info.srv_priority_ = instance.srv_priority_;
    instance_info.dirty_ = true;
  }

  if (instance_info.srv_weight_ != instance.srv_weight_) {
    instance_info.srv_weight_ = instance.srv_weight_;
    instance_info.dirty_ = true;
  }

  auto target_full_name = MdnsNames::HostFullName(instance.target_name_);
  auto [target_iter, _] = target_infos_by_full_name_.insert(
      std::make_pair(target_full_name, TargetInfo{.is_local_ = true}));
  auto& target_info = target_iter->second;

  if (target_info.addresses_.Replace(instance.addresses_)) {
    target_info.dirty_ = true;
  }

  if (instance_info.dirty_ || target_info.dirty_) {
    EndOfMessage();
  }
}

void InstanceRequestor::OnRemoveLocalServiceInstance(const DnsName& service_name,
                                                     const DnsLabel& instance_name,
                                                     bool from_proxy) {
  if (from_proxy ? !include_local_proxies_ : !include_local_) {
    return;
  }

  if (service_name != service_name_ && service_full_name_ != MdnsNames::kAnyServiceFullName) {
    return;
  }

  RemoveLocalInstance(MdnsNames::InstanceFullName(instance_name, service_name));

  EndOfMessage();
}

/////////////////////////////////////////////////////////////////////////////////////////////////
// TargetAddressSet implementation.

bool InstanceRequestor::TargetAddressSet::Replace(const std::vector<inet::SocketAddress>& from) {
  bool result = false;

  if (from.size() != insert_times_by_address_.size()) {
    result = true;
  } else {
    for (auto& address : from) {
      if (insert_times_by_address_.find(address) == insert_times_by_address_.end()) {
        result = true;
        break;
      }
    }
  }

  // Note that |Replace| is used only for local instances, which are updated wholesale and to which
  // cache_flush does not apply. We use a bogus time value |zx::time()| and don't bother updating
  // times when the addresses don't change.
  if (result) {
    insert_times_by_address_.clear();
    std::transform(from.begin(), from.end(),
                   std::inserter(insert_times_by_address_, insert_times_by_address_.begin()),
                   [](auto& address) { return std::pair(address, zx::time()); });
  }

  return result;
}

void InstanceRequestor::TargetAddressSet::EraseV4AddressesOlderThanOneSecond() {
  auto one_second_ago = zx::clock::get_monotonic() - zx::sec(1);

  for (auto iter = insert_times_by_address_.begin(); iter != insert_times_by_address_.end();) {
    if (iter->first.is_v4() && iter->second < one_second_ago) {
      iter = insert_times_by_address_.erase(iter);
    } else {
      ++iter;
    }
  }
}

// Erases all IPv6 addresses that are older than one second.
void InstanceRequestor::TargetAddressSet::EraseV6AddressesOlderThanOneSecond() {
  auto one_second_ago = zx::clock::get_monotonic() - zx::sec(1);

  for (auto iter = insert_times_by_address_.begin(); iter != insert_times_by_address_.end();) {
    if (iter->first.is_v6() && iter->second < one_second_ago) {
      iter = insert_times_by_address_.erase(iter);
    } else {
      ++iter;
    }
  }
}

std::vector<inet::SocketAddress> InstanceRequestor::TargetAddressSet::AddressesWithPort(
    inet::IpPort port) {
  std::vector<inet::SocketAddress> result;
  std::transform(insert_times_by_address_.begin(), insert_times_by_address_.end(),
                 std::back_inserter(result), [port](auto& pair) {
                   return inet::SocketAddress(pair.first.address(), port, pair.first.scope_id());
                 });
  return result;
}

}  // namespace mdns
