// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "addr.h"

#include <arpa/inet.h>
#include <net/if.h>
#include <netdb.h>
#include <netinet/in.h>

#include <iomanip>
#include <ostream>

#include "lib/fit/defer.h"
#include "log.h"
#include "packet.h"
#include "util.h"

#if PACKET_SOCKETS
#include <netpacket/packet.h>

#include "api_abstraction.h"
#endif

struct PrintInterface {
  explicit PrintInterface(unsigned int iface_index) : iface_index(iface_index) {}

  const unsigned int iface_index;
};

std::ostream& operator<<(std::ostream& out, const PrintInterface& interface) {
  char ifname[IF_NAMESIZE] = {};
  char* name = if_indextoname(interface.iface_index, ifname);
  if (name == nullptr) {
    LOG(ERROR) << "Error: if_indextoname(" << interface.iface_index << "): " << strerror(errno);
    return out << interface.iface_index;
  }
  return out << name;
}

std::ostream& operator<<(std::ostream& out, const PrintHardwareAddress ha) {
  // Store the original out format flags, so that they can be restored later.
  auto out_flags = out.flags();
  out << "[";
  for (int i = 0; i < ha.len; i++) {
    if (i != 0) {
      out << ":";
    }
    out << std::setw(2) << std::setfill('0') << std::hex << (int)ha.addr_bytes[i];
  }
  out << "]";

  out.flags(out_flags);
  return out;
}

std::string Format(const sockaddr_storage& addr) {
  switch (addr.ss_family) {
    case AF_INET: {
      const sockaddr_in& addr_in = *reinterpret_cast<const sockaddr_in*>(&addr);
      char buf[INET_ADDRSTRLEN] = {};
      std::stringstream o;
      o << inet_ntop(addr_in.sin_family, &addr_in.sin_addr, buf, sizeof(buf));
      if (addr_in.sin_port != 0) {
        o << ':' << ntohs(addr_in.sin_port);
      }
      return o.str();
    }
    case AF_INET6: {
      const sockaddr_in6& addr_in = *reinterpret_cast<const sockaddr_in6*>(&addr);
      char buf[INET6_ADDRSTRLEN] = {};
      std::stringstream o;
      o << '[' << inet_ntop(addr_in.sin6_family, &addr_in.sin6_addr, buf, sizeof(buf));
      if (addr_in.sin6_scope_id != 0) {
        o << '%' << PrintInterface(addr_in.sin6_scope_id);
      }
      o << ']';
      if (addr_in.sin6_port != 0) {
        o << ":" << ntohs(addr_in.sin6_port);
      }
      return o.str();
    }
    case AF_UNSPEC:
      return "<unspec>";

#if PACKET_SOCKETS
    case AF_PACKET: {
      const sockaddr_ll& addr_ll = *reinterpret_cast<const sockaddr_ll*>(&addr);
      std::stringstream o;

      o << "(";

      o << "protocol:" << ntohs(addr_ll.sll_protocol);

      if (addr_ll.sll_ifindex != 0) {
        o << ", interface:" << PrintInterface(addr_ll.sll_ifindex);
      } else {
        o << ", interface:<None>";
      }

      o << ", packet_type:" << (int)addr_ll.sll_pkttype;

      o << ", address:" << PrintHardwareAddress{addr_ll.sll_halen, addr_ll.sll_addr};

      o << ")";
      return o.str();
    }
#endif

    default:
      std::stringstream o;
      o << '<' << addr.ss_family << '>';
      return o.str();
  }
}

std::optional<sockaddr_storage> Parse(const std::string& ip_port_str) {
  std::string ip_str = ip_port_str;
  std::string port_str = ip_port_str;
  if (ip_port_str[0] == '[') {
    size_t addr_end_pos = ip_port_str.find_first_of(']');
    if (addr_end_pos == std::string::npos) {
      LOG(ERROR) << "Error-Cannot parse ip_port_str='" << ip_port_str
                 << "' for <ip>:<port> - missing address closing bracket ']'!";
      return std::nullopt;
    }
    ip_str = ip_port_str.substr(1, addr_end_pos - 1);
    port_str = ip_port_str.substr(addr_end_pos);
  }
  const size_t col_pos = port_str.find_first_of(':');
  if (col_pos == std::string::npos) {
    LOG(ERROR) << "Error-Cannot parse ip_port_str='" << ip_port_str
               << "' for <ip>:<port> - missing port!";
    return std::nullopt;
  }
  if (port_str.find_first_of(':', col_pos + 1) != std::string::npos) {
    LOG(ERROR) << "Error-Cannot parse ip_port_str='" << ip_port_str
               << "' for <ip>:<port> - too many colons ':',"
               << " use '[]' brackets around IPv6 addresses!";
    return std::nullopt;
  }
  if (ip_str == ip_port_str) {
    ip_str = ip_port_str.substr(0, col_pos);
  }
  port_str = port_str.substr(col_pos + 1);

  return Parse(ip_str, port_str);
}

