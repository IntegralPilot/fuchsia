// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.posix.socket.packet/cpp/wire.h>
#include <fidl/fuchsia.posix.socket/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <lib/fit/result.h>
#include <lib/zx/socket.h>
#include <lib/zxio/bsdsocket.h>
#include <lib/zxio/cpp/transitional.h>
#include <lib/zxio/cpp/udp_socket_private.h>
#include <lib/zxio/fault_catcher.h>
#include <lib/zxio/null.h>
#include <netinet/icmp6.h>
#include <netinet/if_ether.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <zircon/availability.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <algorithm>
#include <bitset>
#include <cstring>
#include <type_traits>

#include <fbl/unaligned.h>
#include <safemath/safe_conversions.h>

#include "sdk/lib/zxio/dgram_cache.h"
#include "sdk/lib/zxio/private.h"
#include "sdk/lib/zxio/socket_address.h"
#include "sdk/lib/zxio/vector.h"
#include "src/connectivity/network/netstack/udp_serde/udp_serde.h"

namespace fio = fuchsia_io;
namespace fsocket = fuchsia_posix_socket;
namespace frawsocket = fuchsia_posix_socket_raw;
namespace fpacketsocket = fuchsia_posix_socket_packet;
namespace fnet = fuchsia_net;

/* Socket class hierarchy
 *
 *  Wrapper structs for supported FIDL protocols used to template socket_with_event.
 *
 *   +-------------------------+  +---------------------+  +-------------------------------------+
 *   |   struct PacketSocket   |  |  struct RawSocket   |  |  struct SynchronousDatagramSocket   |
 *   |  fpacketsocket::Socket  |  |  frawsocket:Socket  |  |  fsocket:SynchronousDatagramSocket  |
 *   +-------------------------+  +---------------------+  +-------------------------------------+
 *
 *  Socket class helpers for common socket operations.
 *
 *   +------------------------------------+
 *   | socket_with_event                  |
 *   |                                    |
 *   | Used by:                           |
 *   |              PacketSocket::Storage |
 *   |                 RawSocket::Storage |
 *   | SynchronousDatagramSocket::Storage |
 *   |                                    |
 *   | Implements:                        |
 *   |   Overrides for sockets using FIDL |
 *   |   over channel as a data plane.    |
 *   +------------------------------------+
 *
 *   +------------------------------------+     +----------------------+ +------------------------+
 *   | network_socket                     |     | stream_socket        | | datagram_socket        |
 *   |                                    |     |                      | |                        |
 *   | Used by:                           |     | Used by:             | | Used by:               |
 *   |             zxio_datagram_socket_t |     | zxio_stream_socket_t | | zxio_datagram_socket_t |
 *   |               zxio_stream_socket_t |     |                      | |                        |
 *   |                 RawSocket::Storage |     | Implements:          | | Implements:            |
 *   | SynchronousDatagramSocket::Storage |     |   Overrides for      | |   Overrides for        |
 *   |                                    |     |   SOCK_STREAM        | |   SOCK_DGRAM           |
 *   | Implements:                        |     |   sockets using      | |   sockets using        |
 *   |   Overrides for network layer      |     |   a zx::socket       | |   a zx::socket         |
 *   |   sockets.                         |     |   data plane.        | |   data plane.          |
 *   +-----------+------------------------+     +---------+------------+ +------------+-----------+
 *               |                                        |                           |
 *               |                                        +-----------+---------------+
 *               |                                                    |
 *   +-----------+------------+                           +-----------+-------------+
 *   |      base_socket       |                           |  socket_with_zx_socket  |
 *   |                        |                           |                         |
 *   |      Used by: All      |                           |       Implements:       |
 *   |                        |                           |      Overrides for      |
 *   |      Implements:       |                           |     sockets using a     |
 *   |   Overrides for all    |                           |  zx::socket data plane  |
 *   |      socket types      |                           +-------------------------+
 *   +------------------------+
 */

namespace {

uint16_t fidl_protoassoc_to_protocol(
    const fidl::WireOptional<fpacketsocket::wire::ProtocolAssociation>& optional_protocol) {
  // protocol is not provided by the server (when the socket is not associated).
  if (!optional_protocol.has_value()) {
    return 0;
  }
  const fpacketsocket::wire::ProtocolAssociation& protocol = optional_protocol.value();

  switch (protocol.Which()) {
    case fpacketsocket::wire::ProtocolAssociation::Tag::kAll:
      return ETH_P_ALL;
    case fpacketsocket::wire::ProtocolAssociation::Tag::kSpecified:
      return protocol.specified();
  }
}

socklen_t fidl_to_sockaddr(const fuchsia_net::wire::SocketAddress& fidl, void* addr,
                           socklen_t addr_len) {
  switch (fidl.Which()) {
    case fuchsia_net::wire::SocketAddress::Tag::kIpv4: {
      const auto& ipv4 = fidl.ipv4();
      struct sockaddr_in tmp = {
          .sin_family = AF_INET,
          .sin_port = htons(ipv4.port),
      };
      static_assert(sizeof(tmp.sin_addr.s_addr) == sizeof(ipv4.address.addr),
                    "size of IPv4 addresses should be the same");
      memcpy(&tmp.sin_addr.s_addr, ipv4.address.addr.data(), sizeof(ipv4.address.addr));
      // Copy truncated address.
      memcpy(addr, &tmp, std::min(sizeof(tmp), static_cast<size_t>(addr_len)));
      return sizeof(tmp);
    }
    case fuchsia_net::wire::SocketAddress::Tag::kIpv6: {
      const auto& ipv6 = fidl.ipv6();
      struct sockaddr_in6 tmp = {
          .sin6_family = AF_INET6,
          .sin6_port = htons(ipv6.port),
          .sin6_scope_id = static_cast<uint32_t>(ipv6.zone_index),
      };
      static_assert(std::size(tmp.sin6_addr.s6_addr) == decltype(ipv6.address.addr)::size(),
                    "size of IPv6 addresses should be the same");
      std::copy(ipv6.address.addr.begin(), ipv6.address.addr.end(),
                std::begin(tmp.sin6_addr.s6_addr));
      // Copy truncated address.
      memcpy(addr, &tmp, std::min(sizeof(tmp), static_cast<size_t>(addr_len)));
      return sizeof(tmp);
    }
  }
}

template <zxio_object_type_t kObjectType>
zx_status_t AttrGet(zxio_t* zxio, zxio_node_attributes_t* inout_attr) {
  if (inout_attr->has.object_type) {
    ZXIO_NODE_ATTR_SET(*inout_attr, object_type, kObjectType);
  }
  return ZX_OK;
}

// https://github.com/torvalds/linux/blob/f2850dd5ee0/include/net/tcp.h#L1012
constexpr socklen_t kTcpCANameMax = 16;
constexpr const char kCcCubic[kTcpCANameMax] = "cubic";
constexpr const char kCcReno[kTcpCANameMax] = "reno";

struct SockOptResult {
  const zx_status_t status;
  const int16_t err;

  // An implicit constructor to simplify error handling. This should represent an errno.
  SockOptResult(fit::error<int16_t> error)
      : SockOptResult(ZX_OK, fit::result<int16_t>(error).error_value()) {}

  SockOptResult(zx_status_t status, int16_t err) : status(status), err(err) {}

  bool ok() const { return status == ZX_OK && err == 0; }

  static inline SockOptResult Ok() { return SockOptResult{ZX_OK, 0}; }

  static inline SockOptResult Errno(int16_t err) { return SockOptResult{ZX_OK, err}; }

  static inline SockOptResult Zx(zx_status_t status) { return SockOptResult{status, 0}; }

  template <typename T>
  static inline SockOptResult FromFidlResponse(const T& response) {
    if (response.status() != ZX_OK) {
      return SockOptResult::Zx(response.status());
    }
    const auto& result = response.value();
    if (result.is_error()) {
      return SockOptResult::Errno(static_cast<int16_t>(result.error_value()));
    }
    return SockOptResult::Ok();
  }
};

// A helper class that forbids direct access to `optval` and `optlen`.
template <typename OptVal, typename OptLen>
class OptValAndLen {
  static_assert(std::is_pointer_v<OptVal>);
  static_assert(std::is_same_v<std::remove_pointer_t<OptLen>, socklen_t>);
  static_assert(std::is_same_v<std::remove_const_t<std::remove_pointer_t<OptVal>>, void>);

 protected:
  OptValAndLen(OptVal optval, OptLen optlen) : optval_(optval), optlen_(optlen) {}
  fit::result<int16_t, const OptVal> checked_optval() const {
    if (!optval_) {
      return fit::as_error<int16_t>(EFAULT);
    }
    return fit::success(optval_);
  }

  // The only exception that allows direct access is when `optlen_` is not a pointer.
  template <typename T = OptLen, typename = std::enable_if_t<!std::is_pointer_v<T>>>
  OptLen optlen() const {
    return optlen_;
  }

  template <typename T = OptLen, typename = std::enable_if_t<std::is_pointer_v<T>>>
  fit::result<int16_t, const OptLen> checked_optlen() const {
    if (!optlen_) {
      return fit::as_error<int16_t>(EFAULT);
    }
    return fit::success(optlen_);
  }

  template <typename T = OptLen, typename = std::enable_if_t<std::is_pointer_v<T>>>
  fit::result<int16_t, std::pair<const OptVal, const OptLen>> checked_optval_and_optlen() const {
    if (!optlen_ || !optval_) {
      return fit::as_error<int16_t>(EFAULT);
    }
    return fit::success(std::pair(optval_, optlen_));
  }

 private:
  const OptVal optval_;
  const OptLen optlen_;
};

class GetSockOptProcessor : OptValAndLen<void*, socklen_t*> {
 public:
  GetSockOptProcessor(void* optval, socklen_t* optlen) : OptValAndLen(optval, optlen) {}

  template <typename T, typename F>
  SockOptResult Process(T&& response, F getter) {
    if (response.status() != ZX_OK) {
      return SockOptResult::Zx(response.status());
    }
    const auto& result = response.value();
    if (result.is_error()) {
      return SockOptResult::Errno(static_cast<int16_t>(result.error_value()));
    }
    return StoreOption(getter(*result.value()));
  }

  template <typename T>
  SockOptResult StoreOption(const T& value) {
    static_assert(sizeof(T) != sizeof(T), "function must be specialized");
  }

 private:
  SockOptResult StoreRaw(const void* data, socklen_t data_len) {
    auto optval_len = checked_optval_and_optlen();
    if (optval_len.is_error()) {
      return optval_len.take_error();
    }
    auto [optval, optlen] = *optval_len;
    socklen_t copy_len = std::min(data_len, *optlen);
    memcpy(optval, data, copy_len);
    *optlen = copy_len;
    return SockOptResult::Ok();
  }
};

template <>
SockOptResult GetSockOptProcessor::StoreOption(const int32_t& value) {
  return StoreRaw(&value, sizeof(int32_t));
}

template <>
SockOptResult GetSockOptProcessor::StoreOption(const uint64_t& value) {
  return StoreRaw(&value, sizeof(uint64_t));
}

template <>
SockOptResult GetSockOptProcessor::StoreOption(const uint32_t& value) {
  return StoreRaw(&value, sizeof(uint32_t));
}

template <>
SockOptResult GetSockOptProcessor::StoreOption(const uint8_t& value) {
  return StoreRaw(&value, sizeof(uint8_t));
}

template <>
SockOptResult GetSockOptProcessor::StoreOption(const fsocket::wire::Domain& value) {
  int32_t domain;
  switch (value) {
    case fsocket::wire::Domain::kIpv4:
      domain = AF_INET;
      break;
    case fsocket::wire::Domain::kIpv6:
      domain = AF_INET6;
      break;
  }
  return StoreOption(domain);
}

template <>
SockOptResult GetSockOptProcessor::StoreOption(const bool& value) {
  return StoreOption(static_cast<uint32_t>(value));
}

template <>
SockOptResult GetSockOptProcessor::StoreOption(const struct linger& value) {
  return StoreRaw(&value, sizeof(struct linger));
}

template <>
SockOptResult GetSockOptProcessor::StoreOption(const fidl::StringView& value) {
  auto optval_len = checked_optval_and_optlen();
  if (optval_len.is_error()) {
    return optval_len.take_error();
  }
  auto [optval, optlen] = *optval_len;
  if (value.empty()) {
    *optlen = 0;
  } else if (*optlen > value.size()) {
    char* p = std::copy(value.begin(), value.end(), static_cast<char*>(optval));
    *p = 0;
    *optlen = static_cast<socklen_t>(value.size()) + 1;
  } else {
    return SockOptResult::Errno(EINVAL);
  }
  return SockOptResult::Ok();
}

// Helper type to provide GetSockOptProcessor with a truncating string view conversion.
struct TruncatingStringView {
  explicit TruncatingStringView(fidl::StringView string) : string(string) {}

  fidl::StringView string;
};

template <>
SockOptResult GetSockOptProcessor::StoreOption(const TruncatingStringView& value) {
  auto optval_len = checked_optval_and_optlen();
  if (optval_len.is_error()) {
    return optval_len.take_error();
  }
  auto [optval, optlen] = *optval_len;
  *optlen = std::min(*optlen, static_cast<socklen_t>(value.string.size()));
  if (*optlen > 0) {
    char* p = std::copy_n(value.string.begin(), *optlen - 1, static_cast<char*>(optval));
    *p = 0;
  }
  return SockOptResult::Ok();
}

template <>
SockOptResult GetSockOptProcessor::StoreOption(const fsocket::wire::OptionalUint8& value) {
  switch (value.Which()) {
    case fsocket::wire::OptionalUint8::Tag::kValue:
      return StoreOption(static_cast<int32_t>(value.value()));
    case fsocket::wire::OptionalUint8::Tag::kUnset:
      return StoreOption(-1);
  }
}

template <>
SockOptResult GetSockOptProcessor::StoreOption(const fsocket::wire::OptionalUint32& value) {
  switch (value.Which()) {
    case fsocket::wire::OptionalUint32::Tag::kValue:
      ZX_ASSERT(value.value() < std::numeric_limits<int32_t>::max());
      return StoreOption(static_cast<int32_t>(value.value()));
    case fsocket::wire::OptionalUint32::Tag::kUnset:
      return StoreOption(-1);
  }
}

template <>
SockOptResult GetSockOptProcessor::StoreOption(const fnet::wire::Ipv4Address& value) {
  static_assert(sizeof(struct in_addr) == sizeof(value.addr));
  return StoreRaw(value.addr.data(), sizeof(value.addr));
}

template <>
SockOptResult GetSockOptProcessor::StoreOption(const frawsocket::wire::Icmpv6Filter& value) {
  auto optval_len = checked_optval_and_optlen();
  if (optval_len.is_error()) {
    return optval_len.take_error();
  }
  auto [optval, optlen] = *optval_len;
  static_assert(sizeof(icmp6_filter) == sizeof(value.blocked_types));
  *optlen = std::min(static_cast<socklen_t>(sizeof(icmp6_filter)), *optlen);
  memcpy(optval, value.blocked_types.data(), *optlen);
  return SockOptResult::Ok();
}

template <>
SockOptResult GetSockOptProcessor::StoreOption(const fsocket::wire::TcpInfo& value) {
  tcp_info info;
  // Explicitly initialize unsupported fields to a garbage value. It would probably be quieter
  // to zero-initialize, but that can mask bugs in the interpretation of fields for which zero
  // is a valid value.
  //
  // Note that "unsupported" includes fields not defined in FIDL *and* fields not populated by
  // the server.
  memset(&info, 0xff, sizeof(info));

  auto optlen = checked_optlen();
  if (optlen.is_error()) {
    return optlen.take_error();
  }

  if (value.has_state()) {
    info.tcpi_state = [](fsocket::wire::TcpState state) -> uint8_t {
      switch (state) {
        case fsocket::wire::TcpState::kEstablished:
          return TCP_ESTABLISHED;
        case fsocket::wire::TcpState::kSynSent:
          return TCP_SYN_SENT;
        case fsocket::wire::TcpState::kSynRecv:
          return TCP_SYN_RECV;
        case fsocket::wire::TcpState::kFinWait1:
          return TCP_FIN_WAIT1;
        case fsocket::wire::TcpState::kFinWait2:
          return TCP_FIN_WAIT2;
        case fsocket::wire::TcpState::kTimeWait:
          return TCP_TIME_WAIT;
        case fsocket::wire::TcpState::kClose:
          return TCP_CLOSE;
        case fsocket::wire::TcpState::kCloseWait:
          return TCP_CLOSE_WAIT;
        case fsocket::wire::TcpState::kLastAck:
          return TCP_LAST_ACK;
        case fsocket::wire::TcpState::kListen:
          return TCP_LISTEN;
        case fsocket::wire::TcpState::kClosing:
          return TCP_CLOSING;
      }
    }(value.state());
  }
  if (value.has_ca_state()) {
    info.tcpi_ca_state = [](fsocket::wire::TcpCongestionControlState ca_state) -> uint8_t {
      switch (ca_state) {
        case fsocket::wire::TcpCongestionControlState::kOpen:
          return TCP_CA_Open;
        case fsocket::wire::TcpCongestionControlState::kDisorder:
          return TCP_CA_Disorder;
        case fsocket::wire::TcpCongestionControlState::kCongestionWindowReduced:
          return TCP_CA_CWR;
        case fsocket::wire::TcpCongestionControlState::kRecovery:
          return TCP_CA_Recovery;
        case fsocket::wire::TcpCongestionControlState::kLoss:
          return TCP_CA_Loss;
      }
    }(value.ca_state());
  }

  // These fields are unset when the current  TCP State doesn't have a value for
  // them (e.g. RTT is unknown during the handshake).
  info.tcpi_rto = value.has_rto_usec() ? value.rto_usec() : 0;
  info.tcpi_rtt = value.has_rtt_usec() ? value.rtt_usec() : 0;
  info.tcpi_rttvar = value.has_rtt_var_usec() ? value.rtt_var_usec() : 0;
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
  info.tcpi_snd_mss = value.has_tcpi_snd_mss() ? value.tcpi_snd_mss() : 0;
  info.tcpi_rcv_mss = value.has_tcpi_rcv_mss() ? value.tcpi_rcv_mss() : 0;
#endif

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  // Setting these to MAX doesn't change their value, but if and when we stop
  // filling tcp_info with garbage, these should not default to zero. That would
  // mean "last (data sent|ack received) 0 ms ago".
  //
  // The equivalent Linux behavior is to return the number of milliseconds since
  // boot (it calculates boot_timestamp - last_<event>_timestamp and the event
  // timestamps all start at 0).
  info.tcpi_last_data_sent = value.has_tcpi_last_data_sent_msec()
                                 ? value.tcpi_last_data_sent_msec()
                                 : std::numeric_limits<uint32_t>::max();
  info.tcpi_last_ack_recv = value.has_tcpi_last_ack_recv_msec()
                                ? value.tcpi_last_ack_recv_msec()
                                : std::numeric_limits<uint32_t>::max();

  if (value.has_tcpi_segs_out()) {
    info.tcpi_segs_out = static_cast<uint32_t>(std::min(
        value.tcpi_segs_out(), static_cast<uint64_t>(std::numeric_limits<uint32_t>::max())));
  }
  if (value.has_tcpi_segs_in()) {
    info.tcpi_segs_in = static_cast<uint32_t>(std::min(
        value.tcpi_segs_in(), static_cast<uint64_t>(std::numeric_limits<uint32_t>::max())));
  }
  if (value.has_tcpi_total_retrans()) {
    info.tcpi_total_retrans = value.tcpi_total_retrans();
  }
#endif

  if (value.has_snd_ssthresh()) {
    info.tcpi_snd_ssthresh = value.snd_ssthresh();
  }
  if (value.has_snd_cwnd()) {
    info.tcpi_snd_cwnd = value.snd_cwnd();
  }
  if (value.has_reorder_seen()) {
    info.tcpi_reord_seen = value.reorder_seen();
  }

  static_assert(sizeof(info) <= std::numeric_limits<socklen_t>::max());
  return StoreRaw(&info, std::min(**optlen, static_cast<socklen_t>(sizeof(info))));
}

template <>
SockOptResult GetSockOptProcessor::StoreOption(const fuchsia_net::wire::SocketAddress& value) {
  auto optval_len = checked_optval_and_optlen();
  if (optval_len.is_error()) {
    return optval_len.take_error();
  }
  auto [optval, optlen] = *optval_len;
  *optlen = fidl_to_sockaddr(value, optval, *optlen);
  return SockOptResult::Ok();
}

// TODO(https://fxbug.dev/384115233): Update after the API is stabilized.
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
struct FidlSocketMarkWithDomain {
  FidlSocketMarkWithDomain() = default;
  explicit FidlSocketMarkWithDomain(fsocket::wire::OptionalUint32 mark,
                                    fnet::wire::MarkDomain domain)
      : mark(mark), domain(domain) {}

