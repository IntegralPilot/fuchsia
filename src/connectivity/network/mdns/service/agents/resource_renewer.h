// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_AGENTS_RESOURCE_RENEWER_H_
#define SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_AGENTS_RESOURCE_RENEWER_H_

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>

#include <memory>
#include <queue>
#include <string>
#include <unordered_set>
#include <vector>

#include "src/connectivity/network/mdns/service/agents/mdns_agent.h"
#include "src/connectivity/network/mdns/service/common/types.h"
#include "src/lib/inet/ip_port.h"

namespace mdns {

// |ResourceRenewer| requests resources by sending queries repeatedly. This capability can be used
// to renew a received resource (using the |Renew| method) or for general querying (using the
// |Query| method). When renewing a received resource, queries are sent at 80%, 85%, 90% and 95%
// of the resource's TTL.
//
// If a resource is received, the renewer forgets about the resource until asked again to renew it.
// If the resource is not received after the complete query sequence, |ResourceRenewer| sends a
// resource record to all the agents with a TTL of zero, signalling that the resource should be
// deleted and forgets about the resource. If a resource is explicitly deleted (a resource
// record arrives with TTL 0), |ResourceRenewer| will stop querying for it.
//
// Agents that need a resource record renewed call |Renew| on the host, which
// then calls |Renew| on the |ResourceRenewer|. Agents must continue to renew
// incoming resources as long as they want renewals to occur. When an agent
// loses interest in a record, it should simply stop renewing the incoming
// resource records. This approach will cause some unneeded renewals, but avoids
// difficult cleanup issues associated with a persistent renewal scheme.
class ResourceRenewer : public MdnsAgent {
 public:
  explicit ResourceRenewer(MdnsAgent::Owner* owner);

  ~ResourceRenewer() override;

  // Attempts to renew |resource| before its TTL expires.
  void Renew(const DnsResource& resource, Media media, IpVersions ip_versions);

  // Queries for the indicated resource with the specified schedule.
  void Query(DnsType type, const DnsName& name, Media media, IpVersions ip_versions,
             zx::time initial_query_time, zx::duration interval, uint32_t interval_multiplier,
             uint32_t max_queries, bool request_unicast_response = false);

  // MdnsAgent overrides.
  void ReceiveResource(const DnsResource& resource, MdnsResourceSection section,
                       ReplyAddress sender_address) override;

  void Quit() override;

 private:
  static constexpr uint32_t kFirstQueryPerThousand = 800;
  static constexpr uint32_t kQueryIntervalPerThousand = 50;
  static constexpr uint32_t kQueriesToAttempt = 4;
  static constexpr zx::duration kFudgeFactor = zx::msec(1);

  // All Entry objects are represented in both |entries_| and |schedule_|. We're
  // using raw pointers, so the destructor must delete all Entry objects
  // explicitly.
  struct Entry {
    Entry(DnsName name, DnsType type, Media media, IpVersions ip_versions,
          bool request_unicast_response)
        : name_(std::move(name)),
          type_(type),
          media_(media),
          ip_versions_(ip_versions),
          request_unicast_response_(request_unicast_response) {}

    DnsName name_;
    DnsType type_;
    Media media_;
    IpVersions ip_versions_;

    zx::time time_;
    zx::duration interval_;
    uint32_t interval_multiplier_;
    uint32_t queries_remaining_;
    bool request_unicast_response_ = false;

    // Time value used for |schedule|. In some cases, we want to postpone a
    // query or expiration that was previously scheduled. In this case, |time_|
    // will be increased, but |schedule_time_| will remain unchanged. When the
    // entry comes up in the schedule, the entry should be rescheduled if
    // |time_| is different from |schedule_time_|.
    zx::time schedule_time_;

    bool delete_ = false;

    // Sets |time_|, |interval_|, |interval_multiplier_| and |queries_remaining_| to their initial
    // values.
    void SetFirstQuery(zx::time initial_query_time, zx::duration interval,
                       uint32_t interval_multiplier, uint32_t max_queries);

    // Updates |time_| and |queries_remaining_| for the purposes of scheduling
    // the next query or expiration.
    void SetNextQueryOrExpiration();
  };

  struct Hash {
    size_t operator()(const std::unique_ptr<Entry>& m) const {
      FX_DCHECK(m);
      return std::hash<DnsName>{}(m->name_) ^ std::hash<DnsType>{}(m->type_);
    }
  };

  struct Equals {
    size_t operator()(const std::unique_ptr<Entry>& a, const std::unique_ptr<Entry>& b) const {
      FX_DCHECK(a);
      FX_DCHECK(b);
      return a->name_ == b->name_ && a->type_ == b->type_;
    }
  };

  struct LaterScheduleTime {
    size_t operator()(const Entry* a, const Entry* b) {
      FX_DCHECK(a != nullptr);
      FX_DCHECK(b != nullptr);
      return a->schedule_time_ > b->schedule_time_;
    }
  };

  // Sends current renewals and schedules another call to |SendRenewals|, as
  // appropriate.
  void SendRenewals();

  void Schedule(Entry* entry);

  void EraseEntry(Entry* entry);

  std::unordered_set<std::unique_ptr<Entry>, Hash, Equals> entries_;
  std::priority_queue<Entry*, std::vector<Entry*>, LaterScheduleTime> schedule_;

 public:
  // Disallow copy, assign and move.
  ResourceRenewer(const ResourceRenewer&) = delete;
  ResourceRenewer(ResourceRenewer&&) = delete;
  ResourceRenewer& operator=(const ResourceRenewer&) = delete;
  ResourceRenewer& operator=(ResourceRenewer&&) = delete;
};

}  // namespace mdns

#endif  // SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_AGENTS_RESOURCE_RENEWER_H_
