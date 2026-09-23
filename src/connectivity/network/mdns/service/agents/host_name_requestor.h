// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_AGENTS_HOST_NAME_REQUESTOR_H_
#define SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_AGENTS_HOST_NAME_REQUESTOR_H_

#include <lib/zx/time.h>

#include <memory>
#include <string>
#include <unordered_set>

#include "src/connectivity/network/mdns/service/agents/mdns_agent.h"
#include "src/connectivity/network/mdns/service/mdns.h"
#include "src/lib/inet/ip_address.h"

namespace mdns {

// Requests host name resolution.
class HostNameRequestor : public MdnsAgent {
 public:
  // Creates a |HostNameRequestor|.
  HostNameRequestor(MdnsAgent::Owner* owner, const DnsName& host_name, Media media,
                    IpVersions ip_versions, bool include_local, bool include_local_proxies);

  ~HostNameRequestor() override;

  // Adds the subscriber.
  void AddSubscriber(Mdns::HostNameSubscriber* subscriber);

  // Removes the subscriber. If it's the last subscriber, this |HostNameRequestor| is destroyed.
  void RemoveSubscriber(Mdns::HostNameSubscriber* subscriber);

  // MdnsAgent overrides.
  void Start(const DnsName& local_host_full_name) override;

  void ReceiveResource(const DnsResource& resource, MdnsResourceSection section,
                       ReplyAddress sender_address) override;

  void EndOfMessage() override;

  void OnAddProxyHost(const DnsName& host_full_name,
                      const std::vector<HostAddress>& addresses) override;

  void OnRemoveProxyHost(const DnsName& host_full_name) override;

  void OnLocalHostAddressesChanged() override;

 private:
  // Set of |HostAddress|es associated with a host.
  class HostAddressSet {
   public:
    HostAddressSet() = default;

    ~HostAddressSet() = default;

    bool operator==(const HostAddressSet& other) {
      return insert_times_by_address_ == other.insert_times_by_address_;
    }

    bool operator!=(const HostAddressSet& other) { return !(*this == other); }

    // Indicates whether the set is empty.
    bool empty() const { return insert_times_by_address_.empty(); }

    // Returns the number of .
    bool contains(HostAddress address) const {
      return insert_times_by_address_.find(address) != insert_times_by_address_.end();
    }

    // Erases |address| from the set. Returns true if the address was in the set, false if not.
    bool erase(HostAddress address) { return insert_times_by_address_.erase(address); }

    // Inserts |address| into the set if it isn't already there. The age of the address is zeroed
    // regardless. Returns true if the address was added, false if it was already there.
    bool insert(HostAddress address) {
      auto now = zx::clock::get_monotonic();

      auto [iter, inserted] = insert_times_by_address_.emplace(address, now);
      if (!inserted) {
        iter->second = now;
      }

      return inserted;
    }

    // Clears this collection.
    void clear() { insert_times_by_address_.clear(); }

    // Replaces all addresses in the set with |from| and returns true, unless this set already
    // contains exactly the addresses in |from|, in which case changes nothing and returns false.
    // This is used only for local instances and does not assign meaningful insert times.
    bool Replace(const std::vector<HostAddress>& from);

    // Erases all IPv4 addresses that are older than one second.
    void EraseV4AddressesOlderThanOneSecond();

    // Erases all IPv6 addresses that are older than one second.
    void EraseV6AddressesOlderThanOneSecond();

    // Returns the contained addresses as a vector.
    std::vector<HostAddress> Addresses();

   private:
    struct HostAddressHash {
      std::size_t operator()(const HostAddress& host_address) const noexcept {
        return std::hash<inet::IpAddress>{}(host_address.address()) ^
               (std::hash<uint32_t>{}(host_address.interface_id()) << 1);
      }
    };

    struct HostAddressesEqual {
      bool operator()(const HostAddress& lhs, const HostAddress& rhs) const noexcept {
        return lhs.address() == rhs.address() && lhs.interface_id() == rhs.interface_id();
      }
    };

    std::unordered_map<HostAddress, zx::time, HostAddressHash, HostAddressesEqual>
        insert_times_by_address_;
  };

  // Sends an |AddressesChanged| notification to the subscribers and clear |addresses_dirty_| if
  // |addresses_dirty_| is set.
  void MaybeSendAddressesChanged();

  DnsName host_name_;
  DnsName host_full_name_;
  Media media_;
  IpVersions ip_versions_;
  bool include_local_;
  bool include_local_proxies_;
  DnsName local_host_full_name_;
  HostAddressSet addresses_;
  bool addresses_dirty_ = false;
  std::unordered_set<Mdns::HostNameSubscriber*> subscribers_;

 public:
  // Disallow copy, assign and move.
  HostNameRequestor(const HostNameRequestor&) = delete;
  HostNameRequestor(HostNameRequestor&&) = delete;
  HostNameRequestor& operator=(const HostNameRequestor&) = delete;
  HostNameRequestor& operator=(HostNameRequestor&&) = delete;
};

}  // namespace mdns

#endif  // SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_AGENTS_HOST_NAME_REQUESTOR_H_
