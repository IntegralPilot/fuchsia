// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_AGENTS_INSTANCE_REQUESTOR_H_
#define SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_AGENTS_INSTANCE_REQUESTOR_H_

#include <lib/zx/time.h>

#include <algorithm>
#include <memory>
#include <queue>
#include <string>
#include <unordered_map>
#include <unordered_set>

#include "src/connectivity/network/mdns/service/agents/mdns_agent.h"
#include "src/connectivity/network/mdns/service/mdns.h"
#include "src/lib/inet/socket_address.h"

namespace mdns {

// Searches for instances of a service type.
class InstanceRequestor : public MdnsAgent {
 public:
  // Creates an |InstanceRequestor|.
  InstanceRequestor(MdnsAgent::Owner* owner, const DnsName& service_name, Media media,
                    IpVersions ip_versions, bool include_local, bool include_local_proxies);

  // Creates an |InstanceRequestor| requesting all services.
  InstanceRequestor(MdnsAgent::Owner* owner, Media media, IpVersions ip_versions,
                    bool include_local, bool include_local_proxies);

  ~InstanceRequestor() override = default;

  // Adds the subscriber.
  void AddSubscriber(Mdns::Subscriber* subscriber);

  // Removes the subscriber. If it's the last subscriber, this
  // |InstanceRequestor| is destroyed.
  void RemoveSubscriber(Mdns::Subscriber* subscriber);

  // MdnsAgent overrides.
  void Start(const DnsName& local_host_full_name) override;

  void ReceiveResource(const DnsResource& resource, MdnsResourceSection section,
                       ReplyAddress sender_address) override;

  void EndOfMessage() override;

  void OnAddLocalServiceInstance(const Mdns::ServiceInstance& instance, bool from_proxy) override;

  void OnChangeLocalServiceInstance(const Mdns::ServiceInstance& instance,
                                    bool from_proxy) override;

  void OnRemoveLocalServiceInstance(const DnsName& service_name, const DnsLabel& instance_name,
                                    bool from_proxy) override;

 private:
  // Set of socket addresses associated with a target.
  class TargetAddressSet {
   public:
    TargetAddressSet() = default;

    ~TargetAddressSet() = default;

    // Indicates whether the set is empty.
    bool empty() const { return insert_times_by_address_.empty(); }

    // Erases |address| from the set. Returns true if the address was in the set, false if not.
    bool erase(inet::SocketAddress address) { return insert_times_by_address_.erase(address); }

    // Inserts |address| into the set if it isn't already there. The age of the address is zeroed
    // regardless. Returns true if the address was added, false if it was already there.
    bool insert(inet::SocketAddress address) {
      auto now = zx::clock::get_monotonic();

      auto [iter, inserted] = insert_times_by_address_.emplace(address, now);
      if (!inserted) {
        iter->second = now;
      }

      return inserted;
    }

    // Replaces all addresses in the set with |from| and returns true, unless this set already
    // contains exactly the addresses in |from|, in which case changes nothing and returns false.
    // This is used only for local instances and does not assign meaningful insert times.
    bool Replace(const std::vector<inet::SocketAddress>& from);

    // Erases all IPv4 addresses that are older than one second.
    void EraseV4AddressesOlderThanOneSecond();

    // Erases all IPv6 addresses that are older than one second.
    void EraseV6AddressesOlderThanOneSecond();

    // Returns a list of the contained addresses each with its port number replaced with |port|.
    std::vector<inet::SocketAddress> AddressesWithPort(inet::IpPort port);

   private:
    std::unordered_map<inet::SocketAddress, zx::time> insert_times_by_address_;
  };

  // Describes a service instance.
  struct InstanceInfo {
    DnsName service_name_;
    DnsLabel instance_name_;
    DnsName target_;
    DnsName target_full_name_;
    inet::IpPort port_;
    std::vector<std::vector<uint8_t>> text_;
    uint16_t srv_priority_ = 0;
    uint16_t srv_weight_ = 0;
    bool new_ = true;
    bool dirty_ = true;
    bool srv_queried_ = false;
    bool txt_received_ = false;
    bool txt_queried_ = false;
    bool is_local_ = false;
  };

  // Describes a target. We use |inet::SocketAddress| because it has scope_id. The port numbers
  // aren't used. |InstanceInfo| provides the service instance's port number.
  struct TargetInfo {
    TargetAddressSet addresses_;
    bool keep_ = false;
    bool dirty_ = false;
    bool addresses_queried_ = false;
    bool is_local_ = false;
  };

  // Report all known instances to the indicated subscriber.
  void ReportAllDiscoveries(Mdns::Subscriber* subscriber);

  // Sends a query for instances and schedules the next query, as appropriate.
  void SendQuery();

  void ReceivePtrResource(const DnsResource& resource, MdnsResourceSection section);

  void ReceiveSrvResource(const DnsResource& resource, MdnsResourceSection section,
                          InstanceInfo* instance_info);

  void ReceiveTxtResource(const DnsResource& resource, MdnsResourceSection section,
                          InstanceInfo* instance_info);

  void ReceiveAResource(const DnsResource& resource, MdnsResourceSection section,
                        TargetInfo* target_info, uint32_t scope_id);

  void ReceiveAaaaResource(const DnsResource& resource, MdnsResourceSection section,
                           TargetInfo* target_info, uint32_t scope_id);

  void RemoveInstance(const DnsName& instance_full_name);

  void RemoveLocalInstance(const DnsName& instance_full_name);

  DnsName service_name_;
  DnsName service_full_name_;
  Media media_;
  IpVersions ip_versions_;
  bool include_local_;
  bool include_local_proxies_;
  std::unordered_set<Mdns::Subscriber*> subscribers_;
  std::unordered_map<DnsName, InstanceInfo> instance_infos_by_full_name_;
  std::unordered_map<DnsName, TargetInfo> target_infos_by_full_name_;
  zx::duration query_delay_;
  std::shared_ptr<DnsQuestion> question_;

 public:
  // Disallow copy, assign and move.
  InstanceRequestor(const InstanceRequestor&) = delete;
  InstanceRequestor(InstanceRequestor&&) = delete;
  InstanceRequestor& operator=(const InstanceRequestor&) = delete;
  InstanceRequestor& operator=(InstanceRequestor&&) = delete;
};

}  // namespace mdns

#endif  // SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_AGENTS_INSTANCE_REQUESTOR_H_