  fsocket::wire::OptionalUint32 mark;
  fnet::wire::MarkDomain domain;
};

zxio_socket_mark_domain_t from_fidl_mark_domain(fnet::wire::MarkDomain domain) {
  switch (domain) {
    case fnet::wire::MarkDomain::kMark1:
      return ZXIO_SOCKET_MARK_DOMAIN_1;
    case fnet::wire::MarkDomain::kMark2:
      return ZXIO_SOCKET_MARK_DOMAIN_2;
  }
}

fit::result<int16_t, fnet::wire::MarkDomain> into_fidl_mark_domain(
    zxio_socket_mark_domain_t domain) {
  switch (domain) {
    case ZXIO_SOCKET_MARK_DOMAIN_1:
      return fit::success(fnet::wire::MarkDomain::kMark1);
    case ZXIO_SOCKET_MARK_DOMAIN_2:
      return fit::success(fnet::wire::MarkDomain::kMark2);
    default:
      return fit::as_error<int16_t>(EINVAL);
  }
}

template <>
SockOptResult GetSockOptProcessor::StoreOption(const FidlSocketMarkWithDomain& mark_and_domain) {
  const auto& [mark, domain] = mark_and_domain;
  // Fuchsia socket marks are optional. It's different between having a mark with 0
  // and not having a mark at all. So if `is_present` is false, then `value` has
  // no meaning.
  zxio_socket_mark_t socket_mark{
      .value = mark.is_value() ? mark.value() : 0,
      .domain = from_fidl_mark_domain(domain),
      .is_present = mark.is_value(),
  };
  return StoreRaw(&socket_mark, sizeof(socket_mark));
}
#endif

// Used for various options that allow the caller to supply larger buffers than needed.
struct PartialCopy {
  int32_t value;
  // Appears to be true for IP_*, SO_* and false for IPV6_*.
  bool allow_char;
};

template <>
SockOptResult GetSockOptProcessor::StoreOption(const PartialCopy& value) {
  auto optval_len = checked_optval_and_optlen();
  if (optval_len.is_error()) {
    return optval_len.take_error();
  }
  auto [optval, optlen] = *optval_len;
  socklen_t want_size =
      *optlen < sizeof(int32_t) && value.allow_char ? sizeof(uint8_t) : sizeof(value.value);
  *optlen = std::min(want_size, *optlen);
  memcpy(optval, &value.value, *optlen);
  return SockOptResult::Ok();
}

class SetSockOptProcessor : OptValAndLen<const void*, socklen_t> {
 public:
  SetSockOptProcessor(const void* optval, socklen_t optlen) : OptValAndLen(optval, optlen) {}

  template <typename T>
  using GetResult = fit::result<int16_t, T>;

  template <typename T>
  GetResult<T> Get() {
    if (optlen() < sizeof(T)) {
      return fit::as_error<int16_t>(EINVAL);
    }
    auto optval = checked_optval();
    if (optval.is_error()) {
      return optval.take_error();
    }
    return fit::success(fbl::UnalignedLoad<T>(*optval));
  }

  template <typename T, typename F>
  SockOptResult Process(F f) {
    auto result = Get<T>();
    if (result.is_error()) {
      return SockOptResult::Errno(result.error_value());
    }
    return SockOptResult::FromFidlResponse(f(result.value()));
  }
};

template <>
SetSockOptProcessor::GetResult<fidl::StringView> SetSockOptProcessor::Get() {
  auto optval = checked_optval();
  if (optval.is_error()) {
    return optval.take_error();
  }
  const char* str = static_cast<const char*>(*optval);
  return fit::success(fidl::StringView::FromExternal(str, strnlen(str, optlen())));
}

template <>
SetSockOptProcessor::GetResult<bool> SetSockOptProcessor::Get() {
  auto r = Get<int32_t>();
  if (r.is_error())
    return r;
  return fit::success(r.value() != 0);
}

template <>
SetSockOptProcessor::GetResult<uint32_t> SetSockOptProcessor::Get() {
  auto r = Get<int32_t>();
  if (r.is_error()) {
    return r;
  }
  if (r.value() < 0) {
    return fit::as_error<int16_t>(EINVAL);
  }
  return fit::success<uint32_t>(r.value());
}

template <>
SetSockOptProcessor::GetResult<fsocket::wire::OptionalUint8> SetSockOptProcessor::Get() {
  auto r = Get<int32_t>();
  if (r.is_error()) {
    return r.take_error();
  }
  auto i = r.value();
  if (i < -1 || i > std::numeric_limits<uint8_t>::max()) {
    return fit::as_error<int16_t>(EINVAL);
  }
  if (i == -1) {
    return fit::success(fsocket::wire::OptionalUint8::WithUnset({}));
  } else {
    return fit::success(fsocket::wire::OptionalUint8::WithValue(static_cast<uint8_t>(i)));
  }
}

// Like OptionalUint8, but permits truncation to a single byte.
struct OptionalUint8CharAllowed {
  fsocket::wire::OptionalUint8 inner;
};

template <>
SetSockOptProcessor::GetResult<OptionalUint8CharAllowed> SetSockOptProcessor::Get() {
  if (optlen() == sizeof(uint8_t)) {
    auto optval = checked_optval();
    if (optval.is_error()) {
      return optval.take_error();
    }
    return fit::success(
        fsocket::wire::OptionalUint8::WithValue(*static_cast<const uint8_t*>(*optval)));
  }
  return Get<fsocket::wire::OptionalUint8>();
}

template <>
SetSockOptProcessor::GetResult<fsocket::wire::IpMulticastMembership> SetSockOptProcessor::Get() {
  struct in_addr local;
  struct in_addr mcast;
  fsocket::wire::IpMulticastMembership out;
  if (optlen() < sizeof(struct ip_mreqn)) {
    auto req = Get<struct ip_mreq>();
    if (req.is_error()) {
      return fit::as_error<int16_t>(EINVAL);
    }
    out.iface = 0;
    local = req->imr_interface;
    mcast = req->imr_multiaddr;
  } else {
    auto reqn = Get<struct ip_mreqn>();
    if (reqn.is_error()) {
      return fit::as_error<int16_t>(EINVAL);
    }
    out.iface = reqn->imr_ifindex;
    local = reqn->imr_address;
    mcast = reqn->imr_multiaddr;
  }
  static_assert(sizeof(out.local_addr.addr) == sizeof(local));
  memcpy(out.local_addr.addr.data(), &local, sizeof(local));
  static_assert(sizeof(out.mcast_addr.addr) == sizeof(mcast));
  memcpy(out.mcast_addr.addr.data(), &mcast, sizeof(mcast));
  return fit::success(out);
}

template <>
SetSockOptProcessor::GetResult<fsocket::wire::Ipv6MulticastMembership> SetSockOptProcessor::Get() {
  auto req = Get<struct ipv6_mreq>();
  if (req.is_error()) {
    return fit::as_error<int16_t>(EINVAL);
  }
  fsocket::wire::Ipv6MulticastMembership out;
  out.iface = req->ipv6mr_interface;
  static_assert(sizeof(req->ipv6mr_multiaddr.s6_addr) == decltype(out.mcast_addr.addr)::size());
  std::copy(std::begin(req->ipv6mr_multiaddr.s6_addr), std::end(req->ipv6mr_multiaddr.s6_addr),
            out.mcast_addr.addr.begin());
  return fit::success(out);
}

template <>
SetSockOptProcessor::GetResult<frawsocket::wire::Icmpv6Filter> SetSockOptProcessor::Get() {
  auto filter = Get<struct icmp6_filter>();
  if (filter.is_error()) {
    return fit::as_error<int16_t>(EINVAL);
  }

  static_assert(sizeof(filter.value()) == sizeof(frawsocket::wire::Icmpv6Filter::blocked_types));
  return fit::success(fbl::UnalignedLoad<frawsocket::wire::Icmpv6Filter>(&filter.value()));
}

template <>
SetSockOptProcessor::GetResult<fsocket::wire::TcpCongestionControl> SetSockOptProcessor::Get() {
  auto optval = checked_optval();
  if (optval.is_error()) {
    return optval.take_error();
  }
  if (strncmp(static_cast<const char*>(*optval), kCcCubic, optlen()) == 0) {
    return fit::success(fsocket::wire::TcpCongestionControl::kCubic);
  }
  if (strncmp(static_cast<const char*>(*optval), kCcReno, optlen()) == 0) {
    return fit::success(fsocket::wire::TcpCongestionControl::kReno);
  }
  return fit::as_error<int16_t>(ENOENT);
}

struct IntOrChar {
  int32_t value;
};

template <>
SetSockOptProcessor::GetResult<IntOrChar> SetSockOptProcessor::Get() {
  auto value = Get<int32_t>();
  if (value.is_ok()) {
    return value.take_value();
  }
  if (optlen() == 0) {
    return fit::as_error<int16_t>(EINVAL);
  }
  auto optval = checked_optval();
  if (optval.is_error()) {
    return optval.take_error();
  }
  return fit::success(*static_cast<const uint8_t*>(*optval));
}

// TODO(https://fxbug.dev/384115233): Update after the API is stabilized.
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
template <>
SetSockOptProcessor::GetResult<FidlSocketMarkWithDomain> SetSockOptProcessor::Get() {
  auto socket_mark = Get<zxio_socket_mark_t>();
  if (socket_mark.is_error()) {
    return fit::as_error<int16_t>(EINVAL);
  }
  auto fidl_domain_result = into_fidl_mark_domain(socket_mark->domain);
  if (fidl_domain_result.is_error()) {
    return fidl_domain_result.take_error();
  }
  return fit::success(FidlSocketMarkWithDomain(
      socket_mark->is_present ? fsocket::wire::OptionalUint32::WithValue(socket_mark->value)
                              : fsocket::wire::OptionalUint32::WithUnset(fsocket::wire::Empty{}),
      fidl_domain_result.value()));
}
#endif

template <typename Client,
          typename = std::enable_if_t<
              std::is_same_v<Client, fidl::WireSyncClient<fsocket::SynchronousDatagramSocket>> ||
              std::is_same_v<Client, fidl::WireSyncClient<fsocket::DatagramSocket>> ||
              std::is_same_v<Client, fidl::WireSyncClient<fsocket::StreamSocket>> ||
              std::is_same_v<Client, fidl::WireSyncClient<frawsocket::Socket>> ||
              std::is_same_v<Client, fidl::WireSyncClient<fpacketsocket::Socket>>>>
class base_socket {
  static_assert(std::is_same_v<Client, fidl::WireSyncClient<fsocket::SynchronousDatagramSocket>> ||
                std::is_same_v<Client, fidl::WireSyncClient<fsocket::DatagramSocket>> ||
                std::is_same_v<Client, fidl::WireSyncClient<fsocket::StreamSocket>> ||
                std::is_same_v<Client, fidl::WireSyncClient<frawsocket::Socket>> ||
                std::is_same_v<Client, fidl::WireSyncClient<fpacketsocket::Socket>>);

 public:
  explicit base_socket(Client& client) : client_(client) {}

  Client& client() { return client_; }

  zx_status_t CloseSocket() {
    const fidl::WireResult result = client_->Close();
    if (!result.ok()) {
      return result.status();
    }
    const auto& response = result.value();
    if (response.is_error()) {
      return response.error_value();
    }
    return ZX_OK;
  }

  zx_status_t CloneSocket(zx_handle_t* out_handle) {
    auto [client, server] = fidl::Endpoints<fuchsia_unknown::Cloneable>::Create();
    zx_status_t status = client_->Clone(std::move(server)).status();
    if (status != ZX_OK) {
      return status;
    }
    *out_handle = client.TakeChannel().release();
    return ZX_OK;
  }

  SockOptResult get_solsocket_sockopt_fidl(int optname, void* optval, socklen_t* optlen) {
    GetSockOptProcessor proc(optval, optlen);
    switch (optname) {
      case SO_TYPE:
        if constexpr (std::is_same_v<Client, fidl::WireSyncClient<fsocket::DatagramSocket>> ||
                      std::is_same_v<Client,
                                     fidl::WireSyncClient<fsocket::SynchronousDatagramSocket>>) {
          return proc.StoreOption<int32_t>(SOCK_DGRAM);
        }
        if constexpr (std::is_same_v<Client, fidl::WireSyncClient<fsocket::StreamSocket>>) {
          return proc.StoreOption<int32_t>(SOCK_STREAM);
        }
        if constexpr (std::is_same_v<Client, fidl::WireSyncClient<frawsocket::Socket>>) {
          return proc.StoreOption<int32_t>(SOCK_RAW);
        }
        if constexpr (std::is_same_v<Client, fidl::WireSyncClient<fpacketsocket::Socket>>) {
          return proc.Process(client()->GetInfo(), [](const auto& response) {
            switch (response.kind) {
              case fpacketsocket::wire::Kind::kNetwork:
                return SOCK_DGRAM;
              case fpacketsocket::wire::Kind::kLink:
                return SOCK_RAW;
            }
          });
        }
      case SO_DOMAIN:
        if constexpr (std::is_same_v<Client, fidl::WireSyncClient<fpacketsocket::Socket>>) {
          return proc.StoreOption<int32_t>(AF_PACKET);
        } else {
          return proc.Process(client()->GetInfo(),
                              [](const auto& response) { return response.domain; });
        }
      case SO_TIMESTAMP:
        return proc.Process(client()->GetTimestamp(), [](const auto& response) {
          return PartialCopy{
              .value = response.value == fsocket::wire::TimestampOption::kMicrosecond,
              .allow_char = false,
          };
        });
      case SO_TIMESTAMPNS:
        return proc.Process(client()->GetTimestamp(), [](const auto& response) {
          return PartialCopy{
              .value = response.value == fsocket::wire::TimestampOption::kNanosecond,
              .allow_char = false,
          };
        });
      case SO_PROTOCOL:
        if constexpr (std::is_same_v<Client, fidl::WireSyncClient<fsocket::DatagramSocket>> ||
                      std::is_same_v<Client,
                                     fidl::WireSyncClient<fsocket::SynchronousDatagramSocket>>) {
          return proc.Process(client()->GetInfo(), [](const auto& response) {
            switch (response.proto) {
              case fsocket::wire::DatagramSocketProtocol::kUdp:
                return IPPROTO_UDP;
              case fsocket::wire::DatagramSocketProtocol::kIcmpEcho:
                switch (response.domain) {
                  case fsocket::wire::Domain::kIpv4:
                    return IPPROTO_ICMP;
                  case fsocket::wire::Domain::kIpv6:
                    return IPPROTO_ICMPV6;
                }
            }
          });
        }
        if constexpr (std::is_same_v<Client, fidl::WireSyncClient<fsocket::StreamSocket>>) {
          return proc.Process(client()->GetInfo(), [](const auto& response) {
            switch (response.proto) {
              case fsocket::wire::StreamSocketProtocol::kTcp:
                return IPPROTO_TCP;
            }
          });
        }
        if constexpr (std::is_same_v<Client, fidl::WireSyncClient<frawsocket::Socket>>) {
          return proc.Process(client()->GetInfo(), [](const auto& response) {
            switch (response.proto.Which()) {
              case frawsocket::wire::ProtocolAssociation::Tag::kUnassociated:
                return IPPROTO_RAW;
              case frawsocket::wire::ProtocolAssociation::Tag::kAssociated:
                return static_cast<int>(response.proto.associated());
            }
          });
        }
        if constexpr (std::is_same_v<Client, fidl::WireSyncClient<fpacketsocket::Socket>>) {
          return proc.StoreOption<int32_t>(0);
        }
      case SO_ERROR: {
        auto response = client()->GetError();
        if (response.status() != ZX_OK) {
          return SockOptResult::Zx(response.status());
        }
        int32_t error_code = 0;
        const auto& result = response.value();
        if (result.is_error()) {
          error_code = static_cast<int32_t>(result.error_value());
        }
        return proc.StoreOption(error_code);
      }
      case SO_SNDBUF:
        return proc.Process(client()->GetSendBuffer(), [](const auto& response) {
          return static_cast<uint32_t>(response.value_bytes);
        });
      case SO_RCVBUF:
        return proc.Process(client()->GetReceiveBuffer(), [](const auto& response) {
          return static_cast<uint32_t>(response.value_bytes);
        });
      case SO_REUSEADDR:
        return proc.Process(client()->GetReuseAddress(),
                            [](const auto& response) { return response.value; });
      case SO_REUSEPORT:
        return proc.Process(client()->GetReusePort(),
                            [](const auto& response) { return response.value; });
      case SO_BINDTODEVICE:
        return proc.Process(
            client()->GetBindToDevice(),
            [](const auto& response) -> const fidl::StringView& { return response.value; });
      case SO_BINDTOIFINDEX:
        return proc.Process(client()->GetBindToInterfaceIndex(), [](const auto& response) {
          // It's unfortunate to cast through `int32_t`, but since this is what
          // Linux uses to represent interface IDs, we want to be able to report
          // the same values.
          return static_cast<int32_t>(response.value);
        });
      case SO_BROADCAST:
        return proc.Process(client()->GetBroadcast(),
                            [](const auto& response) { return response.value; });
      case SO_KEEPALIVE:
        return proc.Process(client()->GetKeepAlive(),
                            [](const auto& response) { return response.value; });
      case SO_LINGER:
        return proc.Process(client()->GetLinger(), [](const auto& response) {
          struct linger l;
          l.l_onoff = response.linger;
          // NB: l_linger is typed as int but interpreted as unsigned by
          // linux.
          l.l_linger = static_cast<int>(response.length_secs);
          return l;
        });
      case SO_ACCEPTCONN:
        return proc.Process(client()->GetAcceptConn(),
                            [](const auto& response) { return response.value; });
      case SO_OOBINLINE:
        return proc.Process(client()->GetOutOfBandInline(),
                            [](const auto& response) { return response.value; });
      case SO_NO_CHECK:
        return proc.Process(client()->GetNoCheck(), [](const auto& response) {
          return PartialCopy{
              .value = response.value,
              .allow_char = false,
          };
        });
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
      case SO_FUCHSIA_MARK: {
        if (*optlen < sizeof(zxio_socket_mark_t)) {
          return SockOptResult::Errno(EINVAL);
        }
        zxio_socket_mark_domain_t domain = fbl::UnalignedLoad<zxio_socket_mark_t>(optval).domain;
        auto fidl_domain_result = into_fidl_mark_domain(domain);
        if (fidl_domain_result.is_error()) {
          return SockOptResult::Errno(fidl_domain_result.error_value());
        }
        auto fidl_domain = fidl_domain_result.value();
        return proc.Process(client()->GetMark(fidl_domain), [fidl_domain](const auto& response) {
          return FidlSocketMarkWithDomain(response.mark, fidl_domain);
        });
      }
      case SO_COOKIE:
        return proc.Process(client()->GetCookie(),
                            [](const auto& response) { return response.value; });
#endif
      case SO_SNDTIMEO:
      case SO_RCVTIMEO:
      case SO_PEERCRED:
        return SockOptResult::Errno(EOPNOTSUPP);
      default:
        return SockOptResult::Errno(ENOPROTOOPT);
    }
  }

