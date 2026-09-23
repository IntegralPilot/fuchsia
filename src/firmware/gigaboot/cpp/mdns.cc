// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "mdns.h"

#include <algorithm>

#include "network.h"

namespace gigaboot {

namespace {

constexpr uint16_t kMDNSNameAtOffsetFlag = 0xc000;

// Used to allow multiple single-type lambdas in std::visit
// instead of a single lambda with multiple constexpr if branches
template <class... Ts>
struct overload : Ts... {
  using Ts::operator()...;
};
template <class... Ts>
overload(Ts...) -> overload<Ts...>;

// Given a MAC address, return a printable name identifying the device.
// The name will be of the form
// fuchsia-XXXX-XXXX-XXXX
// where the X's are the hex digits that make up the device's MAC address.
// The device name is null terminated.
//
// See https://cs.opensource.google/fuchsia/fuchsia/+/main:src/bringup/bin/device-name-provider/
// for the authoritative definition of the device name format.
DeviceName DeviceNameFromMac(MacAddr m) {
  DeviceName name;
  snprintf(name.data(), name.size(), "fuchsia-%02x%02x-%02x%02x-%02x%02x", m[0], m[1], m[2], m[3],
           m[4], m[5]);
  return name;
}

}  // namespace

constexpr uint16_t kMDNSClassCacheFlush = 1 << 15;
constexpr uint16_t kMDNSClassIn = 1;

bool MdnsPacket::WriteU8(uint8_t c) {
  if (payload_end_ == data_.payload.end()) {
    return false;
  }

  *payload_end_++ = c;
  return true;
}

bool MdnsPacket::WriteU16(uint16_t s) {
  if (payload_end_ + sizeof(s) > data_.payload.end()) {
    return false;
  }

  s = htons(s);
  const uint8_t* s_bytes = reinterpret_cast<const uint8_t*>(&s);
  payload_end_ = std::copy(s_bytes, s_bytes + sizeof(s), payload_end_);
  return true;
}

bool MdnsPacket::WriteU32(uint32_t l) {
  if (payload_end_ + sizeof(l) > data_.payload.end()) {
    return false;
  }

  l = htonl(l);
  const uint8_t* l_bytes = reinterpret_cast<const uint8_t*>(&l);
  payload_end_ = std::copy(l_bytes, l_bytes + sizeof(l), payload_end_);
  return true;
}

bool MdnsPacket::WriteBytes(std::span<const uint8_t> bytes) {
  if (payload_end_ + bytes.size() > data_.payload.end()) {
    return false;
  }

  payload_end_ = std::copy(bytes.begin(), bytes.end(), payload_end_);
  return true;
}

bool DnsContext::WriteFQDN(DnsNameSegment& segment, MdnsPacket& packet) {
  for (DnsNameSegment* cur = &segment; cur != nullptr; cur = cur->next) {
    if (cur->loc) {
      return packet.WriteU16(cur->loc | kMDNSNameAtOffsetFlag);
    }

    // DNS and mDNS name segments have a maximum length of 63 octets.
    // If the two most significant bits are set, that indicates that
    // instead of a 1 byte length the field is instead a 2 byte offset.
    // We cannot represent a name segment longer than 63 bytes in the serialized
    // format, so if we encounter one, signal an error and abort.
    if (cur->name.size() > 63) {
      return false;
    }

    // Cache this segment and its suffixes.
    cur->loc = static_cast<uint16_t>(packet.DnsBytesUsed());

    // Name segments are stored as a length-data tuple.
    if (!packet.WriteU8(static_cast<uint8_t>(cur->name.size())) ||
        !packet.WriteBytes(
            {reinterpret_cast<const uint8_t*>(cur->name.data()), cur->name.size()})) {
      return false;
    }
  }

  // The root name segment is implicit and has a length of 0;
  return packet.WriteU8(0);
}

bool DnsContext::WriteRecord(zx::duration time_to_live, DnsRecord& record, MdnsPacket& packet) {
  if (record.name == nullptr || !WriteFQDN(*record.name, packet) ||
      // Minor hack to make record types self describing.
      !packet.WriteU16(std::visit(overload{[](const auto& r) { return r.kType; }}, record.data)) ||
      !packet.WriteU16(record.record_class) ||
      !packet.WriteU32(static_cast<uint32_t>(time_to_live.to_secs()))) {
    return false;
  }

  // Save a spot for the record length.
  DnsPayload::iterator size_field = packet.CurrentEnd();
  if (!packet.WriteU16(0)) {
    return false;
  }

  // Take a look at the different structures to determine their fields.
  if (!std::visit(overload{
                      [&](DnsPtrRecord& r) -> bool {
                        return r.name != nullptr && WriteFQDN(*r.name, packet);
                      },
                      [&](DnsAAAARecord& r) -> bool { return packet.WriteBytes(r.addr); },
                      [&](DnsSrvRecord& r) -> bool {
                        return r.target != nullptr && packet.WriteU16(r.priority) &&
                               packet.WriteU16(r.weight) && packet.WriteU16(r.port) &&
                               WriteFQDN(*r.target, packet);
                      },
                  },
                  record.data)) {
    return false;
  }

  // Back-patch the record length
  uint16_t resource_length =
      htons(static_cast<uint16_t>(packet.CurrentEnd() - (size_field + sizeof(uint16_t))));
  std::span<const uint8_t> length_bytes(reinterpret_cast<const uint8_t*>(&resource_length),
                                        sizeof(resource_length));
  std::copy(length_bytes.begin(), length_bytes.end(), size_field);
  return true;
}

bool DnsContext::WriteRecords(zx::duration time_to_live, MdnsPacket& packet) {
  // Clear the loc; name segment compression is path dependent.
  for (DnsNameSegment& segment : name_segments_) {
    segment.loc = 0;
  }

  for (DnsRecord& record : records_) {
    if (!WriteRecord(time_to_live, record, packet)) {
      return false;
    }
  }

  return true;
}

fit::result<efi_status, std::span<const uint8_t>> MdnsPacket::Serialize(zx::duration time_to_live) {
  if (time_to_live_.has_value() && *time_to_live_ == time_to_live) {
    return fit::ok(std::span<const uint8_t>(reinterpret_cast<const uint8_t*>(&data_),
                                            reinterpret_cast<const uint8_t*>(&(*payload_end_))));
  }

  ResetPayload();
  if (!context_.WriteRecords(time_to_live, *this)) {
    return fit::error(EFI_BUFFER_TOO_SMALL);
  }

  time_to_live_ = time_to_live;
  return fit::ok(Finalize());
}

std::span<const uint8_t> MdnsPacket::Finalize() {
  uint16_t length = htons(static_cast<uint16_t>(reinterpret_cast<uintptr_t>(&(*payload_end_)) -
                                                reinterpret_cast<uintptr_t>(&data_.udp_header)));
  // IP6 length field does NOT include its header,
  // UDP length field DOES include its header.
  data_.ip6_header.length = length;
  data_.udp_header.length = length;
  data_.udp_header.checksum = 0;

  const uint8_t* end_ptr = reinterpret_cast<const uint8_t*>(&(*payload_end_));
  // The UDP checksum calculation uses an IP pseudoheader and the UDP payload.
  // There are some intricacies involved in the checksum calculation:
  // 1) Assuming a checksum calculation occurs in a single call,
  //    i.e. there is exactly one carry-over calculation in 1s complement addition,
  //    the order of the addition doesn't matter.
  //    Addition is both commutative and associative in 1s complement.
  // 2) The entire UDP header and DNS payload are included in the calculation.
  // 3) The IPv6 pseudoheader includes the UDP length, next header field, and
  //    source and destination address. It is zero-padded to fill 40 bytes.
  // 4) The memory layout of the packet includes no padding bytes and has all the
  //    headers and payloads adjacent to each other.
  // 5) All together, this means that we can start the calculation with the
  //    UDP length and UDP next header value, then calculate the checksum over the
  //    {ip6 src, ip6 dest, udp header, dns payload} as a contiguous array of bytes.
  //    We don't need to construct the pseudoheader, we just fake it.
  std::span<const uint8_t> udp_bytes(const_cast<const uint8_t*>(data_.ip6_header.source.data()),
                                     end_ptr);
  // Add zero-pad bytes to the header, and add the already network-endian length.
  uint64_t start = htons(static_cast<uint16_t>(kUdpHdr)) + length;
  uint16_t checksum = CalculateChecksum(udp_bytes, start);
  data_.udp_header.checksum = (checksum != 0xFFFF) ? static_cast<uint16_t>(~checksum) : checksum;
  return std::span<const uint8_t>(reinterpret_cast<const uint8_t*>(&data_), end_ptr);
}

MdnsAgent::MdnsAgent(EthernetAgent& eth_agent, efi_system_table* sys)
    : eth_agent_(eth_agent),
      device_name_(DeviceNameFromMac(eth_agent_.Addr())),
      timer_(sys),
      tx_callback_event_(nullptr) {
  // The first poll should always broadcast.
  // It's the tx callback's responsibility to set the timer
  // to the steady state poll period.
  // It's safe to ignore the return value because of the semantics of
  // setting the timer to a zero value.
  std::ignore = timer_.SetTimer(TimerRelative, zx::sec(0));

  auto null_terminator = std::find(device_name_.begin(), device_name_.end(), '\0');
  std::string_view id_view(&(*device_name_.begin()), null_terminator - device_name_.begin());

  // The name segments and the records should really be passed in on construction,
  // but the intended use case for the MDNS agent is sharply limited.
  // If that ever becomes a real issue, the two immediate problems to solve are
  // 1) plumbing the segments and records through from the caller.
  // 2) making a sentinel name value to back patch with the device name.
  fbl::Vector<DnsNameSegment> segments({
      // 0: local.
      {
          .name = "local",
          .loc = 0,
      },
      // 1: _tcp.local.
      {
          .name = "_tcp",
          .loc = 0,
      },
      // 2: _fastboot._tcp.local.
      {
          .name = "_fastboot",
          .loc = 0,
      },
      // 3: <nodename>._fastboot._tcp.local.
      {
          .name = id_view,
          .loc = 0,
      },
      // 4: <nodename>.local.
      {
          .name = id_view,
          .loc = 0,
      },
  });

  // Set links after initial setup.
  // Note: it's safe to move `segments` because the elements don't move.
  // Don't do anything on `segments` that invalidates iterators,
  // or that will invalidate the `next` links.
  segments[0].next = nullptr;
  segments[1].next = &segments[0];
  segments[2].next = &segments[1];
  segments[3].next = &segments[2];
  segments[4].next = &segments[0];

  DnsNameSegment* fastboot_ptr_name = &segments[2];
  DnsNameSegment* fastboot_service_name = &segments[3];
  DnsNameSegment* mdns_nodename_local = &segments[4];
  fbl::Vector<DnsRecord> records({
      DnsRecord{
          .name = fastboot_ptr_name,
          .record_class = kMDNSClassCacheFlush | kMDNSClassIn,
          .data = DnsPtrRecord{fastboot_service_name},
      },
      DnsRecord{
          .name = fastboot_service_name,
          .record_class = kMDNSClassCacheFlush | kMDNSClassIn,
          .data =
              DnsSrvRecord{
                  .priority = 0,
                  .weight = 0,
                  .port = kFbServerPort,
                  .target = mdns_nodename_local,
              },
      },
      {
          .name = mdns_nodename_local,
          .record_class = kMDNSClassCacheFlush | kMDNSClassIn,
          .data =
              DnsAAAARecord{
                  LocalIp6AddrFromMac(eth_agent_.Addr()),
              },
      },

  });

  DnsHeader header = {
      .id = htons(0),
      .flags = htons(kMdnsFlagQueryResponse | kMdnsFlagAuthoritative),
      .question_count = htons(0),
      .answer_count = htons(1),
      .authority_count = htons(0),
      .additional_count = htons(2),
  };

  packet_ = std::make_unique<MdnsPacket>(LocalIp6AddrFromMac(eth_agent_.Addr()), header,
                                         std::move(segments), std::move(records));
}

fit::result<efi_status, efi_event> MdnsAgent::LazySetup() {
  // We want the agent to reset the timer only AFTER successful transmission.
  //
  // Note: this looks unsafe because the callback event may be signaled asynchronously
  //       after transmission. This seemingly leaves a hole where the agent
  //       and its timer are destructed and then the callback fires.
  //       However, the event is destructed as part of the agent's destruction,
  //       and that deregisters it from all notify lists.
  auto callback_fn = [](efi_event event, void* context) EFIAPI {
    Timer* t = reinterpret_cast<Timer*>(context);
    std::ignore = t->SetTimer(TimerRelative, kMdnsPollPeriod);
  };

  efi_event event;
  if (efi_status res = gEfiSystemTable->BootServices->CreateEvent(EVT_NOTIFY_SIGNAL, TPL_CALLBACK,
                                                                  callback_fn, &timer_, &event);
      res != EFI_SUCCESS) {
    return fit::error(res);
  }

  return fit::ok(event);
}

fit::result<efi_status> MdnsAgent::Poll() {
  if (timer_.CheckTimer() != Timer::Status::kReady) {
    return fit::ok();
  }

  if (!tx_callback_event_) {
    auto res = LazySetup();
    if (res.is_error()) {
      return res.take_error();
    }
    tx_callback_event_ = res.value();
  }

  auto res = packet_->Serialize(MdnsAgent::kMdnsTtl);
  if (res.is_error()) {
    return res.take_error();
  }

  return eth_agent_.SendV6LocalFrame(kEthMdnsDestAddr, res.value(), tx_callback_event_, &token_);
}

MdnsAgent::~MdnsAgent() {
  // Don't do anything if we destruct before successfully polling.
  if (tx_callback_event_) {
    auto res = packet_->Serialize(zx::sec(0));
    if (res.is_ok()) {
      // Name invalidation is done on a best effort basis.
      // There isn't anything we can do if the call fails.
      std::ignore =
          eth_agent_.SendV6LocalFrame(kEthMdnsDestAddr, res.value(), tx_callback_event_, &token_);
    }

    // Note: this looks unsafe because the underlying Transmit call in SendV6LocalFrame
    //       signals the callback event after transmission, and transmission is asynchronous.
    //       It turns out it is perfectly valid to pass a closed event as the callback;
    //       Transmit only returns an error if the event is null, and the transmit can
    //       still complete successfully. The saved status WILL be an error, however,
    //       since the event is no longer valid. This is not a problem.
    //       We don't want to reset the timer in any case, as this is the final
    //       transmission.
    gEfiSystemTable->BootServices->CloseEvent(tx_callback_event_);
  }
}

}  // namespace gigaboot