std::optional<sockaddr_storage> Parse(const std::string& ip_str,
                                      const std::optional<std::string>& port_str) {
  if (ip_str.empty()) {
    LOG(ERROR) << "Error: Empty address string given!";
    return std::nullopt;
  }

  struct addrinfo hints = {};
  hints.ai_flags = AI_NUMERICHOST;
  struct addrinfo* result;
  int s = getaddrinfo(
      ip_str.c_str(),
      [port_str]() -> const char* {
        if (port_str.has_value()) {
          return port_str.value().c_str();
        }
        return nullptr;
      }(),
      &hints, &result);
  if (s != 0) {
    std::stringstream o;
    o << ip_str;
    o << ", ";
    if (port_str.has_value()) {
      o << port_str.value();
    } else {
      o << "[nullptr]";
    }
    LOG(ERROR) << "Error-getaddrinfo(" << o.str() << "): " << gai_strerror(s);
    return std::nullopt;
  }
  auto cleanup = fit::defer([result]() { freeaddrinfo(result); });

  std::optional<std::pair<sockaddr*, socklen_t>> candidate;
  for (const addrinfo* rp = result; rp != nullptr; rp = rp->ai_next) {
    if (candidate.has_value()) {
      auto [ptr, len] = candidate.value();
      if (memcmp(ptr, rp->ai_addr, std::min(len, rp->ai_addrlen)) != 0) {
        sockaddr_storage left, right;
        memcpy(&left, ptr, len);
        memcpy(&right, rp->ai_addr, rp->ai_addrlen);
        LOG(WARNING) << "Multiple choices " << Format(left) << " vs " << Format(right);
      }
    }
    candidate = std::make_pair(rp->ai_addr, rp->ai_addrlen);
  }

  if (candidate.has_value()) {
    auto [ptr, len] = candidate.value();
    sockaddr_storage addr;
    memcpy(&addr, ptr, len);
    return addr;
  }

  std::stringstream o;
  o << ip_str;
  o << ", ";
  if (port_str.has_value()) {
    o << port_str.value();
  } else {
    o << "[nullptr]";
  }
  LOG(ERROR) << "Error-getaddrinfo(" << o.str() << ") returned no results";
  return std::nullopt;
}

std::pair<std::optional<in_addr>, std::optional<int>> ParseIpv4WithScope(
    const std::string& ip_id_str) {
  std::string ip_str = ip_id_str;
  std::string id_str;
  size_t sep = ip_id_str.find_first_of('%');
  if (sep != std::string::npos) {
    ip_str = ip_str.substr(0, sep);
    id_str = ip_id_str.substr(sep + 1);
  }

  std::optional<in_addr> addr_opt;
  if (!ip_str.empty()) {
    std::optional addr = Parse(ip_str, std::nullopt);
    if (addr.has_value()) {
      if (addr.value().ss_family != AF_INET) {
        LOG(ERROR) << "Error: Invalid interface address='" << ip_str << "'!";
      } else {
        addr_opt = reinterpret_cast<sockaddr_in*>(&addr.value())->sin_addr;
      }
    }
  }
  std::optional<int> id_opt;
  if (!id_str.empty()) {
    int id;
    if (!str2int(id_str, &id) || id < 0) {
      LOG(ERROR) << "Error: Invalid interface ID='" << id_str << "'!";
    } else {
      id_opt = id;
    }
  }

  return std::make_pair(addr_opt, id_opt);
}

socklen_t AddrLen(const sockaddr_storage& addr) {
  switch (addr.ss_family) {
    case AF_INET: {
      return sizeof(sockaddr_in);
    }
    case AF_INET6: {
      return sizeof(sockaddr_in6);
    }
    default: {
      return sizeof(addr);
    }
  }
}

#if PACKET_SOCKETS
std::optional<sockaddr_ll> ParseSockAddrLlFromArg(const std::string& argstr, ApiAbstraction* api) {
  size_t col_pos = argstr.find_first_of(':');
  if (col_pos == std::string::npos) {
    LOG(ERROR) << "Error-Cannot parse packet socket addr='" << argstr
               << "' for <protocol>:<ifname> - missing separating colon ':'!";
    return std::nullopt;
  }

  std::string protocol_str = argstr.substr(0, col_pos);
  std::string ifname_str = argstr.substr(col_pos + 1);

  int protocol;
  if (!str2int(protocol_str, &protocol)) {
    LOG(ERROR) << "Error-Cannot parse protocol number='" << protocol_str << "'!";
    return std::nullopt;
  }

  unsigned int if_index = 0;
  if (!ifname_str.empty()) {
    if_index = api->if_nametoindex(ifname_str.c_str());
    if (!if_index) {
      LOG(ERROR) << "Error-if_nametoindex(" << ifname_str << ") failed -" << "[" << errno << "]"
                 << strerror(errno);
      return std::nullopt;
    }
  }

  const struct sockaddr_ll sll = {
      .sll_family = AF_PACKET,
      .sll_protocol = htons(static_cast<uint16_t>(protocol)),
      .sll_ifindex = static_cast<int>(if_index),
  };
  return std::make_optional(sll);
}
#endif  // PACKET_SOCKETS