  SockOptResult set_solsocket_sockopt_fidl(int optname, const void* optval, socklen_t optlen,
                                           zx::handle access_token, zxio_t* io) {
    SetSockOptProcessor proc(optval, optlen);
    switch (optname) {
      case SO_TIMESTAMP:
        return proc.Process<bool>([this](bool value) {
          using fsocket::wire::TimestampOption;
          TimestampOption opt = value ? TimestampOption::kMicrosecond : TimestampOption::kDisabled;
          return client()->SetTimestamp(opt);
        });
      case SO_TIMESTAMPNS:
        return proc.Process<bool>([this](bool value) {
          using fsocket::wire::TimestampOption;
          TimestampOption opt = value ? TimestampOption::kNanosecond : TimestampOption::kDisabled;
          return client()->SetTimestamp(opt);
        });
      case SO_SNDBUF:
        return proc.Process<int32_t>([this](int32_t value) {
          // NB: SNDBUF treated as unsigned, we just cast the value to skip sign check.
          return client()->SetSendBuffer(static_cast<uint64_t>(value));
        });
      case SO_RCVBUF:
        return proc.Process<int32_t>([this](int32_t value) {
          // NB: RCVBUF treated as unsigned, we just cast the value to skip sign check.
          return client()->SetReceiveBuffer(static_cast<uint64_t>(value));
        });
      case SO_REUSEADDR:
        return proc.Process<bool>([this](bool value) { return client()->SetReuseAddress(value); });
      case SO_REUSEPORT: {
#if FUCHSIA_API_LEVEL_AT_LEAST(30)
        auto result = proc.Get<bool>();
        if (result.is_error()) {
          return SockOptResult::Errno(result.error_value());
        }
        fsocket::wire::ReusePortOption opt;
        if (result.value()) {
          if (!access_token) {
            return SockOptResult::Errno(EINVAL);
          }
          opt = fsocket::wire::ReusePortOption::WithEnabled(zx::event(access_token.release()));
        } else {
          opt = fsocket::wire::ReusePortOption::WithDisabled(fsocket::wire::Empty());
        }
        return SockOptResult::FromFidlResponse(client()->SetReusePort(std::move(opt)));
#else   // !FUCHSIA_API_LEVEL_AT_LEAST(30)
        // SetReusePort() takes bool in older versions.
        return proc.Process<bool>([this](bool value) { return client()->SetReusePort(value); });
#endif  // !FUCHSIA_API_LEVEL_AT_LEAST(30)
      }
      case SO_BINDTODEVICE:
        return proc.Process<fidl::StringView>(
            [this](fidl::StringView value) { return client()->SetBindToDevice(value); });
      case SO_BINDTOIFINDEX:
        // It's unfortunate to cast through `int32_t`, but since this is what
        // Linux uses to represent interface IDs, we want to be able to accept
        // the same values.
        return proc.Process<int32_t>([this](int32_t value) {
          return client()->SetBindToInterfaceIndex(static_cast<uint64_t>(value));
        });
      case SO_BROADCAST:
        return proc.Process<bool>([this](bool value) { return client()->SetBroadcast(value); });
      case SO_KEEPALIVE:
        return proc.Process<bool>([this](bool value) { return client()->SetKeepAlive(value); });
      case SO_LINGER:
        return proc.Process<struct linger>([this](struct linger value) {
          // NB: l_linger is typed as int but interpreted as unsigned by linux.
          return client()->SetLinger(value.l_onoff != 0, static_cast<uint32_t>(value.l_linger));
        });
      case SO_OOBINLINE:
        return proc.Process<bool>(
            [this](bool value) { return client()->SetOutOfBandInline(value); });
      case SO_NO_CHECK:
        return proc.Process<bool>([this](bool value) { return client()->SetNoCheck(value); });
// TODO(https://fxbug.dev/384115233): Update after the API is stabilized.
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
      case SO_FUCHSIA_MARK:
        return proc.Process<FidlSocketMarkWithDomain>([this](auto mark_and_domain) {
          auto [mark, domain] = mark_and_domain;
          return client()->SetMark(domain, mark);
        });
#endif
      case SO_SNDTIMEO:
      case SO_RCVTIMEO:
        return SockOptResult::Errno(ENOTSUP);
      default:
        return SockOptResult::Errno(ENOPROTOOPT);
    }
  }

 private:
  Client& client_;
};

template <typename T,
          typename = std::enable_if_t<
              std::is_same_v<T, fidl::WireSyncClient<fsocket::SynchronousDatagramSocket>> ||
              std::is_same_v<T, fidl::WireSyncClient<fsocket::StreamSocket>> ||
              std::is_same_v<T, fidl::WireSyncClient<frawsocket::Socket>> ||
              std::is_same_v<T, fidl::WireSyncClient<fsocket::DatagramSocket>>>>
struct network_socket : public base_socket<T> {
  static_assert(std::is_same_v<T, fidl::WireSyncClient<fsocket::SynchronousDatagramSocket>> ||
                std::is_same_v<T, fidl::WireSyncClient<fsocket::StreamSocket>> ||
                std::is_same_v<T, fidl::WireSyncClient<frawsocket::Socket>> ||
                std::is_same_v<T, fidl::WireSyncClient<fsocket::DatagramSocket>>);

 public:
  using base_socket = base_socket<T>;
  using base_socket::client;

  explicit network_socket(T& client) : base_socket(client) {}

  zx_status_t bind(const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
    SocketAddress fidl_addr;
    zx_status_t status = fidl_addr.LoadSockAddr(addr, addrlen);
    if (status != ZX_OK) {
      return status;
    }

    auto response = fidl_addr.WithFIDL(
        [this](fnet::wire::SocketAddress address) { return client()->Bind(address); });
    status = response.status();
    if (status != ZX_OK) {
      return status;
    }
    auto const& result = response.value();
    if (result.is_error()) {
      *out_code = static_cast<int16_t>(result.error_value());
      return ZX_OK;
    }
    *out_code = 0;
    return ZX_OK;
  }

  zx_status_t connect(const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
    // If address is AF_UNSPEC we should call disconnect.
    if (addr->sa_family == AF_UNSPEC) {
      auto response = client()->Disconnect();
      zx_status_t status = response.status();
      if (status != ZX_OK) {
        return status;
      }
      const auto& result = response.value();
      if (result.is_error()) {
        *out_code = static_cast<int16_t>(result.error_value());
      } else {
        *out_code = 0;
      }
      return ZX_OK;
    }

    SocketAddress fidl_addr;
    zx_status_t status = fidl_addr.LoadSockAddr(addr, addrlen);
    if (status != ZX_OK) {
      return status;
    }

    auto response = fidl_addr.WithFIDL(
        [this](fnet::wire::SocketAddress address) { return client()->Connect(address); });
    status = response.status();
    if (status != ZX_OK) {
      return status;
    }
    auto const& result = response.value();
    if (result.is_error()) {
      *out_code = static_cast<int16_t>(result.error_value());
    } else {
      *out_code = 0;
    }
    return ZX_OK;
  }

  template <typename R>
  zx_status_t getname(R&& response, struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    zx_status_t status = response.status();
    if (status != ZX_OK) {
      return status;
    }
    auto const& result = response.value();
    if (result.is_error()) {
      *out_code = static_cast<int16_t>(result.error_value());
      return ZX_OK;
    }
    if (addrlen == nullptr || (*addrlen != 0 && addr == nullptr)) {
      *out_code = EFAULT;
      return ZX_OK;
    }
    *out_code = 0;
    auto const& out = result.value()->addr;
    *addrlen = fidl_to_sockaddr(out, addr, *addrlen);
    return ZX_OK;
  }

  zx_status_t getsockname(struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    return getname(client()->GetSockName(), addr, addrlen, out_code);
  }

  zx_status_t getpeername(struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    return getname(client()->GetPeerName(), addr, addrlen, out_code);
  }

  SockOptResult getsockopt_fidl(int level, int optname, void* optval, socklen_t* optlen) {
    GetSockOptProcessor proc(optval, optlen);
    switch (level) {
      case SOL_SOCKET:
        return base_socket::get_solsocket_sockopt_fidl(optname, optval, optlen);
      case SOL_IP:
        switch (optname) {
          case IP_TTL:
            return proc.Process(client()->GetIpTtl(), [](const auto& response) {
              return PartialCopy{
                  .value = response.value,
                  .allow_char = true,
              };
            });
          case IP_RECVTTL:
            return proc.Process(client()->GetIpReceiveTtl(), [](const auto& response) {
              return PartialCopy{
                  .value = response.value,
                  .allow_char = true,
              };
            });
          case IP_MULTICAST_TTL:
            return proc.Process(client()->GetIpMulticastTtl(), [](const auto& response) {
              return PartialCopy{
                  .value = response.value,
                  .allow_char = true,
              };
            });
          case IP_MULTICAST_IF:
            return proc.Process(client()->GetIpMulticastInterface(),
                                [](const auto& response) { return response.value; });
          case IP_MULTICAST_LOOP:
            return proc.Process(client()->GetIpMulticastLoopback(), [](const auto& response) {
              return PartialCopy{
                  .value = response.value,
                  .allow_char = true,
              };
            });
          case IP_TOS:
            return proc.Process(client()->GetIpTypeOfService(), [](const auto& response) {
              return PartialCopy{
                  .value = response.value,
                  .allow_char = true,
              };
            });
          case IP_RECVTOS:
            return proc.Process(client()->GetIpReceiveTypeOfService(), [](const auto& response) {
              return PartialCopy{
                  .value = response.value,
                  .allow_char = true,
              };
            });
          case IP_PKTINFO:
            return proc.Process(client()->GetIpPacketInfo(),
                                [](const auto& response) { return response.value; });
          case SO_ORIGINAL_DST:
            return proc.Process(client()->GetOriginalDestination(),
                                [](const auto& response) { return response.value; });
          case IP_RECVORIGDSTADDR:
            return proc.Process(client()->GetIpReceiveOriginalDestinationAddress(),
                                [](const auto& response) { return response.value; });
          case IP_TRANSPARENT:
            return proc.Process(client()->GetIpTransparent(),
                                [](const auto& response) { return response.value; });
          default:
            return SockOptResult::Errno(ENOPROTOOPT);
        }
      case SOL_IPV6:
        switch (optname) {
          case IPV6_V6ONLY:
            return proc.Process(client()->GetIpv6Only(),
                                [](const auto& response) { return response.value; });
          case IPV6_TCLASS:
            return proc.Process(client()->GetIpv6TrafficClass(), [](const auto& response) {
              return PartialCopy{
                  .value = response.value,
                  .allow_char = false,
              };
            });
          case IPV6_MULTICAST_IF:
            return proc.Process(client()->GetIpv6MulticastInterface(), [](const auto& response) {
              return static_cast<uint32_t>(response.value);
            });
          case IPV6_UNICAST_HOPS:
            return proc.Process(client()->GetIpv6UnicastHops(), [](const auto& response) {
              return PartialCopy{
                  .value = response.value,
                  .allow_char = false,
              };
            });
          case IPV6_MULTICAST_HOPS:
            return proc.Process(client()->GetIpv6MulticastHops(), [](const auto& response) {
              return PartialCopy{
                  .value = response.value,
                  .allow_char = false,
              };
            });
          case IPV6_MULTICAST_LOOP:
            return proc.Process(client()->GetIpv6MulticastLoopback(), [](const auto& response) {
              return PartialCopy{
                  .value = response.value,
                  .allow_char = false,
              };
            });
          case IPV6_RECVTCLASS:
            return proc.Process(client()->GetIpv6ReceiveTrafficClass(), [](const auto& response) {
              return PartialCopy{
                  .value = response.value,
                  .allow_char = false,
              };
            });
          case IPV6_RECVHOPLIMIT:
            return proc.Process(client()->GetIpv6ReceiveHopLimit(), [](const auto& response) {
              return PartialCopy{
                  .value = response.value,
                  .allow_char = false,
              };
            });
          case IPV6_RECVPKTINFO:
            return proc.Process(client()->GetIpv6ReceivePacketInfo(), [](const auto& response) {
              return PartialCopy{
                  .value = response.value,
                  .allow_char = false,
              };
            });
          default:
            return SockOptResult::Errno(ENOPROTOOPT);
        }
      case SOL_TCP:
        if constexpr (std::is_same_v<T, fidl::WireSyncClient<fsocket::StreamSocket>>) {
          switch (optname) {
            case TCP_NODELAY:
              return proc.Process(client()->GetTcpNoDelay(),
                                  [](const auto& response) { return response.value; });
            case TCP_CORK:
              return proc.Process(client()->GetTcpCork(),
                                  [](const auto& response) { return response.value; });
            case TCP_QUICKACK:
              return proc.Process(client()->GetTcpQuickAck(),
                                  [](const auto& response) { return response.value; });
            case TCP_MAXSEG:
              return proc.Process(client()->GetTcpMaxSegment(),
                                  [](const auto& response) { return response.value_bytes; });
            case TCP_KEEPIDLE:
              return proc.Process(client()->GetTcpKeepAliveIdle(),
                                  [](const auto& response) { return response.value_secs; });
            case TCP_KEEPINTVL:
              return proc.Process(client()->GetTcpKeepAliveInterval(),
                                  [](const auto& response) { return response.value_secs; });
            case TCP_KEEPCNT:
              return proc.Process(client()->GetTcpKeepAliveCount(),
                                  [](const auto& response) { return response.value; });
            case TCP_USER_TIMEOUT:
              return proc.Process(client()->GetTcpUserTimeout(),
                                  [](const auto& response) { return response.value_millis; });
            case TCP_CONGESTION:
              return proc.Process(client()->GetTcpCongestion(), [](const auto& response) {
                switch (response.value) {
                  case fsocket::wire::TcpCongestionControl::kCubic:
                    return TruncatingStringView(
                        fidl::StringView::FromExternal(kCcCubic, sizeof(kCcCubic)));
                  case fsocket::wire::TcpCongestionControl::kReno:
                    return TruncatingStringView(
                        fidl::StringView::FromExternal(kCcReno, sizeof(kCcReno)));
                }
              });
            case TCP_DEFER_ACCEPT:
              return proc.Process(client()->GetTcpDeferAccept(),
                                  [](const auto& response) { return response.value_secs; });
            case TCP_INFO:
              return proc.Process(client()->GetTcpInfo(), [](const auto& response) -> const auto& {
                return response.info;
              });
            case TCP_SYNCNT:
              return proc.Process(client()->GetTcpSynCount(),
                                  [](const auto& response) { return response.value; });
            case TCP_WINDOW_CLAMP:
              return proc.Process(client()->GetTcpWindowClamp(),
                                  [](const auto& response) { return response.value; });
            case TCP_LINGER2:
              return proc.Process(client()->GetTcpLinger(),
                                  [](const auto& response) -> const fsocket::wire::OptionalUint32& {
                                    return response.value_secs;
                                  });
            default:
              return SockOptResult::Errno(ENOPROTOOPT);
          }
        } else {
          __FALLTHROUGH;
        }
      default:
        return SockOptResult::Errno(EOPNOTSUPP);
    }
  }

  SockOptResult setsockopt_fidl(int level, int optname, const void* optval, socklen_t optlen,
                                zx::handle access_token, zxio_t* io) {
    SetSockOptProcessor proc(optval, optlen);
    switch (level) {
      case SOL_SOCKET:
        return base_socket::set_solsocket_sockopt_fidl(optname, optval, optlen,
                                                       std::move(access_token), io);
      case SOL_IP:
        switch (optname) {
          case IP_MULTICAST_TTL:
            return proc.Process<OptionalUint8CharAllowed>([this](OptionalUint8CharAllowed value) {
              return client()->SetIpMulticastTtl(value.inner);
            });
          case IP_ADD_MEMBERSHIP: {
            return proc.Process<fsocket::wire::IpMulticastMembership>(
                [this](fsocket::wire::IpMulticastMembership value) {
                  return client()->AddIpMembership(value);
                });
          }
          case IP_DROP_MEMBERSHIP:
            return proc.Process<fsocket::wire::IpMulticastMembership>(
                [this](fsocket::wire::IpMulticastMembership value) {
                  return client()->DropIpMembership(value);
                });
          case IP_MULTICAST_IF: {
            if (optlen == sizeof(struct in_addr)) {
              return proc.Process<struct in_addr>([this](struct in_addr value) {
                fnet::wire::Ipv4Address addr;
                static_assert(sizeof(addr.addr) == sizeof(value.s_addr));
                memcpy(addr.addr.data(), &value.s_addr, sizeof(value.s_addr));
                return client()->SetIpMulticastInterface(0, addr);
              });
            }
            return proc.Process<fsocket::wire::IpMulticastMembership>(
                [this](fsocket::wire::IpMulticastMembership value) {
                  return client()->SetIpMulticastInterface(value.iface, value.local_addr);
                });
          }
          case IP_MULTICAST_LOOP:
            return proc.Process<IntOrChar>([this](IntOrChar value) {
              return client()->SetIpMulticastLoopback(value.value != 0);
            });
          case IP_TTL:
            return proc.Process<OptionalUint8CharAllowed>(
                [this](OptionalUint8CharAllowed value) { return client()->SetIpTtl(value.inner); });
          case IP_RECVTTL:
            return proc.Process<IntOrChar>(
                [this](IntOrChar value) { return client()->SetIpReceiveTtl(value.value != 0); });
          case IP_TOS:
            if (optlen == 0) {
              return SockOptResult::Ok();
            }
            return proc.Process<IntOrChar>([this](IntOrChar value) {
              return client()->SetIpTypeOfService(static_cast<uint8_t>(value.value));
            });
          case IP_RECVTOS:
            return proc.Process<IntOrChar>([this](IntOrChar value) {
              return client()->SetIpReceiveTypeOfService(value.value != 0);
            });
          case IP_PKTINFO:
            return proc.Process<IntOrChar>(
                [this](IntOrChar value) { return client()->SetIpPacketInfo(value.value != 0); });
          case IP_RECVORIGDSTADDR:
            return proc.Process<IntOrChar>([this](IntOrChar value) {
              return client()->SetIpReceiveOriginalDestinationAddress(value.value != 0);
            });
          case IP_TRANSPARENT:
            return proc.Process<IntOrChar>(
                [this](IntOrChar value) { return client()->SetIpTransparent(value.value != 0); });
          case MCAST_JOIN_GROUP:
            return SockOptResult::Errno(ENOTSUP);
          default:
            return SockOptResult::Errno(ENOPROTOOPT);
        }
      case SOL_IPV6:
        switch (optname) {
          case IPV6_V6ONLY:
            return proc.Process<bool>([this](bool value) { return client()->SetIpv6Only(value); });
          case IPV6_ADD_MEMBERSHIP:
            return proc.Process<fsocket::wire::Ipv6MulticastMembership>(
                [this](fsocket::wire::Ipv6MulticastMembership value) {
                  return client()->AddIpv6Membership(value);
                });
          case IPV6_DROP_MEMBERSHIP:
            return proc.Process<fsocket::wire::Ipv6MulticastMembership>(
                [this](fsocket::wire::Ipv6MulticastMembership value) {
                  return client()->DropIpv6Membership(value);
                });
          case IPV6_MULTICAST_IF:
            return proc.Process<IntOrChar>([this](IntOrChar value) {
              return client()->SetIpv6MulticastInterface(value.value);
            });
          case IPV6_UNICAST_HOPS:
            return proc.Process<fsocket::wire::OptionalUint8>(
                [this](fsocket::wire::OptionalUint8 value) {
                  return client()->SetIpv6UnicastHops(value);
                });
          case IPV6_MULTICAST_HOPS:
            return proc.Process<fsocket::wire::OptionalUint8>(
                [this](fsocket::wire::OptionalUint8 value) {
                  return client()->SetIpv6MulticastHops(value);
                });
          case IPV6_MULTICAST_LOOP:
            return proc.Process<bool>(
                [this](bool value) { return client()->SetIpv6MulticastLoopback(value); });
          case IPV6_TCLASS:
            return proc.Process<fsocket::wire::OptionalUint8>(
                [this](fsocket::wire::OptionalUint8 value) {
                  return client()->SetIpv6TrafficClass(value);
                });
          case IPV6_RECVTCLASS:
            return proc.Process<bool>(
                [this](bool value) { return client()->SetIpv6ReceiveTrafficClass(value); });
          case IPV6_RECVHOPLIMIT:
            return proc.Process<bool>(
                [this](bool value) { return client()->SetIpv6ReceiveHopLimit(value); });
          case IPV6_RECVPKTINFO:
            return proc.Process<bool>(
                [this](bool value) { return client()->SetIpv6ReceivePacketInfo(value); });
          default:
            return SockOptResult::Errno(ENOPROTOOPT);
        }
      case SOL_TCP:
        if constexpr (std::is_same_v<T, fidl::WireSyncClient<fsocket::StreamSocket>>) {
          switch (optname) {
            case TCP_NODELAY:
              return proc.Process<bool>(
                  [this](bool value) { return client()->SetTcpNoDelay(value); });
            case TCP_CORK:
              return proc.Process<bool>([this](bool value) { return client()->SetTcpCork(value); });
            case TCP_QUICKACK:
              return proc.Process<bool>(
                  [this](bool value) { return client()->SetTcpQuickAck(value); });
            case TCP_MAXSEG:
              return proc.Process<uint32_t>(
                  [this](uint32_t value) { return client()->SetTcpMaxSegment(value); });
            case TCP_KEEPIDLE:
              return proc.Process<uint32_t>(
                  [this](uint32_t value) { return client()->SetTcpKeepAliveIdle(value); });
            case TCP_KEEPINTVL:
              return proc.Process<uint32_t>(
                  [this](uint32_t value) { return client()->SetTcpKeepAliveInterval(value); });
            case TCP_KEEPCNT:
              return proc.Process<uint32_t>(
                  [this](uint32_t value) { return client()->SetTcpKeepAliveCount(value); });
            case TCP_USER_TIMEOUT:
              return proc.Process<uint32_t>(
                  [this](uint32_t value) { return client()->SetTcpUserTimeout(value); });
            case TCP_CONGESTION:
              return proc.Process<fsocket::wire::TcpCongestionControl>(
                  [this](fsocket::wire::TcpCongestionControl value) {
                    return client()->SetTcpCongestion(value);
                  });
            case TCP_DEFER_ACCEPT:
              return proc.Process<int32_t>([this](int32_t value) {
                if (value < 0) {
                  value = 0;
                }
                return client()->SetTcpDeferAccept(value);
              });
            case TCP_SYNCNT:
              return proc.Process<uint32_t>(
                  [this](uint32_t value) { return client()->SetTcpSynCount(value); });
            case TCP_WINDOW_CLAMP:
              return proc.Process<uint32_t>(
                  [this](uint32_t value) { return client()->SetTcpWindowClamp(value); });
            case TCP_LINGER2:
              return proc.Process<int32_t>([this](int32_t value) {
                fsocket::wire::OptionalUint32 opt;
                if (value < 0) {
                  opt = fsocket::wire::OptionalUint32::WithUnset({});
                } else {
                  opt = fsocket::wire::OptionalUint32::WithValue(static_cast<uint32_t>(value));
                }
                return client()->SetTcpLinger(opt);
              });
            default:
              return SockOptResult::Errno(ENOPROTOOPT);
          }
        } else {
          __FALLTHROUGH;
        }
      default:
        return SockOptResult::Errno(ENOPROTOOPT);
    }
  }

