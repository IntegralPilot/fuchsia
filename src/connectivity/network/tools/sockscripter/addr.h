// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_TOOLS_SOCKSCRIPTER_ADDR_H_
#define SRC_CONNECTIVITY_NETWORK_TOOLS_SOCKSCRIPTER_ADDR_H_

#include <netinet/in.h>

#include "packet.h"
#if PACKET_SOCKETS
#include <netpacket/packet.h>

#include "api_abstraction.h"
#endif  // PACKET_SOCKETS

#include <optional>
#include <string>

// Pretty prints an sockaddr_ll hardware address.
struct PrintHardwareAddress {
  const unsigned char len;
  const unsigned char (&addr_bytes)[8];
};
std::ostream& operator<<(std::ostream& out, const PrintHardwareAddress ha);

std::string Format(const sockaddr_storage& addr);

// Parse an address of the form \[IP[%ID]\]:PORT.
std::optional<sockaddr_storage> Parse(const std::string& ip_port_str);

// Parse an IP address and optional port.
std::optional<sockaddr_storage> Parse(const std::string& ip_str,
                                      const std::optional<std::string>& port_str);

// Parse an IPv4 address with optional index specifier of the form IP[%ID].
//
// This function exists because sockaddr_in does not carry an interface index.
std::pair<std::optional<in_addr>, std::optional<int>> ParseIpv4WithScope(
    const std::string& ip_id_str);

/// Returns the length of the provided address based on its family.
socklen_t AddrLen(const sockaddr_storage& addr);

#if PACKET_SOCKETS
/// Parses the `argstr` into a packet socket address (`sockaddr_ll`).
///
/// The expected format for `argstr` is "<protocol>:<ifname>".
std::optional<sockaddr_ll> ParseSockAddrLlFromArg(const std::string& argstr, ApiAbstraction* api);
#endif  // PACKET_SOCKETS

#endif  // SRC_CONNECTIVITY_NETWORK_TOOLS_SOCKSCRIPTER_ADDR_H_
