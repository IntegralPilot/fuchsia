// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_GIGABOOT_CPP_NETWORK_H_
#define SRC_FIRMWARE_GIGABOOT_CPP_NETWORK_H_

#include <arpa/inet.h>
#include <lib/fit/result.h>

#include <array>
#include <cstdint>
#include <numeric>
#include <span>

#include <efi/protocol/managed-network.h>
#include <phys/efi/protocol.h>

#include "utils.h"

namespace gigaboot {

constexpr uint8_t kUdpHdr = 0x11;

// Ethernet MTU: 2 ethernet addresses, ethertype, ipv6 header, udp header, payload
constexpr size_t kEthMTU = 1502;

constexpr size_t kMacAddrLen = 6;
using MacAddr = std::array<uint8_t, kMacAddrLen>;

constexpr size_t kIp6AddrLen = 16;
using Ip6Addr = std::array<uint8_t, kIp6AddrLen>;

// Given a multicast IPv6 address, generate the destination multicast MAC address for
// link-local transmission.
// Defined in RFC 2464.
constexpr MacAddr MulticastMacFromIp6(Ip6Addr const& addr) {
  return MacAddr{
      0x33, 0x33, addr[12], addr[13], addr[14], addr[15],
  };
}

// IPv6 Stateless Address Auto-configuration (SLAAC) allows the definition of
// link-local, deterministic IP addresses without input from other network elements.
// The address must use have the prefix fe80::/64, but the precise generation mechanism
// is deferred to the implementation.
constexpr Ip6Addr LocalIp6AddrFromMac(MacAddr mac_addr) {
  return Ip6Addr{
      0xFE,        0x80,        0,
      0,           0,           0,
      0,           0,           static_cast<uint8_t>(mac_addr[0] ^ 2),
      mac_addr[1], mac_addr[2], 0xFF,
      0xFE,        mac_addr[3], mac_addr[4],
      mac_addr[5],
  };
}

// Calculate the 1s complement checksum on a payload.
uint16_t CalculateChecksum(std::span<const uint8_t> data, uint64_t start);

class Ip6Header {
 public:
  Ip6Header(uint16_t len, uint8_t next_hdr, const Ip6Addr& src, const Ip6Addr& dst);

  // The first four octets of an IPv6 header contain three fields:
  // version (4 bits), always 0b0110, or 6.
  // traffic class (8 bits), used for differentiated services and congestion control.
  // flow label (20 bits), identifies a flow of packets, used for QoS.
  //
  // These fields are combined into a single 32 bit number for simplified serialization.
  // Both traffic class and flow label can be 0.
  uint32_t vtcf;
  uint16_t length;
  uint8_t next_header;
  uint8_t hop_limit;
  Ip6Addr source;
  Ip6Addr dest;
};
static_assert(sizeof(Ip6Header) == 40);

struct UdpHeader {
  uint16_t src_port;
  uint16_t dst_port;
  uint16_t length;
  uint16_t checksum;
};
static_assert(sizeof(UdpHeader) == 8);

class EthernetAgent {
 public:
  // Create and return an EthernetAgent, or an error if construction fails.
  // The EthernetAgent manages transmission and configuration of a single interface
  // via the ManagedNetworkProtocol.
  //
  // Construction may fail for any of the following reasons:
  // *) There are no interfaces
  // *) No interface has connected media
  // *) Failure to determine configuration for an interface
  // *) Failure to configure an interface
  static fit::result<efi_status, EthernetAgent> Create();

  // No default ctor or copy.
  // The whole point of an agent is that it is the sole owner of a managed protocol.
  EthernetAgent() = delete;
  EthernetAgent(const EthernetAgent&) = delete;
  EthernetAgent& operator=(const EthernetAgent&) = delete;

  // We do want move because of the factory function.
  EthernetAgent(EthernetAgent&&) = default;
  EthernetAgent& operator=(EthernetAgent&&) = default;

  constexpr MacAddr Addr() const { return mac_addr_; }

  // Transmit an IPv6 packet.
  //
  // Note: the only reason that this method specifically transmits an IPv6 packet is
  //       because it sets the ethertype to be that of IPv6. All v6 header data
  //       must be set in the payload by the caller.
  //
  // Parameters:
  // dst: the MAC address of the destination.
  //      The address can be unicast, multicast, or broadcast.
  //
  // data: the IPv6 packet to transmit. This must include the v6 header,
  //       any higher protocol headers, and any payload data.
  //
  // callback: a UEFI event to be signaled once the data has been transmitted.
  //           It must be created with a priority of TPL_CALLBACK and must be of type
  //           EVT_NOTIFY_SIGNAL.
  //           The event callback with associated context can be used to e.g. set a timer,
  //           update a counter, or clean up a resource.
  //
  //           See the documentation for EFI_BOOT_SERVICES.CreateEvent for more details
  //           on creating and using signalled events.
  //
  // token: (output) a transmission token that, after transmission is complete,
  //        contains the return status of transmission.
  //        The UEFI spec states that the completion token's status is updated after the
  //        operation completes, and that the operation is asynchronous.
  //        The token is provided by the caller because the caller is better equipped
  //        to guarantee that the token is not mutated until transmission is complete
  //        and `callback` has been signalled.
  //
  // The transmission data is copied into a buffer before being sent to the
  // simple network protocol, so it is safe to overwrite that as soon
  // as SendV6Frame returns.
  fit::result<efi_status> SendV6LocalFrame(MacAddr dst, std::span<const uint8_t> data,
                                           efi_event callback,
                                           efi_managed_network_sync_completion_token* token);

 private:
  explicit EthernetAgent(EfiProtocolPtr<efi_managed_network_protocol> net_proto,
                         const MacAddr& mac_addr)
      : net_proto_(std::move(net_proto)), mac_addr_(mac_addr) {}

  EfiProtocolPtr<efi_managed_network_protocol> net_proto_;
  MacAddr mac_addr_;
};

}  // namespace gigaboot

#endif  // SRC_FIRMWARE_GIGABOOT_CPP_NETWORK_H_