  zx_status_t shutdown(zxio_shutdown_options_t options, int16_t* out_code) {
    using fsocket::wire::ShutdownMode;
    ShutdownMode mode;
    if (options == ZXIO_SHUTDOWN_OPTIONS_READ) {
      mode = ShutdownMode::kRead;
    } else if (options == ZXIO_SHUTDOWN_OPTIONS_WRITE) {
      mode = ShutdownMode::kWrite;
    } else if (options == (ZXIO_SHUTDOWN_OPTIONS_READ | ZXIO_SHUTDOWN_OPTIONS_WRITE)) {
      mode = ShutdownMode::kRead | ShutdownMode::kWrite;
    } else {
      return ZX_ERR_INVALID_ARGS;
    }

    const auto response = client()->Shutdown(mode);
    zx_status_t status = response.status();
    if (status != ZX_OK) {
      return status;
    }
    const auto& result = response.value();
    if (result.is_error()) {
      *out_code = static_cast<int16_t>(result.error_value());
      return ZX_OK;
    }
    *out_code = 0;
    return ZX_OK;
  }
};

fsocket::wire::RecvMsgFlags to_recvmsg_flags(int flags) {
  fsocket::wire::RecvMsgFlags r;
  if (flags & MSG_PEEK) {
    r |= fsocket::wire::RecvMsgFlags::kPeek;
  }
  return r;
}

fsocket::wire::SendMsgFlags to_sendmsg_flags(int flags) { return fsocket::wire::SendMsgFlags(); }

uint8_t fidl_pkttype_to_pkttype(const fpacketsocket::wire::PacketType type) {
  switch (type) {
    case fpacketsocket::wire::PacketType::kHost:
      return PACKET_HOST;
    case fpacketsocket::wire::PacketType::kBroadcast:
      return PACKET_BROADCAST;
    case fpacketsocket::wire::PacketType::kMulticast:
      return PACKET_MULTICAST;
    case fpacketsocket::wire::PacketType::kOtherHost:
      return PACKET_OTHERHOST;
    case fpacketsocket::wire::PacketType::kOutgoing:
      return PACKET_OUTGOING;
  }
}

void recvmsg_populate_socketaddress(const fidl::WireOptional<fnet::wire::SocketAddress>& fidl,
                                    void* addr, socklen_t& addr_len) {
  // Result address is absent when it's not provided by the server (when the address
  // is not requested).
  if (!fidl.has_value()) {
    return;
  }

  addr_len = fidl_to_sockaddr(fidl.value(), addr, addr_len);
}

uint16_t fidl_hwtype_to_arphrd(const fpacketsocket::wire::HardwareType type) {
  switch (type) {
    case fpacketsocket::wire::HardwareType::kNetworkOnly:
      return ARPHRD_NONE;
    case fpacketsocket::wire::HardwareType::kEthernet:
      return ARPHRD_ETHER;
    case fpacketsocket::wire::HardwareType::kLoopback:
      return ARPHRD_LOOPBACK;
  }
}

void populate_from_fidl_hwaddr(const fpacketsocket::wire::HardwareAddress& addr, sockaddr_ll& s) {
  switch (addr.Which()) {
    default:
      // The server is newer than us and sending a variant we don't understand,
      // or there was a new |HardwareAddress| member that is yet to be handled.
      __FALLTHROUGH;
    case fpacketsocket::wire::HardwareAddress::Tag::kNone:
      s.sll_halen = 0;
      break;
    case fpacketsocket::wire::HardwareAddress::Tag::kEui48: {
      const fnet::wire::MacAddress& eui48 = addr.eui48();
      static_assert(std::size(decltype(s.sll_addr){}) == decltype(eui48.octets)::size() + 2);
      std::copy(eui48.octets.begin(), eui48.octets.end(), std::begin(s.sll_addr));
      s.sll_halen = decltype(eui48.octets)::size();
    } break;
  }
}

// A helper structure to keep a packet info and any members' variants
// allocations on the stack.
class PacketInfo {
 public:
  zx_status_t LoadSockAddr(const sockaddr* addr, size_t addr_len) {
    // Address length larger than sockaddr_storage causes an error for API compatibility only.
    if (addr == nullptr || addr_len > sizeof(sockaddr_storage)) {
      return ZX_ERR_INVALID_ARGS;
    }
    switch (addr->sa_family) {
      case AF_PACKET: {
        if (addr_len < sizeof(sockaddr_ll)) {
          return ZX_ERR_INVALID_ARGS;
        }
        const auto& s = *reinterpret_cast<const sockaddr_ll*>(addr);
        protocol_ = ntohs(s.sll_protocol);
        interface_id_ = s.sll_ifindex;
        switch (s.sll_halen) {
          case 0:
            eui48_storage_.reset();
            return ZX_OK;
          case ETH_ALEN: {
            fnet::wire::MacAddress address;
            static_assert(decltype(address.octets)::size() == ETH_ALEN,
                          "eui48 address must have the same size as ETH_ALEN");
            static_assert(sizeof(s.sll_addr) == ETH_ALEN + 2);
            memcpy(address.octets.data(), s.sll_addr, ETH_ALEN);
            eui48_storage_ = address;
            return ZX_OK;
          }
          default:
            return ZX_ERR_NOT_SUPPORTED;
        }
      }
      default:
        return ZX_ERR_INVALID_ARGS;
    }
  }

  template <typename F>
  std::invoke_result_t<F, fidl::ObjectView<fuchsia_posix_socket_packet::wire::PacketInfo>> WithFIDL(
      F fn) {
    auto packet_info = [this]() -> fuchsia_posix_socket_packet::wire::PacketInfo {
      return {
          .protocol = protocol_,
          .interface_id = interface_id_,
          .addr =
              [this]() {
                if (eui48_storage_.has_value()) {
                  return fuchsia_posix_socket_packet::wire::HardwareAddress::WithEui48(
                      fidl::ObjectView<fuchsia_net::wire::MacAddress>::FromExternal(
                          &eui48_storage_.value()));
                }
                return fuchsia_posix_socket_packet::wire::HardwareAddress::WithNone({});
              }(),
      };
    }();
    return fn(fidl::ObjectView<fuchsia_posix_socket_packet::wire::PacketInfo>::FromExternal(
        &packet_info));
  }

 private:
  decltype(fuchsia_posix_socket_packet::wire::PacketInfo::protocol) protocol_;
  decltype(fuchsia_posix_socket_packet::wire::PacketInfo::interface_id) interface_id_;
  std::optional<fuchsia_net::wire::MacAddress> eui48_storage_;
};

struct SynchronousDatagramSocket {
  using FidlSockAddr = SocketAddress;
  using FidlSendControlData = fsocket::wire::DatagramSocketSendControlData;
  struct Storage {
    using FidlProtocol = fuchsia_posix_socket::SynchronousDatagramSocket;

    zxio_t io;
    zx::eventpair event;
    fidl::WireSyncClient<FidlProtocol> client;
  };

  static_assert(sizeof(Storage) <= sizeof(zxio_storage_t), "must fit inside zxio_storage_t.");

  static void recvmsg_populate_msgname(
      const fsocket::wire::SynchronousDatagramSocketRecvMsgResponse& response, void* addr,
      socklen_t& addr_len) {
    recvmsg_populate_socketaddress(response.addr, addr, addr_len);
  }

  static void handle_sendmsg_response(
      const fsocket::wire::SynchronousDatagramSocketSendMsgResponse& response,
      ssize_t expected_len) {
    // TODO(https://fxbug.dev/42162902): Drop len from the response as SendMsg does
    // does not perform partial writes.
    ZX_DEBUG_ASSERT_MSG(response.len == expected_len, "got SendMsg(...) = %ld, want = %ld",
                        response.len, expected_len);
  }
};

struct RawSocket {
  using FidlSockAddr = SocketAddress;
  using FidlSendControlData = fsocket::wire::NetworkSocketSendControlData;
  struct Storage {
    using FidlProtocol = fuchsia_posix_socket_raw::Socket;

    zxio_t io;
    zx::eventpair event;
    fidl::WireSyncClient<FidlProtocol> client;
  };

  static_assert(sizeof(Storage) <= sizeof(zxio_storage_t), "must fit inside zxio_storage_t.");

  static void recvmsg_populate_msgname(const frawsocket::wire::SocketRecvMsgResponse& response,
                                       void* addr, socklen_t& addr_len) {
    recvmsg_populate_socketaddress(response.addr, addr, addr_len);
  }

  static void handle_sendmsg_response(const frawsocket::wire::SocketSendMsgResponse& response,
                                      ssize_t expected_len) {
    // TODO(https://fxbug.dev/42162902): Drop this method once DatagramSocket.SendMsg
    // no longer returns a length field.
  }
};

struct PacketSocket {
  using FidlSockAddr = PacketInfo;
  using FidlSendControlData = fpacketsocket::wire::SendControlData;
  struct Storage {
    using FidlProtocol = fuchsia_posix_socket_packet::Socket;

    zxio_t io;
    zx::eventpair event;
    fidl::WireSyncClient<FidlProtocol> client;
  };

  static_assert(sizeof(Storage) <= sizeof(zxio_storage_t), "must fit inside zxio_storage_t.");

  static void recvmsg_populate_msgname(const fpacketsocket::wire::SocketRecvMsgResponse& response,
                                       void* addr, socklen_t& addr_len) {
    fidl::ObjectView view = response.packet_info;
    if (!view) {
      // The packet info field is not provided by the server (when it is not requested).
      return;
    }

    const fpacketsocket::wire::RecvPacketInfo& info = *view;

    sockaddr_ll sll = {
        .sll_family = AF_PACKET,
        .sll_protocol = htons(info.packet_info.protocol),
        .sll_ifindex = static_cast<int>(info.packet_info.interface_id),
        .sll_hatype = fidl_hwtype_to_arphrd(info.interface_type),
        .sll_pkttype = fidl_pkttype_to_pkttype(info.packet_type),
    };
    populate_from_fidl_hwaddr(info.packet_info.addr, sll);
    memcpy(addr, &sll, std::min(sizeof(sll), static_cast<size_t>(addr_len)));
    addr_len = sizeof(sll);
  }

  static void handle_sendmsg_response(const fpacketsocket::wire::SocketSendMsgResponse& response,
                                      ssize_t expected_len) {
    // TODO(https://fxbug.dev/42162902): Drop this method once DatagramSocket.SendMsg
    // no longer returns a length field.
  }
};

std::optional<size_t> total_iov_len(const struct msghdr& msg) {
  size_t total = 0;
  for (int i = 0; i < msg.msg_iovlen; ++i) {
    const iovec& iov = msg.msg_iov[i];
    if (iov.iov_base == nullptr && iov.iov_len != 0) {
      return std::nullopt;
    }
    total += iov.iov_len;
  }
  return total;
}

size_t set_trunc_flags_and_return_out_actual(struct msghdr& msg, size_t written, size_t truncated,
                                             int flags) {
  if (truncated != 0) {
    msg.msg_flags |= MSG_TRUNC;
  } else {
    msg.msg_flags &= ~MSG_TRUNC;
  }
  if ((flags & MSG_TRUNC) != 0) {
    written += truncated;
  }
  return written;
}

class FidlControlDataProcessor {
 public:
  FidlControlDataProcessor(void* buf, socklen_t len)
      : buffer_(cpp20::span{reinterpret_cast<unsigned char*>(buf), len}) {}

  socklen_t Store(fsocket::wire::DatagramSocketRecvControlData const& control_data,
                  const RequestedCmsgSet& requested) {
    socklen_t total = 0;
    if (control_data.has_network()) {
      total += Store(control_data.network(), requested);
    }
    return total;
  }

  socklen_t Store(fsocket::wire::NetworkSocketRecvControlData const& control_data,
                  const RequestedCmsgSet& requested) {
    socklen_t total = 0;
    if (control_data.has_socket()) {
      total += Store(control_data.socket(), requested);
    }
    if (control_data.has_ip()) {
      total += Store(control_data.ip(), requested);
    }
    if (control_data.has_ipv6()) {
      total += Store(control_data.ipv6(), requested);
    }
    return total;
  }

  socklen_t Store(fpacketsocket::wire::RecvControlData const& control_data,
                  const RequestedCmsgSet& requested) {
    socklen_t total = 0;
    if (control_data.has_socket()) {
      total += Store(control_data.socket(), requested);
    }
    return total;
  }

 private:
  socklen_t Store(fsocket::wire::SocketRecvControlData const& control_data,
                  const RequestedCmsgSet& requested) {
    socklen_t total = 0;

    if (control_data.has_timestamp()) {
      const fsocket::wire::Timestamp& timestamp = control_data.timestamp();
      std::chrono::duration t = std::chrono::nanoseconds(timestamp.nanoseconds);
      std::chrono::seconds sec = std::chrono::duration_cast<std::chrono::seconds>(t);

      std::optional<fsocket::wire::TimestampOption> requested_ts = requested.so_timestamp();
      const fsocket::wire::TimestampOption& which_timestamp =
          requested_ts.has_value() ? requested_ts.value() : timestamp.requested;
      switch (which_timestamp) {
        case fsocket::wire::TimestampOption::kNanosecond: {
          const struct timespec ts = {
              .tv_sec = sec.count(),
              .tv_nsec = std::chrono::duration_cast<std::chrono::nanoseconds>(t - sec).count(),
          };
          total += StoreControlMessage(SOL_SOCKET, SO_TIMESTAMPNS, &ts, sizeof(ts));
        } break;
        case fsocket::wire::TimestampOption::kMicrosecond: {
          const struct timeval tv = {
              .tv_sec = sec.count(),
              .tv_usec = std::chrono::duration_cast<std::chrono::microseconds>(t - sec).count(),
          };
          total += StoreControlMessage(SOL_SOCKET, SO_TIMESTAMP, &tv, sizeof(tv));
        } break;
        case fsocket::wire::TimestampOption::kDisabled:
          break;
      }
    }

    return total;
  }

  socklen_t Store(fsocket::wire::IpRecvControlData const& control_data,
                  const RequestedCmsgSet& requested) {
    socklen_t total = 0;

    if (requested.ip_tos() && control_data.has_tos()) {
      const uint8_t tos = control_data.tos();
      total += StoreControlMessage(IPPROTO_IP, IP_TOS, &tos, sizeof(tos));
    }

    if (requested.ip_ttl() && control_data.has_ttl()) {
      // Even though the ttl can be encoded in a single byte, Linux returns it as an `int` when
      // it is received as a control message.
      // https://github.com/torvalds/linux/blob/7e57714cd0a/net/ipv4/ip_sockglue.c#L67
      const int ttl = static_cast<int>(control_data.ttl());
      total += StoreControlMessage(IPPROTO_IP, IP_TTL, &ttl, sizeof(ttl));
    }

    if (requested.ip_recvorigdstaddr() && control_data.has_original_destination_address()) {
      struct sockaddr_storage addr;
      socklen_t addr_len =
          fidl_to_sockaddr(control_data.original_destination_address(), &addr, sizeof(addr));
      total += StoreControlMessage(IPPROTO_IP, IP_RECVORIGDSTADDR, &addr, addr_len);
    }

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
    if (requested.ip_pktinfo() && control_data.has_pktinfo()) {
      const fsocket::wire::Ipv4PktInfoRecvControlData& fidl_pktinfo = control_data.pktinfo();
      in_pktinfo pktinfo = {
          .ipi_ifindex = static_cast<int>(fidl_pktinfo.iface),
      };
      static_assert(
          sizeof(pktinfo.ipi_spec_dst) == decltype(fidl_pktinfo.local_addr.addr)::size() &&
              sizeof(pktinfo.ipi_addr) ==
                  decltype(fidl_pktinfo.header_destination_addr.addr)::size(),
          "mismatch between size of FIDL and in_pktinfo addresses");
      memcpy(&pktinfo.ipi_spec_dst, fidl_pktinfo.local_addr.addr.data(),
             sizeof(pktinfo.ipi_spec_dst));
      memcpy(&pktinfo.ipi_addr, fidl_pktinfo.header_destination_addr.addr.data(),
             sizeof(pktinfo.ipi_addr));
      total += StoreControlMessage(IPPROTO_IP, IP_PKTINFO, &pktinfo, sizeof(pktinfo));
    }
#endif

    return total;
  }

  socklen_t Store(fsocket::wire::Ipv6RecvControlData const& control_data,
                  const RequestedCmsgSet& requested) {
    socklen_t total = 0;

    if (requested.ipv6_tclass() && control_data.has_tclass()) {
      // Even though the traffic class can be encoded in a single byte, Linux returns it as an
      // `int` when it is received as a control message.
      // https://github.com/torvalds/linux/blob/7e57714cd0a/include/net/ipv6.h#L968
      const int tclass = static_cast<int>(control_data.tclass());
      total += StoreControlMessage(IPPROTO_IPV6, IPV6_TCLASS, &tclass, sizeof(tclass));
    }

    if (requested.ipv6_hoplimit() && control_data.has_hoplimit()) {
      // Even though the hop limit can be encoded in a single byte, Linux returns it as an `int`
      // when it is received as a control message.
      // https://github.com/torvalds/linux/blob/7e57714cd0a/net/ipv6/datagram.c#L622
      const int hoplimit = static_cast<int>(control_data.hoplimit());
      total += StoreControlMessage(IPPROTO_IPV6, IPV6_HOPLIMIT, &hoplimit, sizeof(hoplimit));
    }

    if (requested.ipv6_pktinfo() && control_data.has_pktinfo()) {
      const fsocket::wire::Ipv6PktInfoRecvControlData& fidl_pktinfo = control_data.pktinfo();
      in6_pktinfo pktinfo = {
          .ipi6_ifindex = static_cast<unsigned int>(fidl_pktinfo.iface),
      };
      static_assert(
          sizeof(pktinfo.ipi6_addr) == decltype(fidl_pktinfo.header_destination_addr.addr)::size(),
          "mismatch between size of FIDL and in6_pktinfo IPv6 addresses");
      memcpy(&pktinfo.ipi6_addr, fidl_pktinfo.header_destination_addr.addr.data(),
             sizeof(pktinfo.ipi6_addr));
      total += StoreControlMessage(IPPROTO_IPV6, IPV6_PKTINFO, &pktinfo, sizeof(pktinfo));
    }
    return total;
  }

