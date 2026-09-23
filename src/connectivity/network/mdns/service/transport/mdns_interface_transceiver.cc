// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/transport/mdns_interface_transceiver.h"

#include <arpa/inet.h>
#include <errno.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <net/if.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/socket.h>

#include <algorithm>
#include <cstring>
#include <iostream>
#include <iterator>

#include <fbl/unique_fd.h>

#include "src/connectivity/network/mdns/service/common/formatters.h"
#include "src/connectivity/network/mdns/service/common/mdns_addresses.h"
#include "src/connectivity/network/mdns/service/encoding/dns_formatting.h"
#include "src/connectivity/network/mdns/service/encoding/dns_reading.h"
#include "src/connectivity/network/mdns/service/encoding/dns_writing.h"
#include "src/connectivity/network/mdns/service/transport/mdns_interface_transceiver_v4.h"
#include "src/connectivity/network/mdns/service/transport/mdns_interface_transceiver_v6.h"
#include "src/lib/fostr/hex_dump.h"

namespace mdns {

bool IpSubnet::Contains(const inet::IpAddress& address) const {
  if (!address_.is_valid() || !address.is_valid()) {
    return false;
  }

  inet::IpAddress subnet_addr =
      address_.is_mapped_from_v4() ? address_.mapped_v4_address() : address_;
  inet::IpAddress target_addr = address.is_mapped_from_v4() ? address.mapped_v4_address() : address;

  if (subnet_addr.family() != target_addr.family()) {
    return false;
  }

  size_t total_bytes = subnet_addr.byte_count();
  size_t max_prefix_len = total_bytes * 8;
  size_t effective_prefix_len = std::min(static_cast<size_t>(prefix_len_), max_prefix_len);

  size_t full_bytes = effective_prefix_len / 8;
  size_t remaining_bits = effective_prefix_len % 8;

  const uint8_t* b1 = subnet_addr.as_bytes();
  const uint8_t* b2 = target_addr.as_bytes();

  if (full_bytes > 0 && std::memcmp(b1, b2, full_bytes) != 0) {
    return false;
  }

  if (remaining_bits > 0) {
    uint8_t mask = static_cast<uint8_t>(0xFF << (8 - remaining_bits));
    if ((b1[full_bytes] & mask) != (b2[full_bytes] & mask)) {
      return false;
    }
  }

  return true;
}

// static
std::unique_ptr<MdnsInterfaceTransceiver> MdnsInterfaceTransceiver::Create(inet::IpAddress address,
                                                                           const std::string& name,
                                                                           uint32_t id,
                                                                           Media media) {
  if (address.is_v4()) {
    return std::make_unique<MdnsInterfaceTransceiverV4>(address, name, id, media);
  } else {
    return std::make_unique<MdnsInterfaceTransceiverV6>(address, name, id, media);
  }
}

MdnsInterfaceTransceiver::MdnsInterfaceTransceiver(inet::IpAddress address, const std::string& name,
                                                   uint32_t id, Media media)
    : address_(address),
      name_(name),
      id_(id),
      media_(media),
      inbound_buffer_(kMaxPacketSize),
      outbound_buffer_(kMaxPacketSize) {
  FX_DCHECK(media_ == Media::kWired || media_ == Media::kWireless);
}

MdnsInterfaceTransceiver::~MdnsInterfaceTransceiver() {}

bool MdnsInterfaceTransceiver::Start(InboundMessageCallback callback) {
  FX_DCHECK(callback);
  FX_DCHECK(!socket_fd_.is_valid()) << "Start called when already started.";

  FX_LOGS(INFO) << "Starting mDNS on interface " << name_ << " using port "
                << MdnsAddresses::port();

  socket_fd_ = fbl::unique_fd(socket(address_.family(), SOCK_DGRAM, 0));

  if (!socket_fd_.is_valid()) {
    FX_LOGS(ERROR) << "Failed to open socket, " << strerror(errno);
    return false;
  }

  // Set socket options and bind.
  if (SetOptionShareAddress() != 0 || SetOptionSharePort() != 0 ||
      SetOptionDisableMulticastLoop() != 0 || SetOptionJoinMulticastGroup() != 0 ||
      SetOptionOutboundInterface() != 0 || SetOptionUnicastTtl() != 0 ||
      SetOptionMulticastTtl() != 0 || SetOptionFamilySpecific() != 0 ||
      SetOptionBindToDevice() != 0 || Bind() != 0) {
    socket_fd_.reset();
    return false;
  }

  inbound_message_callback_ = std::move(callback);

  WaitForInbound();
  return true;
}

void MdnsInterfaceTransceiver::Stop() {
  FX_DCHECK(socket_fd_.is_valid()) << "Stop called when stopped.";
  fd_waiter_.Cancel();
  socket_fd_.reset();
}

void MdnsInterfaceTransceiver::SetInterfaceAddresses(
    const std::vector<IpSubnet>& interface_addresses) {
  FX_DCHECK(!interface_addresses.empty());

  interface_addresses_ = interface_addresses;

  // These resources are a cached version of |interface_addresses_|. Make sure they get regenerated.
  interface_address_resources_.clear();
}

bool MdnsInterfaceTransceiver::IsOnLocalSubnet(const inet::IpAddress& address) const {
  for (const auto& subnet : interface_addresses_) {
    if (subnet.Contains(address)) {
      return true;
    }
  }

  return false;
}

void MdnsInterfaceTransceiver::SendMessage(const DnsMessage& message,
                                           const inet::SocketAddress& address) {
  FX_DCHECK(address.is_valid());
  FX_DCHECK(address.family() == address_.family() || address == MdnsAddresses::v4_multicast());

  DnsMessage fixed_up_message;
  fixed_up_message.header_ = message.header_;
  fixed_up_message.questions_ = message.questions_;
  fixed_up_message.answers_ = FixUpAddresses(message.answers_);
  fixed_up_message.authorities_ = FixUpAddresses(message.authorities_);
  fixed_up_message.additionals_ = FixUpAddresses(message.additionals_);
  fixed_up_message.UpdateCounts();

  PacketWriter writer(std::move(outbound_buffer_));
  writer << fixed_up_message;
  size_t packet_size = writer.position();
  outbound_buffer_ = writer.GetPacket();

  ssize_t result = SendTo(outbound_buffer_.data(), packet_size, address);

  ++messages_sent_;
  bytes_sent_ += packet_size;

  // Host down errors are expected. See https://fxbug.dev/42140430.
  if (result < 0 && errno != EHOSTDOWN && errno != ENETUNREACH) {
    FX_LOGS(ERROR) << "Failed to sendto " << address << " from " << name_ << " (" << address_
                   << "), size " << packet_size << ", " << strerror(errno);
  }
}

void MdnsInterfaceTransceiver::SendAddress(const DnsName& host_full_name) {
  DnsMessage message;
  message.answers_.push_back(GetAddressResource(host_full_name));

  SendMessage(message, MdnsAddresses::v4_multicast());
}

void MdnsInterfaceTransceiver::SendAddressGoodbye(const DnsName& host_full_name) {
  DnsMessage message;
  // Not using |GetAddressResource| here, because we want to modify the ttl.
  message.answers_.push_back(std::make_shared<DnsResource>(host_full_name, address_));
  message.answers_.back()->time_to_live_ = 0;

  SendMessage(message, MdnsAddresses::v4_multicast());
}

void MdnsInterfaceTransceiver::LogTraffic() {
  std::cout << "interface " << name_ << " " << address_ << "\n";
  std::cout << "    messages received:  " << messages_received_ << "\n";
  std::cout << "    bytes received:     " << bytes_received_ << "\n";
  std::cout << "    messages sent:      " << messages_sent_ << "\n";
  std::cout << "    bytes sent:         " << bytes_sent_ << "\n";
}

int MdnsInterfaceTransceiver::SetOptionBindToDevice() {
  char ifname[IF_NAMESIZE];
  uint32_t id = this->id();
  if (if_indextoname(id, ifname) == nullptr) {
    FX_LOGS(ERROR) << "Failed to look up interface name with index=" << id << ", error "
                   << strerror(errno);
  }
  int result = setsockopt(socket_fd_.get(), SOL_SOCKET, SO_BINDTODEVICE, &ifname,
                          static_cast<socklen_t>(strnlen(ifname, IF_NAMESIZE)));
  if (result < 0) {
    FX_LOGS(ERROR) << "Failed to set socket option SO_BINDTODEVICE with ifname=" << ifname
                   << ", error" << strerror(errno);
  }
  return result;
}

int MdnsInterfaceTransceiver::SetOptionShareAddress() {
  int param = 1;
  int result = setsockopt(socket_fd_.get(), SOL_SOCKET, SO_REUSEADDR, &param, sizeof(param));
  if (result < 0) {
    FX_LOGS(ERROR) << "Failed to set socket option SO_REUSEADDR, " << strerror(errno);
  }

  return result;
}

int MdnsInterfaceTransceiver::SetOptionSharePort() {
  int param = 1;
  int result = setsockopt(socket_fd_.get(), SOL_SOCKET, SO_REUSEPORT, &param, sizeof(param));
  if (result < 0) {
    FX_LOGS(ERROR) << "Failed to set socket option SO_REUSEPORT, " << strerror(errno);
  }

  return result;
}

void MdnsInterfaceTransceiver::WaitForInbound() {
  fd_waiter_.Wait([this](zx_status_t status, uint32_t events) { InboundReady(status, events); },
                  socket_fd_.get(), POLLIN);
}

void MdnsInterfaceTransceiver::InboundReady(zx_status_t status, uint32_t events) {
  sockaddr_storage source_address_storage;
  socklen_t source_address_length = address_.is_v4() ? sizeof(sockaddr_in) : sizeof(sockaddr_in6);

  iovec iov;
  iov.iov_base = inbound_buffer_.data();
  iov.iov_len = inbound_buffer_.size();

  // Control message buffer size.
  // We need enough space for either in_pktinfo (v4) or in6_pktinfo (v6).
  alignas(struct cmsghdr) char
      control_buffer[CMSG_SPACE(std::max(sizeof(in_pktinfo), sizeof(in6_pktinfo)))];

  msghdr msg = {};
  msg.msg_name = &source_address_storage;
  msg.msg_namelen = source_address_length;
  msg.msg_iov = &iov;
  msg.msg_iovlen = 1;
  msg.msg_control = control_buffer;
  msg.msg_controllen = sizeof(control_buffer);

  ssize_t result = ReceiveMessage(socket_fd_.get(), &msg, 0);
  if (result < 0) {
    FX_LOGS(ERROR) << "Failed to recvmsg, " << strerror(errno);
    // Wait a bit before trying again to avoid spamming the log.
    async::PostDelayedTask(
        async_get_default_dispatcher(), [this]() { WaitForInbound(); }, zx::sec(10));
    return;
  }

  ++messages_received_;
  bytes_received_ += result;

  inet::IpAddress destination_address;
  for (cmsghdr* cmsg = CMSG_FIRSTHDR(&msg); cmsg != nullptr; cmsg = CMSG_NXTHDR(&msg, cmsg)) {
    if (cmsg->cmsg_level == IPPROTO_IP && cmsg->cmsg_type == IP_PKTINFO) {
      auto* pktinfo = reinterpret_cast<in_pktinfo*>(CMSG_DATA(cmsg));
      destination_address = inet::IpAddress(pktinfo->ipi_addr);
    } else if (cmsg->cmsg_level == IPPROTO_IPV6 && cmsg->cmsg_type == IPV6_PKTINFO) {
      auto* pktinfo = reinterpret_cast<in6_pktinfo*>(CMSG_DATA(cmsg));
      destination_address = inet::IpAddress(pktinfo->ipi6_addr);
    }
  }

  ReplyAddress reply_address(source_address_storage, address_, id_, media_, IpVersions());

  if (reply_address.socket_address().address() == address_) {
    // This is an outgoing message that's bounced back to us. Drop it.
    WaitForInbound();
    return;
  }

  // The following logic prevents attacks originating from outside the local segment
  // per https://datatracker.ietf.org/doc/html/rfc6762#section-11.
  bool force_multicast_response = false;
  bool is_from_local_subnet = IsOnLocalSubnet(reply_address.socket_address().address());
  if (destination_address == MdnsAddresses::v4_multicast().address() ||
      destination_address == MdnsAddresses::v6_multicast().address()) {
    // This message was sent to a multicast address. If it originated from outside the
    // local segment, we will force multicast responses so we don't send unicast messages
    // outside the local segment.
    force_multicast_response = !is_from_local_subnet;
  } else if (!is_from_local_subnet) {
    // This message was sent to a unicast address from somewhere outside the local segment.
    // Drop it.
    WaitForInbound();
    return;
  }

  PacketReader reader(inbound_buffer_);
  reader.SetBytesRemaining(static_cast<size_t>(result));
  std::unique_ptr<DnsMessage> message = std::make_unique<DnsMessage>();
  reader >> *message;

  if (reader.complete()) {
    if (force_multicast_response) {
      for (auto& question : message->questions_) {
        question->unicast_response_ = false;
      }
    }

    FX_DCHECK(inbound_message_callback_);
    inbound_message_callback_(std::move(message), reply_address);
  } else {
#ifdef MDNS_TRACE
    FX_LOGS(WARNING) << "Couldn't parse message from " << reply_address << ", " << result
                     << " bytes: " << fostr::HexDump(inbound_buffer_.data(), result, 0);
#else
    FX_LOGS(WARNING) << "Couldn't parse message from " << reply_address << ", " << result
                     << " bytes";
#endif  // MDNS_TRACE
  }

  WaitForInbound();
}

ssize_t MdnsInterfaceTransceiver::ReceiveMessage(int sockfd, struct msghdr* msg, int flags) {
  return recvmsg(sockfd, msg, flags);
}

std::shared_ptr<DnsResource> MdnsInterfaceTransceiver::GetAddressResource(
    const DnsName& host_full_name) {
  FX_DCHECK(address_.is_valid());

  if (!address_resource_ || address_resource_->name_ != host_full_name) {
    address_resource_ = std::make_shared<DnsResource>(host_full_name, address_);
  }

  return address_resource_;
}

const std::vector<std::shared_ptr<DnsResource>>&
MdnsInterfaceTransceiver::GetInterfaceAddressResources(const DnsName& host_full_name) {
  FX_DCHECK(!interface_addresses_.empty());

  // Generate new resources if there currently are none or if the host name has changed.
  if (interface_address_resources_.empty() ||
      interface_address_resources_[0]->name_ != host_full_name) {
    interface_address_resources_.clear();

    // We need to generate new address resources for this interface. An A/AAAA resource
    // is generated for each V4/V6 address in the |interface_addresses_| collection. The first
    // A resource and the first AAAA resource should have the cache_flush bit and other resources
    // should not.
    bool v4_cache_flush = true;
    bool v6_cache_flush = true;
    std::transform(interface_addresses_.begin(), interface_addresses_.end(),
                   std::back_inserter(interface_address_resources_),
                   [&host_full_name, &v4_cache_flush, &v6_cache_flush](const IpSubnet& ip_subnet) {
                     bool cache_flush;
                     if (ip_subnet.address().is_v4()) {
                       // Set cache_flush on the first A resource but not subsequent ones.
                       cache_flush = v4_cache_flush;
                       v4_cache_flush = false;
                     } else {
                       // Set cache_flush on the first AAAA resource but not subsequent ones.
                       cache_flush = v6_cache_flush;
                       v6_cache_flush = false;
                     }
                     return std::make_shared<DnsResource>(host_full_name, ip_subnet.address(),
                                                          cache_flush);
                   });
  }

  return interface_address_resources_;
}

std::vector<std::shared_ptr<DnsResource>> MdnsInterfaceTransceiver::FixUpAddresses(
    const std::vector<std::shared_ptr<DnsResource>>& resources) {
  std::vector<DnsName> placeholder_names;
  std::vector<std::shared_ptr<DnsResource>> result;
  std::copy_if(resources.begin(), resources.end(), std::back_inserter(result),
               [&placeholder_names](const std::shared_ptr<DnsResource>& resource) {
                 switch (resource->type_) {
                   case DnsType::kA:
                     if (resource->a_.address_.address_.is_valid()) {
                       // Not a placeholder.
                       return true;
                     }
                     break;
                   case DnsType::kAaaa:
                     if (resource->aaaa_.address_.address_.is_valid()) {
                       // Not a placeholder.
                       return true;
                     }
                     break;
                   default:
                     // Not an address.
                     return true;
                 }

                 if (std::find(placeholder_names.begin(), placeholder_names.end(),
                               resource->name_) == placeholder_names.end()) {
                   placeholder_names.push_back(resource->name_);
                 }

                 return false;
               });

  for (const auto& name : placeholder_names) {
    auto& addr_resources = GetInterfaceAddressResources(name);
    std::copy(addr_resources.begin(), addr_resources.end(), std::back_inserter(result));
  }

  return result;
}

}  // namespace mdns
