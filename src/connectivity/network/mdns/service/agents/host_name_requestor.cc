// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/agents/host_name_requestor.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <algorithm>
#include <iterator>

#include "src/connectivity/network/mdns/service/common/mdns_names.h"

namespace mdns {

HostNameRequestor::HostNameRequestor(MdnsAgent::Owner* owner, const DnsName& host_name, Media media,
                                     IpVersions ip_versions, bool include_local,
                                     bool include_local_proxies)
    : MdnsAgent(owner),
      host_name_(host_name),
      host_full_name_(MdnsNames::HostFullName(host_name)),
      media_(media),
      ip_versions_(ip_versions),
      include_local_(include_local),
      include_local_proxies_(include_local_proxies) {}

HostNameRequestor::~HostNameRequestor() = default;

void HostNameRequestor::AddSubscriber(Mdns::HostNameSubscriber* subscriber) {
  subscribers_.insert(subscriber);
  if (started() && !addresses_.empty()) {
    subscriber->AddressesChanged(addresses_.Addresses());
  }
}

void HostNameRequestor::RemoveSubscriber(Mdns::HostNameSubscriber* subscriber) {
  subscribers_.erase(subscriber);
  if (subscribers_.empty()) {
    // Post the removal rather than calling |RemoveSelf| here, because this method can be called
    // synchronously from a subscriber callback (for example, from |MaybeSendAddressesChanged|),
    // and agents must not remove themselves during inbound message processing. Using |Quit| also
    // runs the on-quit callback, which removes this requestor from the owner's lookup table.
    PostTaskForTime([this, own_this = shared_from_this()]() { Quit(); }, now());
  }
}

void HostNameRequestor::Start(const DnsName& local_host_full_name) {
  // Note that |host_full_name_| is the name we're trying to resolve, not the
  // name of the local host, which is the parameter to this method.

  local_host_full_name_ = local_host_full_name;

  MdnsAgent::Start(local_host_full_name);

  if (include_local_ && host_full_name_ == local_host_full_name_) {
    // Respond with local addresses.
    OnLocalHostAddressesChanged();
    return;
  }

  SendQuestion(std::make_shared<DnsQuestion>(host_full_name_, DnsType::kA),
               ReplyAddress::Multicast(media_, ip_versions_));
  SendQuestion(std::make_shared<DnsQuestion>(host_full_name_, DnsType::kAaaa),
               ReplyAddress::Multicast(media_, ip_versions_));
}

void HostNameRequestor::ReceiveResource(const DnsResource& resource, MdnsResourceSection section,
                                        ReplyAddress sender_address) {
  if (!sender_address.Matches(media_) || !sender_address.Matches(ip_versions_) ||
      resource.name_ != host_full_name_) {
    return;
  }

  if (include_local_ && host_full_name_ == local_host_full_name_) {
    // Local host is handled in |OnLocalHostAddressesChanged|.
    return;
  }

  HostAddress address;
  if (resource.type_ == DnsType::kA) {
    address = HostAddress(resource.a_.address_.address_, sender_address.interface_id(),
                          zx::sec(resource.time_to_live_));
  } else if (resource.type_ == DnsType::kAaaa) {
    address = HostAddress(resource.aaaa_.address_.address_, sender_address.interface_id(),
                          zx::sec(resource.time_to_live_));
  } else {
    // Not an address resource.
    return;
  }

  if (resource.time_to_live_ == 0) {
    if (addresses_.erase(address)) {
      addresses_dirty_ = true;
    }

    return;
  }

  if (resource.cache_flush_) {
    if (resource.type_ == DnsType::kA) {
      addresses_.EraseV4AddressesOlderThanOneSecond();
    } else {
      addresses_.EraseV6AddressesOlderThanOneSecond();
    }
  }

  // Add an address.
  if (addresses_.insert(address)) {
    addresses_dirty_ = true;
  }

  Renew(resource, media_, ip_versions_);
}

void HostNameRequestor::EndOfMessage() { MaybeSendAddressesChanged(); }

void HostNameRequestor::OnAddProxyHost(const DnsName& host_full_name,
                                       const std::vector<HostAddress>& addresses) {
  if (!include_local_proxies_ || host_full_name != host_full_name_) {
    return;
  }

  if (addresses_.Replace(addresses)) {
    addresses_dirty_ = true;
    MaybeSendAddressesChanged();
  }
}

void HostNameRequestor::OnRemoveProxyHost(const DnsName& host_full_name) {
  if (!include_local_proxies_ || host_full_name != host_full_name_) {
    return;
  }

  if (!addresses_.empty()) {
    addresses_.clear();
    addresses_dirty_ = true;
    MaybeSendAddressesChanged();
  }
}

void HostNameRequestor::OnLocalHostAddressesChanged() {
  if (!include_local_ || host_full_name_ != local_host_full_name_) {
    return;
  }

  if (addresses_.Replace(local_host_addresses())) {
    addresses_dirty_ = true;
    MaybeSendAddressesChanged();
  }
}

void HostNameRequestor::MaybeSendAddressesChanged() {
  if (!addresses_dirty_) {
    return;
  }

  // |AddressesChanged| may delete the subscriber synchronously (removing it from
  // |subscribers_|), so iterate over a snapshot to keep the loop iterator valid.
  std::vector<Mdns::HostNameSubscriber*> subscribers(subscribers_.begin(), subscribers_.end());
  for (auto subscriber : subscribers) {
    subscriber->AddressesChanged(addresses_.Addresses());
  }

  addresses_dirty_ = false;
}

/////////////////////////////////////////////////////////////////////////////////////////////////
// HostAddressSet implementation.

bool HostNameRequestor::HostAddressSet::Replace(const std::vector<HostAddress>& from) {
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

  if (result) {
    insert_times_by_address_.clear();
    std::transform(from.begin(), from.end(),
                   std::inserter(insert_times_by_address_, insert_times_by_address_.begin()),
                   [](auto& address) { return std::pair(address, zx::time()); });
  }

  return result;
}

void HostNameRequestor::HostAddressSet::EraseV4AddressesOlderThanOneSecond() {
  auto one_second_ago = zx::clock::get_monotonic() - zx::sec(1);

  for (auto iter = insert_times_by_address_.begin(); iter != insert_times_by_address_.end();) {
    if (iter->first.address().is_v4() && iter->second < one_second_ago) {
      iter = insert_times_by_address_.erase(iter);
    } else {
      ++iter;
    }
  }
}

// Erases all IPv6 addresses that are older than one second.
void HostNameRequestor::HostAddressSet::EraseV6AddressesOlderThanOneSecond() {
  auto one_second_ago = zx::clock::get_monotonic() - zx::sec(1);

  for (auto iter = insert_times_by_address_.begin(); iter != insert_times_by_address_.end();) {
    if (iter->first.address().is_v6() && iter->second < one_second_ago) {
      iter = insert_times_by_address_.erase(iter);
    } else {
      ++iter;
    }
  }
}

std::vector<HostAddress> HostNameRequestor::HostAddressSet::Addresses() {
  std::vector<HostAddress> result;
  std::transform(insert_times_by_address_.begin(), insert_times_by_address_.end(),
                 std::back_inserter(result), [](auto& pair) { return pair.first; });
  return result;
}

}  // namespace mdns