  socklen_t StoreControlMessage(int level, int type, const void* data, socklen_t len) {
    socklen_t cmsg_len = CMSG_LEN(len);
    size_t bytes_left = buffer_.size();
    if (bytes_left < cmsg_len) {
      // Not enough space to store the entire control message.
      // TODO(https://fxbug.dev/42167124): Add support for truncated control messages (MSG_CTRUNC).
      return 0;
    }

    // The user-provided pointer is not guaranteed to be aligned. So instead of casting it into
    // a struct cmsghdr and writing to it directly, stack-allocate one and then copy it.
    cmsghdr cmsg = {
        .cmsg_len = cmsg_len,
        .cmsg_level = level,
        .cmsg_type = type,
    };
    unsigned char* buf = buffer_.data();
    ZX_ASSERT_MSG(CMSG_DATA(buf) + len <= buf + bytes_left,
                  "buffer would overflow, %p + %x > %p + %zx", CMSG_DATA(buf), len, buf,
                  bytes_left);
    memcpy(buf, &cmsg, sizeof(cmsg));
    memcpy(CMSG_DATA(buf), data, len);
    size_t bytes_consumed = std::min(CMSG_SPACE(len), bytes_left);
    buffer_ = buffer_.subspan(bytes_consumed);

    return static_cast<socklen_t>(bytes_consumed);
  }
  cpp20::span<unsigned char> buffer_;
};

int16_t ParseSocketLevelControlMessage(
    fidl::WireTableBuilder<fsocket::wire::SocketSendControlData>& fidl_socket, int type,
    const void* data, socklen_t len) {
  // TODO(https://fxbug.dev/42170274): Validate unsupported SOL_SOCKET control messages.
  return 0;
}

int16_t ParseIpLevelControlMessage(
    fidl::WireTableBuilder<fsocket::wire::IpSendControlData>& fidl_ip, int type, const void* data,
    socklen_t len) {
  switch (type) {
    case IP_TTL: {
      int ttl;
      if (len != CMSG_LEN(sizeof(ttl))) {
        return EINVAL;
      }
      memcpy(&ttl, data, sizeof(ttl));
      if (ttl < 0 || ttl > std::numeric_limits<uint8_t>::max()) {
        return EINVAL;
      }
      // N.B. This extra validation is performed here in the client since the payload
      // might be processed by the Netstack asynchronously.
      //
      // See: https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0109_socket_datagram_socket
      if (ttl == 0) {
        return EINVAL;
      }
      fidl_ip.ttl(static_cast<uint8_t>(ttl));
      return 0;
    }
    default:
      // TODO(https://fxbug.dev/42170274): Validate unsupported SOL_IP control messages.
      return 0;
  }
}

int16_t ParseIpv6LevelControlMessage(
    fidl::WireTableBuilder<fsocket::wire::Ipv6SendControlData>& fidl_ipv6, int type,
    const void* data, socklen_t data_len) {
  switch (type) {
    case IPV6_HOPLIMIT: {
      int hoplimit;
      if (data_len != CMSG_LEN(sizeof(hoplimit))) {
        return EINVAL;
      }
      memcpy(&hoplimit, data, sizeof(hoplimit));
      if (hoplimit < -1 || hoplimit > std::numeric_limits<uint8_t>::max()) {
        return EINVAL;
      }
      // Ignore hoplimit if it's -1 as it it is interpreted as if the cmsg was not present.
      //
      // https://github.com/torvalds/linux/blob/eaa54b1458c/net/ipv6/udp.c#L1531
      if (hoplimit != -1) {
        fidl_ipv6.hoplimit(static_cast<uint8_t>(hoplimit));
      }
      return 0;
    }
    case IPV6_PKTINFO: {
      in6_pktinfo pktinfo;
      if (data_len != CMSG_LEN(sizeof(pktinfo))) {
        return EINVAL;
      }
      memcpy(&pktinfo, data, sizeof(pktinfo));
      fsocket::wire::Ipv6PktInfoSendControlData fidl_pktinfo{
          .iface = static_cast<uint64_t>(pktinfo.ipi6_ifindex),
      };
      static_assert(sizeof(pktinfo.ipi6_addr) == sizeof(fidl_pktinfo.local_addr.addr),
                    "mismatch between size of FIDL and in6_pktinfo IPv6 addresses");
      memcpy(fidl_pktinfo.local_addr.addr.data(), &pktinfo.ipi6_addr, sizeof(pktinfo.ipi6_addr));
      fidl_ipv6.pktinfo(fidl_pktinfo);
      return 0;
    }
    default:
      // TODO(https://fxbug.dev/42170274): Validate unsupported SOL_IPV6 control messages.
      return 0;
  }
}

template <typename F>
int16_t ParseMultipleControlMessages(fidl::AnyArena& allocator, const struct msghdr& msg,
                                     F parse_control_message) {
  if (msg.msg_control == nullptr && msg.msg_controllen != 0) {
    return static_cast<int16_t>(EFAULT);
  }

  socklen_t total_cmsg_len = 0;
  for (cmsghdr* cmsg = CMSG_FIRSTHDR(&msg); cmsg != nullptr; cmsg = CMSG_NXTHDR(&msg, cmsg)) {
    const cmsghdr& cmsg_ref = *cmsg;
    total_cmsg_len += cmsg_ref.cmsg_len;

    // Validate the header length.
    // https://github.com/torvalds/linux/blob/42eb8fdac2f/include/linux/socket.h#L119-L122
    if (msg.msg_controllen < total_cmsg_len || cmsg_ref.cmsg_len < sizeof(cmsghdr)) {
      return static_cast<int16_t>(EINVAL);
    }

    int16_t err = parse_control_message(allocator, cmsg_ref);
    if (err != 0) {
      return err;
    }
  }
  return 0;
}

fit::result<int16_t, fsocket::wire::NetworkSocketSendControlData> ParseNetworkSocketSendControlData(
    fidl::AnyArena& allocator, const struct msghdr& msg) {
  fidl::WireTableBuilder fidl_socket = fsocket::wire::SocketSendControlData::Builder(allocator);
  fidl::WireTableBuilder fidl_ip = fsocket::wire::IpSendControlData::Builder(allocator);
  fidl::WireTableBuilder fidl_ipv6 = fsocket::wire::Ipv6SendControlData::Builder(allocator);
  int16_t err = ParseMultipleControlMessages(
      allocator, msg,
      [&fidl_socket, &fidl_ip, &fidl_ipv6](fidl::AnyArena& allocator,
                                           const cmsghdr& cmsg) -> int16_t {
        switch (cmsg.cmsg_level) {
          case SOL_SOCKET:
            return ParseSocketLevelControlMessage(fidl_socket, cmsg.cmsg_type, CMSG_DATA(&cmsg),
                                                  cmsg.cmsg_len);
          case SOL_IP:
            return ParseIpLevelControlMessage(fidl_ip, cmsg.cmsg_type, CMSG_DATA(&cmsg),
                                              cmsg.cmsg_len);
          case SOL_IPV6:
            return ParseIpv6LevelControlMessage(fidl_ipv6, cmsg.cmsg_type, CMSG_DATA(&cmsg),
                                                cmsg.cmsg_len);
          default:
            return 0;
        }
      });

  if (err != 0) {
    return fit::error(err);
  }

  return fit::success(fsocket::wire::NetworkSocketSendControlData::Builder(allocator)
                          .socket(fidl_socket.Build())
                          .ip(fidl_ip.Build())
                          .ipv6(fidl_ipv6.Build())
                          .Build());
}

template <typename T>
fit::result<int16_t, T> ParseControlMessages(fidl::AnyArena& allocator, const struct msghdr& msg);

template <>
fit::result<int16_t, fuchsia_posix_socket::wire::DatagramSocketSendControlData>
ParseControlMessages<fuchsia_posix_socket::wire::DatagramSocketSendControlData>(
    fidl::AnyArena& allocator, const struct msghdr& msg) {
  fit::result fidl_net = ParseNetworkSocketSendControlData(allocator, msg);
  if (fidl_net.is_error()) {
    return fidl_net.take_error();
  }

  return fit::success(fuchsia_posix_socket::wire::DatagramSocketSendControlData::Builder(allocator)
                          .network(fidl_net.value())
                          .Build());
}

template <>
fit::result<int16_t, fuchsia_posix_socket::wire::NetworkSocketSendControlData>
ParseControlMessages<fuchsia_posix_socket::wire::NetworkSocketSendControlData>(
    fidl::AnyArena& allocator, const struct msghdr& msg) {
  return ParseNetworkSocketSendControlData(allocator, msg);
}

fit::result<int16_t, fsocket::wire::SocketSendControlData> ParseSocketSendControlData(
    fidl::AnyArena& allocator, const struct msghdr& msg) {
  fidl::WireTableBuilder fidl_socket = fsocket::wire::SocketSendControlData::Builder(allocator);
  int16_t err = ParseMultipleControlMessages(
      allocator, msg, [&fidl_socket](fidl::AnyArena& allocator, const cmsghdr& cmsg) -> int16_t {
        switch (cmsg.cmsg_level) {
          case SOL_SOCKET:
            return ParseSocketLevelControlMessage(fidl_socket, cmsg.cmsg_type, CMSG_DATA(&cmsg),
                                                  cmsg.cmsg_len);
          default:
            return 0;
        }
      });

  if (err != 0) {
    return fit::error(err);
  }

  return fit::success(fidl_socket.Build());
}

template <>
fit::result<int16_t, fuchsia_posix_socket::wire::SocketSendControlData>
ParseControlMessages<fuchsia_posix_socket::wire::SocketSendControlData>(fidl::AnyArena& allocator,
                                                                        const struct msghdr& msg) {
  return ParseSocketSendControlData(allocator, msg);
}

template <>
fit::result<int16_t, fuchsia_posix_socket_packet::wire::SendControlData>
ParseControlMessages<fuchsia_posix_socket_packet::wire::SendControlData>(fidl::AnyArena& allocator,
                                                                         const struct msghdr& msg) {
  fit::result fidl_socket = ParseSocketSendControlData(allocator, msg);
  if (fidl_socket.is_error()) {
    return fidl_socket.take_error();
  }

  return fit::success(fuchsia_posix_socket_packet::wire::SendControlData::Builder(allocator)
                          .socket(fidl_socket.value())
                          .Build());
}

template <typename R, typename = int>
struct FitResultHasValue : std::false_type {};
template <typename R>
struct FitResultHasValue<R, decltype(&R::value, 0)> : std::true_type {};
template <typename T, typename R>
typename std::enable_if<FitResultHasValue<R>::value>::type HandleSendMsgResponse(const R& result,
                                                                                 size_t total) {
  T::handle_sendmsg_response(*result->value(), total);
}
template <typename T, typename R>
typename std::enable_if<!FitResultHasValue<T>::value>::type HandleSendMsgResponse(const R& result,
                                                                                  size_t total) {}

template <typename T, typename = std::enable_if_t<std::is_same_v<T, SynchronousDatagramSocket> ||
                                                  std::is_same_v<T, RawSocket> ||
                                                  std::is_same_v<T, PacketSocket>>>
struct socket_with_event {
 public:
  explicit socket_with_event(typename T::Storage& zxio_storage)
      : client_(zxio_storage.client), event_(zxio_storage.event) {}

  void wait_begin(zxio_signals_t zxio_signals, zx_handle_t* handle, zx_signals_t* out_signals) {
    *handle = event_.get();

    zx_signals_t signals = ZX_EVENTPAIR_PEER_CLOSED | fsocket::wire::kSignalDatagramError;
    if (zxio_signals & ZXIO_SIGNAL_READABLE) {
      signals |=
          fsocket::wire::kSignalDatagramIncoming | fsocket::wire::kSignalDatagramShutdownRead;
    }
    if (zxio_signals & ZXIO_SIGNAL_WRITABLE) {
      signals |=
          fsocket::wire::kSignalDatagramOutgoing | fsocket::wire::kSignalDatagramShutdownWrite;
    }
    if (zxio_signals & ZXIO_SIGNAL_READ_DISABLED) {
      signals |= fsocket::wire::kSignalDatagramShutdownRead;
    }
    *out_signals = signals;
  }

  void wait_end(zx_signals_t signals, zxio_signals_t* out_zxio_signals) {
    zxio_signals_t zxio_signals = 0;
    if (signals & (ZX_EVENTPAIR_PEER_CLOSED | fsocket::wire::kSignalDatagramIncoming |
                   fsocket::wire::kSignalDatagramShutdownRead)) {
      zxio_signals |= ZXIO_SIGNAL_READABLE;
    }
    if (signals & (ZX_EVENTPAIR_PEER_CLOSED | fsocket::wire::kSignalDatagramOutgoing |
                   fsocket::wire::kSignalDatagramShutdownWrite)) {
      zxio_signals |= ZXIO_SIGNAL_WRITABLE;
    }
    if (signals & (ZX_EVENTPAIR_PEER_CLOSED | fsocket::wire::kSignalDatagramError)) {
      zxio_signals |= ZXIO_SIGNAL_ERROR;
    }
    if (signals & (ZX_EVENTPAIR_PEER_CLOSED | fsocket::wire::kSignalDatagramShutdownRead)) {
      zxio_signals |= ZXIO_SIGNAL_READ_DISABLED;
    }
    *out_zxio_signals = zxio_signals;
  }

  zx_status_t recvmsg(struct msghdr* msg, int flags, size_t* out_actual, int16_t* out_code) {
    size_t datalen = 0;
    for (int i = 0; i < msg->msg_iovlen; ++i) {
      datalen += msg->msg_iov[i].iov_len;
    }

    bool want_addr = msg->msg_namelen != 0 && msg->msg_name != nullptr;
    bool want_cmsg = msg->msg_controllen != 0 && msg->msg_control != nullptr;
    auto response = client_->RecvMsg(want_addr, static_cast<uint32_t>(datalen), want_cmsg,
                                     to_recvmsg_flags(flags));
    zx_status_t status = response.status();
    if (status != ZX_OK) {
      return status;
    }
    auto const& result = response.value();
    if (result.is_error()) {
      *out_code = static_cast<int16_t>(result.error_value());
      return ZX_OK;
    }
    *out_code = 0;

    T::recvmsg_populate_msgname(*result.value(), msg->msg_name, msg->msg_namelen);

    {
      auto const& out = result.value()->data;

      const uint8_t* data = out.begin();
      size_t remaining = out.size();
      for (int i = 0; remaining != 0 && i < msg->msg_iovlen; ++i) {
        iovec const& iov = msg->msg_iov[i];
        if (iov.iov_base != nullptr) {
          size_t actual = std::min(iov.iov_len, remaining);
          if (unlikely(!zxio_maybe_faultable_copy(static_cast<uint8_t*>(iov.iov_base), data, actual,
                                                  true))) {
            *out_code = EFAULT;
            return ZX_OK;
          }

          data += actual;
          remaining -= actual;
        } else if (iov.iov_len != 0) {
          *out_code = EFAULT;
          return ZX_OK;
        }
      }
      *out_actual = set_trunc_flags_and_return_out_actual(*msg, out.size() - remaining,
                                                          result.value()->truncated, flags);
    }

    if (want_cmsg) {
      FidlControlDataProcessor proc(msg->msg_control, msg->msg_controllen);
      // The synchronous datagram protocol returns all control messages found in the FIDL
      // response. This behavior is implemented using a "filter" that allows everything
      // through.
      msg->msg_controllen =
          proc.Store(result.value()->control, RequestedCmsgSet::AllRequestedCmsgSet());
    } else {
      msg->msg_controllen = 0;
    }

    return ZX_OK;
  }

  zx_status_t sendmsg(const struct msghdr* msg, int flags, size_t* out_actual, int16_t* out_code) {
    // TODO(https://fxbug.dev/42061949) Add tests with msg as nullptr.
    if (msg == nullptr) {
      *out_code = EFAULT;
      return ZX_OK;
    }
    const msghdr& msghdr_ref = *msg;
    typename T::FidlSockAddr addr;
    // Attempt to load socket address if either name or namelen is set.
    // If only one is set, it'll result in INVALID_ARGS.
    if (msghdr_ref.msg_namelen != 0 || msghdr_ref.msg_name != nullptr) {
      zx_status_t status =
          addr.LoadSockAddr(static_cast<struct sockaddr*>(msg->msg_name), msg->msg_namelen);
      if (status != ZX_OK) {
        return status;
      }
    }

    std::optional opt_total = total_iov_len(*msg);
    if (!opt_total.has_value()) {
      *out_code = EFAULT;
      return ZX_OK;
    }
    size_t total = opt_total.value();

    fidl::Arena allocator;
    fit::result cmsg_result =
        ParseControlMessages<typename T::FidlSendControlData>(allocator, msghdr_ref);
    if (cmsg_result.is_error()) {
      *out_code = cmsg_result.error_value();
      return ZX_OK;
    }
    const typename T::FidlSendControlData& cdata = cmsg_result.value();

    std::unique_ptr<uint8_t[]> data;
    auto vec = fidl::VectorView<uint8_t>();
    switch (msg->msg_iovlen) {
      case 0: {
        break;
      }
      case 1: {
        if (zxio_fault_catching_disabled()) {
          const iovec& iov = *msg->msg_iov;
          vec = fidl::VectorView<uint8_t>::FromExternal(static_cast<uint8_t*>(iov.iov_base),
                                                        iov.iov_len);
          break;
        }

        // We reach here if the consumer of zxio expects faults to occur when
        // accessing the message's paylod. We need to catch the fault now so
        // that it can be gracefully handled instead of triggering a crash later
        // on.
        //
        // TODO(https://fxbug.dev/42165811): avoid this copy to catch faults.
        __FALLTHROUGH;
      }
      default: {
        // TODO(https://fxbug.dev/42165811): avoid this copy to linearize the buffer.
        data = std::unique_ptr<uint8_t[]>(new uint8_t[total]);
        uint8_t* dest = data.get();

        for (int i = 0; i < msg->msg_iovlen; ++i) {
          const iovec& iov = msg->msg_iov[i];
          if (unlikely(!zxio_maybe_faultable_copy(dest, static_cast<const uint8_t*>(iov.iov_base),
                                                  iov.iov_len, false))) {
            *out_code = EFAULT;
            return ZX_OK;
          }
          dest += iov.iov_len;
        }
        vec = fidl::VectorView<uint8_t>::FromExternal(data.get(), total);
      }
    }

    // TODO(https://fxbug.dev/42136468): Use better representation of nullable union when
    // available. Currently just using a default-initialized union with an invalid tag.
    auto response = addr.WithFIDL([&](auto address) {
      return client_->SendMsg(address, vec, cdata, to_sendmsg_flags(flags));
    });
    zx_status_t status = response.status();
    if (status != ZX_OK) {
      return status;
    }
    auto const& result = response.value();
    if (result.is_error()) {
      *out_code = static_cast<int16_t>(result.error_value());
      return ZX_OK;
    }
    HandleSendMsgResponse<T, decltype(result)>(result, total);

    *out_code = 0;
    // SendMsg does not perform partial writes.
    *out_actual = total;
    return ZX_OK;
  }

 private:
  typename fidl::WireSyncClient<typename T::Storage::FidlProtocol>& client_;
  zx::eventpair& event_;
};

}  // namespace

static constexpr zxio_ops_t zxio_default_socket_ops = []() {
  zxio_ops_t ops = zxio_default_ops;
  ops.connect = [](zxio_t* io, const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
    *out_code = EOPNOTSUPP;
    return ZX_OK;
  };
  ops.listen = [](zxio_t* io, int backlog, int16_t* out_code) {
    *out_code = EOPNOTSUPP;
    return ZX_OK;
  };
  ops.accept = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen,
                  zxio_storage_t* out_storage, int16_t* out_code) {
    *out_code = EOPNOTSUPP;
    return ZX_OK;
  };
  ops.getpeername = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    *out_code = EOPNOTSUPP;
    return ZX_OK;
  };
  ops.shutdown = [](zxio_t* io, zxio_shutdown_options_t options, int16_t* out_code) {
    *out_code = EOPNOTSUPP;
    return ZX_OK;
  };
  return ops;
}();

static SynchronousDatagramSocket::Storage& zxio_synchronous_datagram_socket(zxio_t* io) {
  return *reinterpret_cast<SynchronousDatagramSocket::Storage*>(io);
}

