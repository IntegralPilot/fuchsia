// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/common/service_instance.h"

namespace mdns {

// static
std::unique_ptr<ServiceInstance> ServiceInstance::Create(
    const std::string& service_name, const std::string& instance_name,
    const std::string& target_name, const std::vector<inet::SocketAddress>& addresses,
    const std::vector<std::vector<uint8_t>>& text, uint16_t srv_priority, uint16_t srv_weight) {
  return Create(DnsName(service_name), DnsLabel(instance_name), DnsName(target_name), addresses,
                text, srv_priority, srv_weight);
}

ServiceInstance::ServiceInstance(const std::string& service_name, const std::string& instance_name,
                                 const std::string& target_name,
                                 const std::vector<inet::SocketAddress>& addresses,
                                 const std::vector<std::vector<uint8_t>>& text,
                                 uint16_t srv_priority, uint16_t srv_weight)
    : ServiceInstance(DnsName(service_name), DnsLabel(instance_name), DnsName(target_name),
                      addresses, text, srv_priority, srv_weight) {}

// static
std::unique_ptr<ServiceInstance> ServiceInstance::Create(
    DnsName service_name, DnsLabel instance_name, DnsName target_name,
    const std::vector<inet::SocketAddress>& addresses,
    const std::vector<std::vector<uint8_t>>& text, uint16_t srv_priority, uint16_t srv_weight) {
  return std::make_unique<ServiceInstance>(std::move(service_name), std::move(instance_name),
                                           std::move(target_name), addresses, text, srv_priority,
                                           srv_weight);
}

ServiceInstance::ServiceInstance(DnsName service_name, DnsLabel instance_name, DnsName target_name,
                                 const std::vector<inet::SocketAddress>& addresses,
                                 const std::vector<std::vector<uint8_t>>& text,
                                 uint16_t srv_priority, uint16_t srv_weight)
    : service_name_(std::move(service_name)),
      instance_name_(std::move(instance_name)),
      target_name_(std::move(target_name)),
      addresses_(addresses),
      text_(text),
      srv_priority_(srv_priority),
      srv_weight_(srv_weight) {}

std::unique_ptr<ServiceInstance> ServiceInstance::Clone() const {
  return Create(service_name_, instance_name_, target_name_, addresses_, text_, srv_priority_,
                srv_weight_);
}

bool ServiceInstance::operator==(const ServiceInstance& other) const {
  return service_name_ == other.service_name_ && instance_name_ == other.instance_name_ &&
         target_name_ == other.target_name_ && addresses_ == other.addresses_ &&
         text_ == other.text_ && srv_priority_ == other.srv_priority_ &&
         srv_weight_ == other.srv_weight_;
}

}  // namespace mdns