static constexpr zxio_ops_t zxio_synchronous_datagram_socket_ops = []() {
  zxio_ops_t ops = zxio_default_socket_ops;
  ops.attr_get = AttrGet<ZXIO_OBJECT_TYPE_SYNCHRONOUS_DATAGRAM_SOCKET>;
  ops.destroy = [](zxio_t* io) {
    SynchronousDatagramSocket::Storage& zs = zxio_synchronous_datagram_socket(io);
    zs.~Storage();
  };
  ops.close = [](zxio_t* io) {
    SynchronousDatagramSocket::Storage& zs = zxio_synchronous_datagram_socket(io);
    zx_status_t status = ZX_OK;
    if (zs.client.is_valid()) {
      status = base_socket(zs.client).CloseSocket();
    }
    return status;
  };
  ops.release = [](zxio_t* io, zx_handle_t* out_handle) {
    if (out_handle == nullptr) {
      return ZX_ERR_INVALID_ARGS;
    }
    *out_handle =
        zxio_synchronous_datagram_socket(io).client.TakeClientEnd().TakeChannel().release();
    return ZX_OK;
  };
  ops.borrow = [](zxio_t* io, zx_handle_t* out_handle) {
    *out_handle =
        zxio_synchronous_datagram_socket(io).client.client_end().borrow().channel()->get();
    return ZX_OK;
  };
  ops.clone = [](zxio_t* io, zx_handle_t* out_handle) {
    SynchronousDatagramSocket::Storage& zs = zxio_synchronous_datagram_socket(io);
    zx_status_t status = base_socket(zs.client).CloneSocket(out_handle);
    return status;
  };
  ops.bind = [](zxio_t* io, const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
    return network_socket(zxio_synchronous_datagram_socket(io).client)
        .bind(addr, addrlen, out_code);
  };
  ops.connect = [](zxio_t* io, const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
    return network_socket(zxio_synchronous_datagram_socket(io).client)
        .connect(addr, addrlen, out_code);
  };
  ops.getsockname = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    return network_socket(zxio_synchronous_datagram_socket(io).client)
        .getsockname(addr, addrlen, out_code);
  };
  ops.getpeername = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    return network_socket(zxio_synchronous_datagram_socket(io).client)
        .getpeername(addr, addrlen, out_code);
  };
  ops.getsockopt = [](zxio_t* io, int level, int optname, void* optval, socklen_t* optlen,
                      int16_t* out_code) {
    SockOptResult result = network_socket(zxio_synchronous_datagram_socket(io).client)
                               .getsockopt_fidl(level, optname, optval, optlen);
    *out_code = result.err;
    return result.status;
  };
  ops.setsockopt = [](zxio_t* io, int level, int optname, const void* optval, socklen_t optlen,
                      zx_handle_t access_token, int16_t* out_code) {
    auto& socket = zxio_synchronous_datagram_socket(io);
    SockOptResult result =
        network_socket(socket.client)
            .setsockopt_fidl(level, optname, optval, optlen, zx::event(access_token), io);
    *out_code = result.err;
    return result.status;
  };
  ops.recvmsg = [](zxio_t* io, struct msghdr* msg, int flags, size_t* out_actual,
                   int16_t* out_code) {
    return socket_with_event<SynchronousDatagramSocket>(zxio_synchronous_datagram_socket(io))
        .recvmsg(msg, flags, out_actual, out_code);
  };
  ops.sendmsg = [](zxio_t* io, const struct msghdr* msg, int flags, size_t* out_actual,
                   int16_t* out_code) {
    return socket_with_event<SynchronousDatagramSocket>(zxio_synchronous_datagram_socket(io))
        .sendmsg(msg, flags, out_actual, out_code);
  };

  ops.shutdown = [](zxio_t* io, zxio_shutdown_options_t options, int16_t* out_code) {
    return network_socket(zxio_synchronous_datagram_socket(io).client).shutdown(options, out_code);
  };
  ops.wait_begin = [](zxio_t* io, zxio_signals_t zxio_signals, zx_handle_t* handle,
                      zx_signals_t* out_signals) {
    return socket_with_event<SynchronousDatagramSocket>(zxio_synchronous_datagram_socket(io))
        .wait_begin(zxio_signals, handle, out_signals);
  };
  ops.wait_end = [](zxio_t* io, zx_signals_t zx_signals, zxio_signals_t* out_zxio_signals) {
    return socket_with_event<SynchronousDatagramSocket>(zxio_synchronous_datagram_socket(io))
        .wait_end(zx_signals, out_zxio_signals);
  };
  return ops;
}();

zx_status_t zxio_synchronous_datagram_socket_init(
    zxio_storage_t* storage, zx::eventpair event,
    fidl::ClientEnd<fsocket::SynchronousDatagramSocket> client) {
  auto zs = new (storage) SynchronousDatagramSocket::Storage{
      .io = storage->io,
      .event = std::move(event),
      .client = fidl::WireSyncClient(std::move(client)),
  };
  zxio_init(&zs->io, &zxio_synchronous_datagram_socket_ops);
  return ZX_OK;
}

namespace {

template <typename Client,
          typename = std::enable_if_t<
              std::is_same_v<Client, fidl::WireSyncClient<fsocket::StreamSocket>> ||
              std::is_same_v<Client, fidl::WireSyncClient<fsocket::DatagramSocket>>>>
class socket_with_zx_socket {
 public:
  explicit socket_with_zx_socket(Client& client) : client_(client) {}

 protected:
  Client& client() { return client_; }

  virtual ErrOrOutCode GetError() = 0;

  std::optional<ErrOrOutCode> GetZxSocketWriteError(zx_status_t status) {
    switch (status) {
      case ZX_OK:
        return std::nullopt;
      case ZX_ERR_INVALID_ARGS:
        return zx::ok(static_cast<int16_t>(EFAULT));
      case ZX_ERR_BAD_STATE:
        __FALLTHROUGH;
      case ZX_ERR_PEER_CLOSED: {
        zx::result err = GetError();
        if (err.is_error()) {
          return zx::error(err.status_value());
        }
        if (int value = err.value(); value != 0) {
          return zx::ok(static_cast<int16_t>(value));
        }
        // Error was consumed.
        return zx::ok(static_cast<int16_t>(EPIPE));
      }
      default:
        return zx::error(status);
    }
  }

  virtual std::optional<ErrOrOutCode> GetZxSocketReadError(zx_status_t status) {
    switch (status) {
      case ZX_OK:
        return std::nullopt;
      case ZX_ERR_INVALID_ARGS:
        return zx::ok(static_cast<int16_t>(EFAULT));
      case ZX_ERR_BAD_STATE:
        __FALLTHROUGH;
      case ZX_ERR_PEER_CLOSED: {
        zx::result err = GetError();
        if (err.is_error()) {
          return zx::error(err.status_value());
        }
        return zx::ok(static_cast<int16_t>(err.value()));
      }
      default:
        return zx::error(status);
    }
  }

 private:
  Client& client_;
};

// A |zxio_t| backend that uses a fuchsia.posix.socket.DatagramSocket object.
using zxio_datagram_socket_t = struct zxio_datagram_socket {
  zxio_t io;
  zxio_pipe_t pipe;
  const zxio_datagram_prelude_size_t prelude_size;
  RouteCache route_cache;
  RequestedCmsgCache cmsg_cache;
  fidl::WireSyncClient<fuchsia_posix_socket::DatagramSocket> client;
};

static_assert(sizeof(zxio_datagram_socket_t) <= sizeof(zxio_storage_t),
              "zxio_datagram_socket_t must fit inside zxio_storage_t.");

static zxio_datagram_socket_t& zxio_datagram_socket(zxio_t* io) {
  return *reinterpret_cast<zxio_datagram_socket_t*>(io);
}

struct datagram_socket
    : public socket_with_zx_socket<fidl::WireSyncClient<fsocket::DatagramSocket>> {
 public:
  explicit datagram_socket(zxio_datagram_socket_t& datagram_socket)
      : socket_with_zx_socket(datagram_socket.client), datagram_socket_(datagram_socket) {}

  void wait_begin(zxio_signals_t zxio_signals, zx_handle_t* handle, zx_signals_t* out_signals) {
    zxio_signals |= ZXIO_SIGNAL_PEER_CLOSED;
    // Translate the `WRITABLE` signal to `WRITE_THRESHOLD`, so that any caller
    // waiting for the socket to become writable will actually wait for capacity
    // to reach the socket's write threshold, which is set by the netstack to
    // the maximum size of a payload. This allows callers to avoid spuriously
    // retrying writes when the outgoing payload is larger than the remaining
    // capacity in the socket.
    if (zxio_signals & ZXIO_SIGNAL_WRITABLE) {
      zxio_signals &= ~ZXIO_SIGNAL_WRITABLE;
      zxio_signals |= ZXIO_SIGNAL_WRITE_THRESHOLD;
    }
    zxio_wait_begin(&datagram_socket_.pipe.io, zxio_signals, handle, out_signals);
    *out_signals |= fsocket::wire::kSignalDatagramError;
  }

  void wait_end(zx_signals_t zx_signals, zxio_signals_t* out_zxio_signals) {
    zxio_signals_t zxio_signals;
    zxio_wait_end(&datagram_socket_.pipe.io, zx_signals, &zxio_signals);
    // Translate the `WRITE_THRESHOLD` signal to `WRITABLE`. See `wait_begin`
    // for why we do this.
    //
    // We don't mask out the `WRITE_THRESHOLD` signal because it's possible that
    // the client actually waited on `WRITE_THRESHOLD` directly rather than
    // `WRITABLE`, and there is no way to know here if that was the case. Note
    // that this means that a client may see `WRITE_THRESHOLD` reported even if
    // it did not provide it in `wait_begin`.
    if (zxio_signals & ZXIO_SIGNAL_WRITE_THRESHOLD) {
      zxio_signals |= ZXIO_SIGNAL_WRITABLE;
    } else {
      // Unconditionally mask out `WRITABLE` to avoid signaling it to the
      // client.
      //
      // If `WRITABLE` (but not `WRITE_THRESHOLD`) is asserted on the underlying
      // object and the wait completes for some other reason, and this signal is
      // propagated to the client, they may attempt a write that will not
      // succeed.
      zxio_signals &= ~ZXIO_SIGNAL_WRITABLE;
    }
    if (zx_signals & fsocket::wire::kSignalDatagramError) {
      zxio_signals |= ZXIO_SIGNAL_ERROR;
    }
    *out_zxio_signals = zxio_signals;
  }

  std::optional<ErrOrOutCode> GetZxSocketReadError(zx_status_t status) override {
    switch (status) {
      case ZX_ERR_BAD_STATE:
        // Datagram sockets return EAGAIN when a socket is read from after shutdown,
        // whereas stream sockets return zero bytes. Enforce this behavior here.
        return zx::ok(static_cast<int16_t>(EAGAIN));
      default:
        return socket_with_zx_socket::GetZxSocketReadError(status);
    }
  }

  zx_status_t recvmsg(struct msghdr* msg, int flags, size_t* out_actual, int16_t* out_code) {
    // Before reading from the socket, we need to check for asynchronous
    // errors. Here, we combine this check with a cache lookup for the
    // requested control message set; when cmsgs are requested, this lets us
    // save a syscall.
    bool cmsg_requested = msg->msg_controllen != 0 && msg->msg_control != nullptr;
    RequestedCmsgCache::Result cache_result = datagram_socket_.cmsg_cache.Get(
        socket_err_wait_item(), cmsg_requested, datagram_socket_.client);
    if (!cache_result.is_ok()) {
      ErrOrOutCode err_value = cache_result.error_value();
      if (err_value.is_error()) {
        return err_value.status_value();
      }
      *out_code = err_value.value();
      return ZX_OK;
    }
    std::optional<RequestedCmsgSet> requested_cmsg_set = cache_result.value();

    zxio_flags_t zxio_flags = 0;
    if (flags & MSG_PEEK) {
      zxio_flags |= ZXIO_PEEK;
    }

    // Use stack allocated memory whenever the client-versioned `kRxUdpPreludeSize` is
    // at least as large as the server's.
    std::unique_ptr<uint8_t[]> heap_allocated_buf;
    uint8_t stack_allocated_buf[zxio::kRxUdpPreludeSize];
    uint8_t* buf = stack_allocated_buf;
    if (datagram_socket_.prelude_size.rx > zxio::kRxUdpPreludeSize) {
      heap_allocated_buf = std::make_unique<uint8_t[]>(datagram_socket_.prelude_size.rx);
      buf = heap_allocated_buf.get();
    }

    zx_iovec_t zx_iov[msg->msg_iovlen + 1];
    zx_iov[0] = {
        .buffer = buf,
        .capacity = datagram_socket_.prelude_size.rx,
    };

    size_t zx_iov_idx = 1;
    std::optional<size_t> fault_idx;
    {
      size_t idx = 0;
      for (int i = 0; i < msg->msg_iovlen; ++i) {
        iovec const& iov = msg->msg_iov[i];
        if (iov.iov_base != nullptr) {
          zx_iov[zx_iov_idx] = {
              .buffer = iov.iov_base,
              .capacity = iov.iov_len,
          };
          zx_iov_idx++;
          idx += iov.iov_len;
        } else if (iov.iov_len != 0) {
          fault_idx = idx;
          break;
        }
      }
    }

    size_t count_bytes_read;
    std::optional read_error = GetZxSocketReadError(
        zxio_readv(&datagram_socket_.io, zx_iov, zx_iov_idx, zxio_flags, &count_bytes_read));
    if (read_error.has_value()) {
      zx::result err = read_error.value();
      if (!err.is_error()) {
        if (err.value() == 0) {
          *out_actual = 0;
        }
        *out_code = err.value();
      }
      return err.status_value();
    }

    if (count_bytes_read < datagram_socket_.prelude_size.rx) {
      *out_code = EIO;
      return ZX_OK;
    }

    fit::result decoded_meta =
        deserialize_recv_msg_meta(cpp20::span<uint8_t>(buf, datagram_socket_.prelude_size.rx));

    if (!decoded_meta.is_ok()) {
      *out_code = EIO;
      return ZX_OK;
    }

    const fuchsia_posix_socket::wire::RecvMsgMeta& meta = *decoded_meta.value();

    if (msg->msg_namelen != 0 && msg->msg_name != nullptr) {
      if (!meta.has_from()) {
        *out_code = EIO;
        return ZX_OK;
      }
      msg->msg_namelen = static_cast<socklen_t>(fidl_to_sockaddr(
          meta.from(), static_cast<struct sockaddr*>(msg->msg_name), msg->msg_namelen));
    }

    size_t payload_bytes_read = count_bytes_read - datagram_socket_.prelude_size.rx;
    if (payload_bytes_read > meta.payload_len()) {
      *out_code = EIO;
      return ZX_OK;
    }
    if (fault_idx.has_value() && meta.payload_len() > fault_idx.value()) {
      *out_code = EFAULT;
      return ZX_OK;
    }

    size_t truncated =
        meta.payload_len() > payload_bytes_read ? meta.payload_len() - payload_bytes_read : 0;
    *out_actual = set_trunc_flags_and_return_out_actual(*msg, payload_bytes_read, truncated, flags);

    if (cmsg_requested) {
      FidlControlDataProcessor proc(msg->msg_control, msg->msg_controllen);
      ZX_ASSERT_MSG(cmsg_requested == requested_cmsg_set.has_value(),
                    "cache lookup should return the RequestedCmsgSet iff it was requested");
      msg->msg_controllen = proc.Store(meta.control(), requested_cmsg_set.value());
    } else {
      msg->msg_controllen = 0;
    }

    *out_code = 0;
    return ZX_OK;
  }

  zx_status_t sendmsg(const struct msghdr* msg, int flags, size_t* out_actual, int16_t* out_code) {
    // TODO(https://fxbug.dev/42061949) Add tests with msg as nullptr.
    if (msg == nullptr) {
      *out_code = EFAULT;
      return ZX_OK;
    }
    const msghdr& msghdr_ref = *msg;
    std::optional opt_total = total_iov_len(msghdr_ref);
    if (!opt_total.has_value()) {
      *out_code = EFAULT;
      return ZX_OK;
    }
    size_t total = opt_total.value();

    std::optional<SocketAddress> remote_addr;
    // Attempt to load socket address if either name or namelen is set.
    // If only one is set, it'll result in INVALID_ARGS.
    if (msg->msg_namelen != 0 || msg->msg_name != nullptr) {
      zx_status_t status = remote_addr.emplace().LoadSockAddr(
          static_cast<struct sockaddr*>(msg->msg_name), msg->msg_namelen);
      if (status != ZX_OK) {
        return status;
      }
    }

    constexpr size_t kSendControlDataArenaSize =
        fidl::MaxSizeInChannel<fsocket::wire::DatagramSocketSendControlData,
                               fidl::MessageDirection::kSending>();
    // Set a sensible upper limit for how much stack space we're going to allow
    // using here to prevent deep stack usage in zxio/fdio. If this grows to
    // untenable sizes we might have to change strategies here.
    static_assert(kSendControlDataArenaSize <= 192);
    fidl::Arena<kSendControlDataArenaSize> alloc;
    fit::result cmsg_result =
        ParseControlMessages<fsocket::wire::DatagramSocketSendControlData>(alloc, msghdr_ref);
    if (cmsg_result.is_error()) {
      *out_code = cmsg_result.error_value();
      return ZX_OK;
    }
    const fsocket::wire::DatagramSocketSendControlData& cdata = cmsg_result.value();
    const std::optional local_iface_and_addr =
        [&cdata]() -> std::optional<std::pair<uint64_t, fuchsia_net::wire::Ipv6Address>> {
      if (!cdata.has_network()) {
        return {};
      }
      const fuchsia_posix_socket::wire::NetworkSocketSendControlData& network = cdata.network();
      if (!network.has_ipv6()) {
        return {};
      }
      const fuchsia_posix_socket::wire::Ipv6SendControlData& ipv6 = network.ipv6();
      if (!ipv6.has_pktinfo()) {
        return {};
      }
      const fuchsia_posix_socket::wire::Ipv6PktInfoSendControlData& pktinfo = ipv6.pktinfo();
      return std::make_pair(pktinfo.iface, pktinfo.local_addr);
    }();

    RouteCache::Result result = datagram_socket_.route_cache.Get(
        remote_addr, local_iface_and_addr, socket_err_wait_item(), datagram_socket_.client);

    if (!result.is_ok()) {
      ErrOrOutCode err_value = result.error_value();
      if (err_value.is_error()) {
        return err_value.status_value();
      }
      *out_code = err_value.value();
      return ZX_OK;
    }

    if (result.value() < total) {
      *out_code = EMSGSIZE;
      return ZX_OK;
    }

    // Use stack allocated memory whenever the client-versioned `kTxUdpPreludeSize` is
    // at least as large as the server's.
    std::unique_ptr<uint8_t[]> heap_allocated_buf;
    uint8_t stack_allocated_buf[zxio::kTxUdpPreludeSize];
    uint8_t* buf = stack_allocated_buf;
    if (datagram_socket_.prelude_size.tx > zxio::kTxUdpPreludeSize) {
      heap_allocated_buf = std::make_unique<uint8_t[]>(datagram_socket_.prelude_size.tx);
      buf = heap_allocated_buf.get();
    }

    auto meta_builder_with_cdata = [&alloc, &cdata]() {
      fidl::WireTableBuilder meta_builder = fuchsia_posix_socket::wire::SendMsgMeta::Builder(alloc);
      meta_builder.control(cdata);
      return meta_builder;
    };

    auto build_and_serialize =
        [this, &buf](fidl::WireTableBuilder<fsocket::wire::SendMsgMeta>& meta_builder) {
          fsocket::wire::SendMsgMeta meta = meta_builder.Build();
          return serialize_send_msg_meta(
              meta, cpp20::span<uint8_t>(buf, datagram_socket_.prelude_size.tx));
        };

    SerializeSendMsgMetaError serialize_err;
    if (remote_addr.has_value()) {
      serialize_err = remote_addr.value().WithFIDL(
          [&build_and_serialize, &meta_builder_with_cdata](fnet::wire::SocketAddress address) {
            fidl::WireTableBuilder meta_builder = meta_builder_with_cdata();
            meta_builder.to(address);
            return build_and_serialize(meta_builder);
          });
    } else {
      fidl::WireTableBuilder meta_builder = meta_builder_with_cdata();
      serialize_err = build_and_serialize(meta_builder);
    }

    if (serialize_err != SerializeSendMsgMetaErrorNone) {
      *out_code = EIO;
      return ZX_OK;
    }

    zx_iovec_t zx_iov[msg->msg_iovlen + 1];
    zx_iov[0] = {
        .buffer = buf,
        .capacity = datagram_socket_.prelude_size.tx,
    };

    size_t zx_iov_idx = 1;
    for (int i = 0; i < msg->msg_iovlen; ++i) {
      iovec const& iov = msg->msg_iov[i];
      if (iov.iov_base != nullptr) {
        zx_iov[zx_iov_idx] = {
            .buffer = iov.iov_base,
            .capacity = iov.iov_len,
        };
        zx_iov_idx++;
      }
    }

    size_t bytes_written;
    std::optional write_error = GetZxSocketWriteError(
        zxio_writev(&datagram_socket_.io, zx_iov, zx_iov_idx, 0, &bytes_written));
    if (write_error.has_value()) {
      zx::result err = write_error.value();
      if (!err.is_error()) {
        *out_code = err.value();
      }
      return err.status_value();
    }

    size_t total_with_prelude = datagram_socket_.prelude_size.tx + total;
    if (bytes_written != total_with_prelude) {
      // Datagram writes should never be short.
      *out_code = EIO;
      return ZX_OK;
    }
    // A successful datagram socket write is never short, so we can assume all bytes
    // were written.
    *out_actual = total;
    *out_code = 0;
    return ZX_OK;
  }

 private:
  zx_wait_item_t socket_err_wait_item() {
    return {
        .handle = datagram_socket_.pipe.socket.get(),
        .waitfor = fsocket::wire::kSignalDatagramError,
    };
  }

  ErrOrOutCode GetError() override {
    std::optional err = GetErrorWithClient(client());
    if (!err.has_value()) {
      return zx::ok(static_cast<int16_t>(0));
    }
    return err.value();
  }

  zxio_datagram_socket_t& datagram_socket_;
};

}  // namespace

static constexpr zxio_ops_t zxio_datagram_socket_ops = []() {
  zxio_ops_t ops = zxio_default_socket_ops;
  ops.attr_get = AttrGet<ZXIO_OBJECT_TYPE_DATAGRAM_SOCKET>;
  ops.destroy = [](zxio_t* io) {
    zxio_datagram_socket_t& zs = zxio_datagram_socket(io);
    zs.~zxio_datagram_socket();
  };
  ops.close = [](zxio_t* io) {
    zxio_datagram_socket_t& zs = zxio_datagram_socket(io);
    zx_status_t status = ZX_OK;
    if (zs.client.is_valid()) {
      status = base_socket(zs.client).CloseSocket();
    }
    return status;
  };
  ops.release = [](zxio_t* io, zx_handle_t* out_handle) {
    if (out_handle == nullptr) {
      return ZX_ERR_INVALID_ARGS;
    }
    *out_handle = zxio_datagram_socket(io).client.TakeClientEnd().TakeChannel().release();
    return ZX_OK;
  };
  ops.borrow = [](zxio_t* io, zx_handle_t* out_handle) {
    *out_handle = zxio_datagram_socket(io).client.client_end().borrow().channel()->get();
    return ZX_OK;
  };
  ops.clone = [](zxio_t* io, zx_handle_t* out_handle) {
    return base_socket(zxio_datagram_socket(io).client).CloneSocket(out_handle);
  };
  ops.wait_begin = [](zxio_t* io, zxio_signals_t zxio_signals, zx_handle_t* out_handle,
                      zx_signals_t* out_zx_signals) {
    datagram_socket(zxio_datagram_socket(io)).wait_begin(zxio_signals, out_handle, out_zx_signals);
  };
  ops.wait_end = [](zxio_t* io, zx_signals_t zx_signals, zxio_signals_t* out_zxio_signals) {
    datagram_socket(zxio_datagram_socket(io)).wait_end(zx_signals, out_zxio_signals);
  };
  ops.readv = [](zxio_t* io, const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                 size_t* out_actual) {
    return zxio_readv(&zxio_datagram_socket(io).pipe.io, vector, vector_count, flags, out_actual);
  };
  ops.writev = [](zxio_t* io, const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                  size_t* out_actual) {
    return zxio_writev(&zxio_datagram_socket(io).pipe.io, vector, vector_count, flags, out_actual);
  };
  ops.shutdown = [](zxio_t* io, zxio_shutdown_options_t options, int16_t* out_code) {
    return network_socket(zxio_datagram_socket(io).client).shutdown(options, out_code);
  };
  ops.bind = [](zxio_t* io, const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
    return network_socket(zxio_datagram_socket(io).client).bind(addr, addrlen, out_code);
  };
  ops.connect = [](zxio_t* io, const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
    return network_socket(zxio_datagram_socket(io).client).connect(addr, addrlen, out_code);
  };
  ops.getsockname = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    return network_socket(zxio_datagram_socket(io).client).getsockname(addr, addrlen, out_code);
  };
  ops.getpeername = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    return network_socket(zxio_datagram_socket(io).client).getpeername(addr, addrlen, out_code);
  };
  ops.getsockopt = [](zxio_t* io, int level, int optname, void* optval, socklen_t* optlen,
                      int16_t* out_code) {
    SockOptResult result = network_socket(zxio_datagram_socket(io).client)
                               .getsockopt_fidl(level, optname, optval, optlen);
    *out_code = result.err;
    return result.status;
  };
  ops.setsockopt = [](zxio_t* io, int level, int optname, const void* optval, socklen_t optlen,
                      zx_handle_t access_token, int16_t* out_code) {
    auto& socket = zxio_datagram_socket(io);
    SockOptResult result =
        network_socket(socket.client)
            .setsockopt_fidl(level, optname, optval, optlen, zx::event(access_token), io);
    *out_code = result.err;
    return result.status;
  };
  ops.recvmsg = [](zxio_t* io, struct msghdr* msg, int flags, size_t* out_actual,
                   int16_t* out_code) {
    return datagram_socket(zxio_datagram_socket(io)).recvmsg(msg, flags, out_actual, out_code);
  };
  ops.sendmsg = [](zxio_t* io, const struct msghdr* msg, int flags, size_t* out_actual,
                   int16_t* out_code) {
    return datagram_socket(zxio_datagram_socket(io)).sendmsg(msg, flags, out_actual, out_code);
  };

  return ops;
}();

zx_status_t zxio_datagram_socket_init(zxio_storage_t* storage, zx::socket socket,
                                      const zx_info_socket_t& info,
                                      const zxio_datagram_prelude_size_t& prelude_size,
                                      fidl::ClientEnd<fsocket::DatagramSocket> client) {
  auto zs = new (storage) zxio_datagram_socket_t{
      .io = {},
      .pipe = {},
      .prelude_size = prelude_size,
      .client = fidl::WireSyncClient(std::move(client)),
  };
  zxio_init(&zs->io, &zxio_datagram_socket_ops);
  return zxio_pipe_init(reinterpret_cast<zxio_storage_t*>(&zs->pipe), std::move(socket), info);
}

enum class zxio_stream_socket_state_t {
  UNCONNECTED,
  LISTENING,
  CONNECTING,
  CONNECTED,
};

// A |zxio_t| backend that uses a fuchsia.posix.socket.StreamSocket object.
using zxio_stream_socket_t = struct zxio_stream_socket {
  zxio_t io;
  zxio_pipe_t pipe;
  std::mutex state_lock;
  zxio_stream_socket_state_t state __TA_GUARDED(state_lock);
  fidl::WireSyncClient<fuchsia_posix_socket::StreamSocket> client;
};

static_assert(sizeof(zxio_stream_socket_t) <= sizeof(zxio_storage_t),
              "zxio_stream_socket_t must fit inside zxio_storage_t.");

static zxio_stream_socket_t& zxio_stream_socket(zxio_t* io) {
  return *reinterpret_cast<zxio_stream_socket_t*>(io);
}

struct stream_socket : public socket_with_zx_socket<fidl::WireSyncClient<fsocket::StreamSocket>> {
 public:
  explicit stream_socket(zxio_stream_socket_t& stream_socket)
      : socket_with_zx_socket(stream_socket.client), stream_socket_(stream_socket) {}

  void wait_begin(zxio_signals_t zxio_signals, zx_handle_t* handle, zx_signals_t* out_signals) {
    zxio_signals_t pipe_signals = ZXIO_SIGNAL_PEER_CLOSED;

    auto [state, has_error] = GetState();
    switch (state) {
      case zxio_stream_socket_state_t::UNCONNECTED:
        // Stream sockets which are non-listening or unconnected do not have a potential peer
        // to generate any waitable signals, skip signal waiting and notify the caller of the
        // same.
        *out_signals = ZX_SIGNAL_NONE;
        return;
      case zxio_stream_socket_state_t::LISTENING:
        break;
      case zxio_stream_socket_state_t::CONNECTING:
        if (zxio_signals & ZXIO_SIGNAL_READABLE) {
          pipe_signals |= ZXIO_SIGNAL_READABLE;
        }
        break;
      case zxio_stream_socket_state_t::CONNECTED:
        if (zxio_signals & ZXIO_SIGNAL_READABLE) {
          pipe_signals |= ZXIO_SIGNAL_READABLE | ZXIO_SIGNAL_READ_DISABLED;
        }
        if (zxio_signals & ZXIO_SIGNAL_WRITABLE) {
          pipe_signals |= ZXIO_SIGNAL_WRITABLE | ZXIO_SIGNAL_WRITE_DISABLED;
        }
        if (zxio_signals & ZXIO_SIGNAL_READ_DISABLED) {
          pipe_signals |= ZXIO_SIGNAL_READ_DISABLED;
        }
        zxio_wait_begin(&stream_socket_.pipe.io, pipe_signals, handle, out_signals);
        return;
    }

    if (zxio_signals & ZXIO_SIGNAL_WRITABLE) {
      pipe_signals |= ZXIO_SIGNAL_WRITE_DISABLED;
    }
    if (zxio_signals & (ZXIO_SIGNAL_READABLE | ZXIO_SIGNAL_READ_DISABLED)) {
      pipe_signals |= ZXIO_SIGNAL_READ_DISABLED;
    }

    zx_signals_t zx_signals = ZX_SIGNAL_NONE;
    zxio_wait_begin(&stream_socket_.pipe.io, pipe_signals, handle, &zx_signals);

    if (zxio_signals & ZXIO_SIGNAL_WRITABLE) {
      // signal when connect() operation is finished.
      zx_signals |= fsocket::wire::kSignalStreamConnected;
    }
    if (zxio_signals & ZXIO_SIGNAL_READABLE) {
      // signal when a listening socket gets an incoming connection.
      zx_signals |= fsocket::wire::kSignalStreamIncoming;
    }
    *out_signals = zx_signals;
  }

  void wait_end(zx_signals_t zx_signals, zxio_signals_t* out_zxio_signals) {
    zxio_signals_t zxio_signals = ZXIO_SIGNAL_NONE;

    bool use_pipe;
    {
      std::lock_guard lock(stream_socket_.state_lock);
      auto [state, has_error] = StateLocked();
      switch (state) {
        case zxio_stream_socket_state_t::UNCONNECTED:
          ZX_ASSERT_MSG(zx_signals == ZX_SIGNAL_NONE, "zx_signals=%s on unconnected socket",
                        std::bitset<sizeof(zx_signals)>(zx_signals).to_string().c_str());
          *out_zxio_signals = ZXIO_SIGNAL_WRITABLE | ZXIO_SIGNAL_PEER_CLOSED;
          use_pipe = false;
          return;

        case zxio_stream_socket_state_t::LISTENING:
          if (zx_signals & fsocket::wire::kSignalStreamIncoming) {
            zxio_signals |= ZXIO_SIGNAL_READABLE;
          }
          use_pipe = false;
          break;
        case zxio_stream_socket_state_t::CONNECTING:
          if (zx_signals & fsocket::wire::kSignalStreamConnected) {
            stream_socket_.state = zxio_stream_socket_state_t::CONNECTED;
            zxio_signals |= ZXIO_SIGNAL_WRITABLE;
          }
          zx_signals &= ~fsocket::wire::kSignalStreamConnected;
          use_pipe = false;
          break;
        case zxio_stream_socket_state_t::CONNECTED:
          use_pipe = true;
          break;
      }
    }

    zxio_signals_t pipe_signals = ZXIO_SIGNAL_NONE;
    zxio_wait_end(&stream_socket_.pipe.io, zx_signals, &pipe_signals);
    if (use_pipe) {
      zxio_signals |= pipe_signals;
      if (zxio_signals & (ZXIO_SIGNAL_PEER_CLOSED | ZXIO_SIGNAL_READ_DISABLED)) {
        zxio_signals |= ZXIO_SIGNAL_READABLE;
      }
      if (zxio_signals & ZXIO_SIGNAL_WRITE_DISABLED) {
        zxio_signals |= ZXIO_SIGNAL_WRITABLE;
      }
      if (zxio_signals & ZXIO_SIGNAL_PEER_CLOSED) {
        zxio_signals |= ZXIO_SIGNAL_READ_DISABLED;
      }
    }

    if (pipe_signals & ZXIO_SIGNAL_PEER_CLOSED) {
      zxio_signals |= ZXIO_SIGNAL_READABLE | ZXIO_SIGNAL_WRITABLE | ZXIO_SIGNAL_ERROR |
                      ZXIO_SIGNAL_PEER_CLOSED | ZXIO_SIGNAL_READ_DISABLED;
    }
    if (pipe_signals & ZXIO_SIGNAL_WRITE_DISABLED) {
      zxio_signals |= ZXIO_SIGNAL_PEER_CLOSED | ZXIO_SIGNAL_WRITABLE;
    }
    if (pipe_signals & ZXIO_SIGNAL_READ_DISABLED) {
      zxio_signals |= ZXIO_SIGNAL_READ_DISABLED | ZXIO_SIGNAL_READABLE;
    }

    *out_zxio_signals = zxio_signals;
  }

  zx_status_t recvmsg(struct msghdr* msg, int flags, size_t* out_actual, int16_t* out_code) {
    std::optional preflight = Preflight(ENOTCONN);
    if (preflight.has_value()) {
      ErrOrOutCode err = preflight.value();
      if (err.is_error()) {
        return err.status_value();
      }
      *out_code = err.value();
      return ZX_OK;
    }

    std::optional read_error =
        GetZxSocketReadError(zxio_recvmsg_inner(&stream_socket_.io, msg, flags, out_actual));
    if (read_error.has_value()) {
      zx::result err = read_error.value();
      if (!err.is_error()) {
        *out_code = err.value();
        if (err.value() == 0) {
          *out_actual = 0;
        }
      }
      return err.status_value();
    }

    if (msg->msg_name) {
      msg->msg_namelen = 0;
    }

    *out_code = 0;
    return ZX_OK;
  }

  zx_status_t sendmsg(const struct msghdr* msg, int flags, size_t* out_actual, int16_t* out_code) {
    std::optional preflight = Preflight(EPIPE);
    if (preflight.has_value()) {
      ErrOrOutCode err = preflight.value();
      if (err.is_error()) {
        return err.status_value();
      }
      *out_code = err.value();
      return ZX_OK;
    }

    // Fuchsia does not support control messages on stream sockets. But we still parse the buffer
    // to check that it is valid.
    // TODO(https://fxbug.dev/42061949) Add tests with msg as nullptr.
    if (msg == nullptr) {
      *out_code = EFAULT;
      return ZX_OK;
    }
    const msghdr& msghdr_ref = *msg;
    fidl::Arena allocator;
    fit::result cmsg_result =
        ParseControlMessages<fsocket::wire::SocketSendControlData>(allocator, msghdr_ref);
    if (cmsg_result.is_error()) {
      *out_code = cmsg_result.error_value();
      return ZX_OK;
    }

    std::optional write_error =
        GetZxSocketWriteError(zxio_sendmsg_inner(&stream_socket_.io, msg, flags, out_actual));
    if (write_error.has_value()) {
      zx::result err = write_error.value();
      if (!err.is_error()) {
        *out_code = err.value();
      }
      return err.status_value();
    }
    *out_code = 0;
    return ZX_OK;
  }

 private:
  std::optional<ErrOrOutCode> Preflight(int fallback) {
    auto [state, has_error] = GetState();
    if (has_error) {
      zx::result err = GetError();
      if (err.is_error()) {
        return err.take_error();
      }
      if (int16_t value = err.value(); value != 0) {
        return zx::ok(value);
      }
      // Error was consumed.
    }

    switch (state) {
      case zxio_stream_socket_state_t::UNCONNECTED:
        __FALLTHROUGH;
      case zxio_stream_socket_state_t::LISTENING:
        return zx::ok(static_cast<int16_t>(fallback));
      case zxio_stream_socket_state_t::CONNECTING:
        if (!has_error) {
          return zx::ok(static_cast<int16_t>(EAGAIN));
        }
        // There's an error on the socket, we will discover it when we perform our I/O.
        __FALLTHROUGH;
      case zxio_stream_socket_state_t::CONNECTED:
        return std::nullopt;
    }
  }

  ErrOrOutCode GetError() override {
    fidl::WireResult response = stream_socket_.client->GetError();
    if (!response.ok()) {
      return zx::error(response.status());
    }
    const auto& result = response.value();
    if (result.is_error()) {
      return zx::ok(static_cast<int16_t>(result.error_value()));
    }
    return zx::ok(static_cast<int16_t>(0));
  }

  std::pair<zxio_stream_socket_state_t, bool> StateLocked()
      __TA_REQUIRES(stream_socket_.state_lock) {
    switch (stream_socket_.state) {
      case zxio_stream_socket_state_t::UNCONNECTED:
        __FALLTHROUGH;
      case zxio_stream_socket_state_t::LISTENING:
        return std::make_pair(stream_socket_.state, false);
      case zxio_stream_socket_state_t::CONNECTING: {
        zx_signals_t observed;
        zx_status_t status = stream_socket_.pipe.socket.wait_one(
            fsocket::wire::kSignalStreamConnected, zx::time::infinite_past(), &observed);
        switch (status) {
          case ZX_OK:
            if (observed & fsocket::wire::kSignalStreamConnected) {
              stream_socket_.state = zxio_stream_socket_state_t::CONNECTED;
            }
            __FALLTHROUGH;
          case ZX_ERR_TIMED_OUT:
            return std::make_pair(stream_socket_.state, observed & ZX_SOCKET_PEER_CLOSED);
          default:
            ZX_PANIC("ASSERT FAILED at (%s:%d): status=%s\n", __FILE__, __LINE__,
                     zx_status_get_string(status));
        }
        break;
      }
      case zxio_stream_socket_state_t::CONNECTED:
        return std::make_pair(stream_socket_.state, false);
    }
  }

  std::pair<zxio_stream_socket_state_t, bool> GetState() __TA_EXCLUDES(stream_socket_.state_lock) {
    std::lock_guard lock(stream_socket_.state_lock);
    return StateLocked();
  }

  zxio_stream_socket_t& stream_socket_;
};

static constexpr zxio_ops_t zxio_stream_socket_ops = []() {
  zxio_ops_t ops = zxio_default_socket_ops;
  ops.attr_get = AttrGet<ZXIO_OBJECT_TYPE_STREAM_SOCKET>;
  ops.destroy = [](zxio_t* io) {
    zxio_stream_socket_t& zs = zxio_stream_socket(io);
    zs.~zxio_stream_socket_t();
  };
  ops.close = [](zxio_t* io) {
    zxio_stream_socket_t& zs = zxio_stream_socket(io);
    zx_status_t status = ZX_OK;
    if (zs.client.is_valid()) {
      status = base_socket(zs.client).CloseSocket();
    }
    return status;
  };
  ops.release = [](zxio_t* io, zx_handle_t* out_handle) {
    if (out_handle == nullptr) {
      return ZX_ERR_INVALID_ARGS;
    }
    *out_handle = zxio_stream_socket(io).client.TakeClientEnd().TakeChannel().release();
    return ZX_OK;
  };
  ops.borrow = [](zxio_t* io, zx_handle_t* out_handle) {
    *out_handle = zxio_stream_socket(io).client.client_end().borrow().channel()->get();
    return ZX_OK;
  };
  ops.clone = [](zxio_t* io, zx_handle_t* out_handle) {
    return base_socket(zxio_stream_socket(io).client).CloneSocket(out_handle);
  };
  ops.wait_begin = [](zxio_t* io, zxio_signals_t zxio_signals, zx_handle_t* out_handle,
                      zx_signals_t* out_zx_signals) {
    stream_socket(zxio_stream_socket(io)).wait_begin(zxio_signals, out_handle, out_zx_signals);
  };
  ops.wait_end = [](zxio_t* io, zx_signals_t zx_signals, zxio_signals_t* out_zxio_signals) {
    stream_socket(zxio_stream_socket(io)).wait_end(zx_signals, out_zxio_signals);
  };
  ops.readv = [](zxio_t* io, const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                 size_t* out_actual) {
    zx::socket& socket = zxio_stream_socket(io).pipe.socket;

    if (flags & ZXIO_PEEK) {
      uint32_t zx_flags = ZX_SOCKET_PEEK;
      flags &= ~ZXIO_PEEK;

      if (flags) {
        return ZX_ERR_NOT_SUPPORTED;
      }

      size_t total = 0;
      for (size_t i = 0; i < vector_count; ++i) {
        total += vector[i].capacity;
      }
      std::unique_ptr<uint8_t[]> buf(new uint8_t[total]);

      size_t actual;
      zx_status_t status = socket.read(zx_flags, buf.get(), total, &actual);
      if (status != ZX_OK) {
        return status;
      }

      uint8_t* data = buf.get();
      size_t remaining = actual;
      return zxio_do_vector(
          vector, vector_count, out_actual,
          [&](void* buffer, size_t capacity, size_t total_so_far, size_t* out_actual) {
            size_t actual = std::min(capacity, remaining);
            memcpy(buffer, data, actual);
            data += actual;
            remaining -= actual;
            *out_actual = actual;
            return ZX_OK;
          });
    }

    if (flags) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    return zxio_stream_do_vector(vector, vector_count, out_actual,
                                 [&](void* buffer, size_t capacity, size_t* out_actual) {
                                   return socket.read(0, buffer, capacity, out_actual);
                                 });
  };
  ops.writev = [](zxio_t* io, const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                  size_t* out_actual) {
    return zxio_writev(&zxio_stream_socket(io).pipe.io, vector, vector_count, flags, out_actual);
  };
  ops.get_read_buffer_available = [](zxio_t* io, size_t* out_available) {
    return zxio_get_read_buffer_available(&zxio_stream_socket(io).pipe.io, out_available);
  };
  ops.shutdown = [](zxio_t* io, zxio_shutdown_options_t options, int16_t* out_code) {
    return network_socket(zxio_stream_socket(io).client).shutdown(options, out_code);
  };
  ops.bind = [](zxio_t* io, const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
    return network_socket(zxio_stream_socket(io).client).bind(addr, addrlen, out_code);
  };
  ops.connect = [](zxio_t* io, const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
    zx_status_t status =
        network_socket(zxio_stream_socket(io).client).connect(addr, addrlen, out_code);
    if (status == ZX_OK) {
      std::lock_guard lock(zxio_stream_socket(io).state_lock);
      switch (*out_code) {
        case 0:
          zxio_stream_socket(io).state = zxio_stream_socket_state_t::CONNECTED;
          break;
        case EINPROGRESS:
          zxio_stream_socket(io).state = zxio_stream_socket_state_t::CONNECTING;
          break;
      }
    }
    return status;
  };
  ops.listen = [](zxio_t* io, int backlog, int16_t* out_code) {
    auto response =
        zxio_stream_socket(io).client->Listen(safemath::saturated_cast<int16_t>(backlog));
    zx_status_t status = response.status();
    if (status != ZX_OK) {
      return status;
    }
    auto const& result = response.value();
    if (result.is_error()) {
      *out_code = static_cast<int16_t>(result.error_value());
      return ZX_OK;
    }
    {
      std::lock_guard lock(zxio_stream_socket(io).state_lock);
      zxio_stream_socket(io).state = zxio_stream_socket_state_t::LISTENING;
    }
    *out_code = 0;
    return ZX_OK;
  };
  ops.accept = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen,
                  zxio_storage_t* out_storage, int16_t* out_code) {
    bool want_addr = addr != nullptr && addrlen != nullptr;
    auto response = zxio_stream_socket(io).client->Accept(want_addr);
    zx_status_t status = response.status();
    if (status != ZX_OK) {
      return status;
    }
    const auto& result = response.value();
    if (result.is_error()) {
      *out_code = static_cast<int16_t>(result.error_value());
      return ZX_OK;
    }
    *out_code = 0;
    auto const& out = result.value()->addr;

    // Result address is not provided by the server (when want_addr is false).
    if (want_addr && out.has_value()) {
      *addrlen = static_cast<socklen_t>(fidl_to_sockaddr(out.value(), addr, *addrlen));
    }

    fidl::ClientEnd<fsocket::StreamSocket>& control = result.value()->s;
    fidl::WireResult describe_result = fidl::WireCall(control)->Describe();
    if (!describe_result.ok()) {
      return describe_result.status();
    }
    fidl::WireResponse describe_response = describe_result.value();
    if (!describe_response.has_socket()) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    zx::socket& socket = describe_response.socket();
    zx_info_socket_t info;
    if (zx_status_t status = socket.get_info(ZX_INFO_SOCKET, &info, sizeof(info), nullptr, nullptr);
        status != ZX_OK) {
      return status;
    }
    if (zx_status_t status = zxio_stream_socket_init(out_storage, std::move(socket), info,
                                                     /*is_connected=*/true, std::move(control));
        status != ZX_OK) {
      return status;
    }
    return ZX_OK;
  };
  ops.getsockname = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    return network_socket(zxio_stream_socket(io).client).getsockname(addr, addrlen, out_code);
  };
  ops.getpeername = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    return network_socket(zxio_stream_socket(io).client).getpeername(addr, addrlen, out_code);
  };
  ops.getsockopt = [](zxio_t* io, int level, int optname, void* optval, socklen_t* optlen,
                      int16_t* out_code) {
    SockOptResult result = network_socket(zxio_stream_socket(io).client)
                               .getsockopt_fidl(level, optname, optval, optlen);
    *out_code = result.err;
    return result.status;
  };
  ops.setsockopt = [](zxio_t* io, int level, int optname, const void* optval, socklen_t optlen,
                      zx_handle_t access_token, int16_t* out_code) {
    auto& socket = zxio_stream_socket(io);
    SockOptResult result =
        network_socket(socket.client)
            .setsockopt_fidl(level, optname, optval, optlen, zx::event(access_token), io);
    *out_code = result.err;
    return result.status;
  };
  ops.recvmsg = [](zxio_t* io, struct msghdr* msg, int flags, size_t* out_actual,
                   int16_t* out_code) {
    return stream_socket(zxio_stream_socket(io)).recvmsg(msg, flags, out_actual, out_code);
  };
  ops.sendmsg = [](zxio_t* io, const struct msghdr* msg, int flags, size_t* out_actual,
                   int16_t* out_code) {
    return stream_socket(zxio_stream_socket(io)).sendmsg(msg, flags, out_actual, out_code);
  };

  return ops;
}();

zx_status_t zxio_stream_socket_init(zxio_storage_t* storage, zx::socket socket,
                                    const zx_info_socket_t& info, const bool is_connected,
                                    fidl::ClientEnd<fsocket::StreamSocket> client) {
  zxio_stream_socket_state_t state = is_connected ? zxio_stream_socket_state_t::CONNECTED
                                                  : zxio_stream_socket_state_t::UNCONNECTED;
  auto zs = new (storage) zxio_stream_socket_t{
      .io = {},
      .pipe = {},
      .state_lock = {},
      .state = state,
      .client = fidl::WireSyncClient(std::move(client)),
  };
  zxio_init(&zs->io, &zxio_stream_socket_ops);
  return zxio_pipe_init(reinterpret_cast<zxio_storage_t*>(&zs->pipe), std::move(socket), info);
}

static RawSocket::Storage& zxio_raw_socket(zxio_t* io) {
  return *reinterpret_cast<RawSocket::Storage*>(io);
}

static constexpr zxio_ops_t zxio_raw_socket_ops = []() {
  zxio_ops_t ops = zxio_default_socket_ops;
  ops.attr_get = AttrGet<ZXIO_OBJECT_TYPE_RAW_SOCKET>;
  ops.destroy = [](zxio_t* io) {
    RawSocket::Storage& zs = zxio_raw_socket(io);
    zs.~Storage();
  };
  ops.close = [](zxio_t* io) {
    RawSocket::Storage& zs = zxio_raw_socket(io);
    zx_status_t status = ZX_OK;
    if (zs.client.is_valid()) {
      status = base_socket(zs.client).CloseSocket();
    }
    return status;
  };
  ops.release = [](zxio_t* io, zx_handle_t* out_handle) {
    if (out_handle == nullptr) {
      return ZX_ERR_INVALID_ARGS;
    }
    *out_handle = zxio_raw_socket(io).client.TakeClientEnd().TakeChannel().release();
    return ZX_OK;
  };
  ops.borrow = [](zxio_t* io, zx_handle_t* out_handle) {
    *out_handle = zxio_raw_socket(io).client.client_end().borrow().channel()->get();
    return ZX_OK;
  };
  ops.clone = [](zxio_t* io, zx_handle_t* out_handle) {
    RawSocket::Storage& zs = zxio_raw_socket(io);
    zx_status_t status = base_socket(zs.client).CloneSocket(out_handle);
    return status;
  };
  ops.bind = [](zxio_t* io, const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
    return network_socket(zxio_raw_socket(io).client).bind(addr, addrlen, out_code);
  };
  ops.connect = [](zxio_t* io, const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
    return network_socket(zxio_raw_socket(io).client).connect(addr, addrlen, out_code);
  };
  ops.getsockname = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    return network_socket(zxio_raw_socket(io).client).getsockname(addr, addrlen, out_code);
  };
  ops.getpeername = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    return network_socket(zxio_raw_socket(io).client).getpeername(addr, addrlen, out_code);
  };
  ops.getsockopt = [](zxio_t* io, int level, int optname, void* optval, socklen_t* optlen,
                      int16_t* out_code) {
    SockOptResult result = [&]() {
      GetSockOptProcessor proc(optval, optlen);
      switch (level) {
        case SOL_ICMPV6:
          switch (optname) {
            case ICMP6_FILTER:
              return proc.Process(zxio_raw_socket(io).client->GetIcmpv6Filter(),
                                  [](const auto& response) { return response.filter; });
          }
          break;
        case SOL_IPV6:
          switch (optname) {
            case IPV6_CHECKSUM:
              return proc.Process(
                  zxio_raw_socket(io).client->GetIpv6Checksum(), [](const auto& response) {
                    switch (response.config.Which()) {
                      case frawsocket::wire::Ipv6ChecksumConfiguration::Tag::kDisabled:
                        return -1;
                      case frawsocket::wire::Ipv6ChecksumConfiguration::Tag::kOffset:
                        return response.config.offset();
                    };
                  });
          }
          break;
        case SOL_IP:
          switch (optname) {
            case IP_HDRINCL:
              return proc.Process(zxio_raw_socket(io).client->GetIpHeaderIncluded(),
                                  [](const auto& response) { return response.value; });
          }
          break;
      }
      return network_socket(zxio_raw_socket(io).client)
          .getsockopt_fidl(level, optname, optval, optlen);
    }();
    *out_code = result.err;
    return result.status;
  };
  ops.getsockopt = [](zxio_t* io, int level, int optname, void* optval, socklen_t* optlen,
                      int16_t* out_code) {
    SockOptResult result = [&]() {
      GetSockOptProcessor proc(optval, optlen);
      switch (level) {
        case SOL_ICMPV6:
          switch (optname) {
            case ICMP6_FILTER:
              return proc.Process(zxio_raw_socket(io).client->GetIcmpv6Filter(),
                                  [](const auto& response) { return response.filter; });
          }
          break;
        case SOL_IPV6:
          switch (optname) {
            case IPV6_CHECKSUM:
              return proc.Process(
                  zxio_raw_socket(io).client->GetIpv6Checksum(), [](const auto& response) {
                    switch (response.config.Which()) {
                      case frawsocket::wire::Ipv6ChecksumConfiguration::Tag::kDisabled:
                        return -1;
                      case frawsocket::wire::Ipv6ChecksumConfiguration::Tag::kOffset:
                        return response.config.offset();
                    };
                  });
          }
          break;
        case SOL_IP:
          switch (optname) {
            case IP_HDRINCL:
              return proc.Process(zxio_raw_socket(io).client->GetIpHeaderIncluded(),
                                  [](const auto& response) { return response.value; });
          }
          break;
      }
      return network_socket(zxio_raw_socket(io).client)
          .getsockopt_fidl(level, optname, optval, optlen);
    }();
    *out_code = result.err;
    return result.status;
  };
  ops.setsockopt = [](zxio_t* io, int level, int optname, const void* optval, socklen_t optlen,
                      zx_handle_t access_token_handle, int16_t* out_code) {
    zx::handle access_token(access_token_handle);
    SockOptResult result = [&]() {
      SetSockOptProcessor proc(optval, optlen);

      switch (level) {
        case SOL_ICMPV6:
          switch (optname) {
            case ICMP6_FILTER:
              return proc.Process<frawsocket::wire::Icmpv6Filter>(
                  [io](frawsocket::wire::Icmpv6Filter value) {
                    return zxio_raw_socket(io).client->SetIcmpv6Filter(value);
                  });
          }
          break;
        case SOL_IPV6:
          switch (optname) {
            case IPV6_CHECKSUM:
              return proc.Process<int32_t>([io](int32_t value) {
                frawsocket::wire::Ipv6ChecksumConfiguration config;

                if (value < 0) {
                  config = frawsocket::wire::Ipv6ChecksumConfiguration::WithDisabled(
                      frawsocket::wire::Empty{});
                } else {
                  config = frawsocket::wire::Ipv6ChecksumConfiguration::WithOffset(value);
                }

                return zxio_raw_socket(io).client->SetIpv6Checksum(config);
              });
          }
          break;
        case SOL_IP:
          switch (optname) {
            case IP_HDRINCL:
              return proc.Process<bool>([io](bool value) {
                return zxio_raw_socket(io).client->SetIpHeaderIncluded(value);
              });
          }
          break;
      }
      auto& socket = zxio_raw_socket(io);
      return network_socket(socket.client)
          .setsockopt_fidl(level, optname, optval, optlen, std::move(access_token), io);
    }();
    *out_code = result.err;
    return result.status;
  };
  ops.recvmsg = [](zxio_t* io, struct msghdr* msg, int flags, size_t* out_actual,
                   int16_t* out_code) {
    return socket_with_event<RawSocket>(zxio_raw_socket(io))
        .recvmsg(msg, flags, out_actual, out_code);
  };
  ops.sendmsg = [](zxio_t* io, const struct msghdr* msg, int flags, size_t* out_actual,
                   int16_t* out_code) {
    return socket_with_event<RawSocket>(zxio_raw_socket(io))
        .sendmsg(msg, flags, out_actual, out_code);
  };
  ops.wait_begin = [](zxio_t* io, zxio_signals_t zxio_signals, zx_handle_t* handle,
                      zx_signals_t* out_signals) {
    return socket_with_event<RawSocket>(zxio_raw_socket(io))
        .wait_begin(zxio_signals, handle, out_signals);
  };
  ops.wait_end = [](zxio_t* io, zx_signals_t zx_signals, zxio_signals_t* out_zxio_signals) {
    return socket_with_event<RawSocket>(zxio_raw_socket(io)).wait_end(zx_signals, out_zxio_signals);
  };
  ops.shutdown = [](zxio_t* io, zxio_shutdown_options_t options, int16_t* out_code) {
    return network_socket(zxio_raw_socket(io).client).shutdown((options), out_code);
  };

  return ops;
}();

zx_status_t zxio_raw_socket_init(zxio_storage_t* storage, zx::eventpair event,
                                 fidl::ClientEnd<frawsocket::Socket> client) {
  auto zs = new (storage) RawSocket::Storage{
      .io = storage->io,
      .event = std::move(event),
      .client = fidl::WireSyncClient(std::move(client)),
  };
  zxio_init(&zs->io, &zxio_raw_socket_ops);
  return ZX_OK;
}

static PacketSocket::Storage& zxio_packet_socket(zxio_t* io) {
  return *reinterpret_cast<PacketSocket::Storage*>(io);
}

static constexpr zxio_ops_t zxio_packet_socket_ops = []() {
  zxio_ops_t ops = zxio_default_socket_ops;
  ops.attr_get = AttrGet<ZXIO_OBJECT_TYPE_PACKET_SOCKET>;
  ops.destroy = [](zxio_t* io) {
    PacketSocket::Storage& zs = zxio_packet_socket(io);
    zs.~Storage();
  };
  ops.close = [](zxio_t* io) {
    PacketSocket::Storage& zs = zxio_packet_socket(io);
    zx_status_t status = ZX_OK;
    if (zs.client.is_valid()) {
      status = base_socket(zs.client).CloseSocket();
    }
    return status;
  };
  ops.release = [](zxio_t* io, zx_handle_t* out_handle) {
    if (out_handle == nullptr) {
      return ZX_ERR_INVALID_ARGS;
    }
    *out_handle = zxio_packet_socket(io).client.TakeClientEnd().TakeChannel().release();
    return ZX_OK;
  };
  ops.borrow = [](zxio_t* io, zx_handle_t* out_handle) {
    *out_handle = zxio_packet_socket(io).client.client_end().borrow().channel()->get();
    return ZX_OK;
  };
  ops.clone = [](zxio_t* io, zx_handle_t* out_handle) {
    PacketSocket::Storage& zs = zxio_packet_socket(io);
    zx_status_t status = base_socket(zs.client).CloneSocket(out_handle);
    return status;
  };
  ops.bind = [](zxio_t* io, const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
    if (addr == nullptr || addrlen < sizeof(sockaddr_ll)) {
      return ZX_ERR_INVALID_ARGS;
    }

    const sockaddr_ll& sll = *reinterpret_cast<const sockaddr_ll*>(addr);

    fpacketsocket::wire::ProtocolAssociation proto_assoc;
    uint16_t protocol = ntohs(sll.sll_protocol);
    switch (protocol) {
      case 0:
        // protocol association is optional.
        break;
      case ETH_P_ALL:
        proto_assoc =
            fpacketsocket::wire::ProtocolAssociation::WithAll(fpacketsocket::wire::Empty());
        break;
      default:
        proto_assoc = fpacketsocket::wire::ProtocolAssociation::WithSpecified(protocol);
        break;
    }

    fpacketsocket::wire::BoundInterfaceId interface_id;
    uint64_t ifindex = sll.sll_ifindex;
    if (ifindex == 0) {
      interface_id = fpacketsocket::wire::BoundInterfaceId::WithAll(fpacketsocket::wire::Empty());
    } else {
      interface_id = fpacketsocket::wire::BoundInterfaceId::WithSpecified(
          fidl::ObjectView<uint64_t>::FromExternal(&ifindex));
    }

    const fidl::WireResult response =
        zxio_packet_socket(io).client->Bind(proto_assoc, interface_id);
    zx_status_t status = response.status();
    if (status != ZX_OK) {
      return status;
    }
    const auto& result = response.value();
    if (result.is_error()) {
      *out_code = static_cast<int16_t>(result.error_value());
      return ZX_OK;
    }
    *out_code = 0;
    return ZX_OK;
  };
  ops.getsockname = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    if (addrlen == nullptr || (*addrlen != 0 && addr == nullptr)) {
      *out_code = EFAULT;
      return ZX_OK;
    }

    const fidl::WireResult response = zxio_packet_socket(io).client->GetInfo();
    zx_status_t status = response.status();
    if (status != ZX_OK) {
      return status;
    }
    const auto& result = response.value();
    if (result.is_error()) {
      *out_code = static_cast<int16_t>(result.error_value());
      return ZX_OK;
    }
    *out_code = 0;

    const fpacketsocket::wire::SocketGetInfoResponse& info = *result.value();
    sockaddr_ll sll = {
        .sll_family = AF_PACKET,
        .sll_protocol = htons(fidl_protoassoc_to_protocol(info.protocol)),
    };

    switch (info.bound_interface.Which()) {
      case fpacketsocket::wire::BoundInterface::Tag::kAll:
        sll.sll_ifindex = 0;
        sll.sll_halen = 0;
        sll.sll_hatype = 0;
        break;
      case fpacketsocket::wire::BoundInterface::Tag::kSpecified: {
        const fpacketsocket::wire::InterfaceProperties& props = info.bound_interface.specified();
        sll.sll_ifindex = static_cast<int>(props.id);
        sll.sll_hatype = fidl_hwtype_to_arphrd(props.type);
        populate_from_fidl_hwaddr(props.addr, sll);
      } break;
    }

    socklen_t used_bytes = offsetof(sockaddr_ll, sll_addr) + sll.sll_halen;
    memcpy(addr, &sll, std::min(used_bytes, *addrlen));
    *addrlen = used_bytes;
    return ZX_OK;
  };
  ops.getsockopt = [](zxio_t* io, int level, int optname, void* optval, socklen_t* optlen,
                      int16_t* out_code) {
    SockOptResult result = [&]() {
      switch (level) {
        case SOL_SOCKET:
          return base_socket(zxio_packet_socket(io).client)
              .get_solsocket_sockopt_fidl(optname, optval, optlen);
        default:
          return SockOptResult::Errno(ENOPROTOOPT);
      }
    }();
    *out_code = result.err;
    return result.status;
  };
  ops.setsockopt = [](zxio_t* io, int level, int optname, const void* optval, socklen_t optlen,
                      zx_handle_t access_token_handle, int16_t* out_code) {
    zx::handle access_token(access_token_handle);
    SockOptResult result = [&]() {
      switch (level) {
        case SOL_SOCKET: {
          auto& socket = zxio_packet_socket(io);
          return base_socket(socket.client)
              .set_solsocket_sockopt_fidl(optname, optval, optlen, std::move(access_token), io);
        }
        default:
          return SockOptResult::Errno(ENOPROTOOPT);
      }
    }();
    *out_code = result.err;
    return result.status;
  };
  ops.recvmsg = [](zxio_t* io, struct msghdr* msg, int flags, size_t* out_actual,
                   int16_t* out_code) {
    return socket_with_event<PacketSocket>(zxio_packet_socket(io))
        .recvmsg(msg, flags, out_actual, out_code);
  };
  ops.sendmsg = [](zxio_t* io, const struct msghdr* msg, int flags, size_t* out_actual,
                   int16_t* out_code) {
    return socket_with_event<PacketSocket>(zxio_packet_socket(io))
        .sendmsg(msg, flags, out_actual, out_code);
  };
  ops.wait_begin = [](zxio_t* io, zxio_signals_t zxio_signals, zx_handle_t* handle,
                      zx_signals_t* out_signals) {
    return socket_with_event<PacketSocket>(zxio_packet_socket(io))
        .wait_begin(zxio_signals, handle, out_signals);
  };
  ops.wait_end = [](zxio_t* io, zx_signals_t zx_signals, zxio_signals_t* out_zxio_signals) {
    return socket_with_event<PacketSocket>(zxio_packet_socket(io))
        .wait_end(zx_signals, out_zxio_signals);
  };

  return ops;
}();

zx_status_t zxio_packet_socket_init(zxio_storage_t* storage, zx::eventpair event,
                                    fidl::ClientEnd<fpacketsocket::Socket> client) {
  auto zs = new (storage) PacketSocket::Storage{
      .io = storage->io,
      .event = std::move(event),
      .client = fidl::WireSyncClient(std::move(client)),
  };
  zxio_init(&zs->io, &zxio_packet_socket_ops);
  return ZX_OK;
}
