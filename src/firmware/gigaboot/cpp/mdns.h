// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_GIGABOOT_CPP_MDNS_H_
#define SRC_FIRMWARE_GIGABOOT_CPP_MDNS_H_

#include <type_traits>
#include <variant>

#include <fbl/vector.h>

#include "network.h"
#include "utils.h"

namespace gigaboot {

struct DnsHeader {
  uint16_t id;
  uint16_t flags;
  uint16_t question_count;
  uint16_t answer_count;
  uint16_t authority_count;
  uint16_t additional_count;
};
static_assert(sizeof(DnsHeader) == 12);

// The ethernet header is implicitly added, but its length still counts towards the payload.
// That header has three fields: source MAC, destination MAC, and 2 octet ethertype.
// Each protocol layer header also counts towards the maximum.
constexpr size_t kDnsMaxPayload = kEthMTU - kMacAddrLen - kMacAddrLen - sizeof(uint16_t) -
                                  sizeof(Ip6Header) - sizeof(UdpHeader) - sizeof(DnsHeader);
using DnsPayload = std::array<uint8_t, kDnsMaxPayload>;

constexpr Ip6Addr kIpV6MdnsDestAddr = {
    0xFF, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFB,
};
constexpr MacAddr kEthMdnsDestAddr = MulticastMacFromIp6(kIpV6MdnsDestAddr);

constexpr uint16_t kMdnsFlagQueryResponse = 0x8000;
constexpr uint16_t kMdnsFlagAuthoritative = 0x400;
constexpr uint16_t kFbServerPort = 5554;
constexpr uint16_t kMdnsPort = 5353;

constexpr size_t kDeviceIdMaxLen = 24;
using DeviceName = std::array<char, kDeviceIdMaxLen>;

// DNS (and mDNS) names are defined using a graph structure a little like an inverted tree: there
// are many entry leaves, and at each higher layer the `next` segment aggregates leaves feeding
// inwards. The "root" of the tree is an implicit, empty string. It can be replaced with a null
// `next` in the graph representation. DNS queries use a simple length+string representation for
// serializing each name segment. The length field is 1 octet and has a maximum value of 63, or
// 0b00111111. The root is indicated with a length of zero. Joining dots are implicit.
//
// Example representation for able.baker.charlie.com:
// DnsNameSegment example[] = {
//   {.name = "able", .loc = 0, .next = &example[1]},
//   {.name = "baker", .loc = 0, .next = &example[2]},
//   {.name = "charlie", .loc = 0, .next = &example[3]},
//   {.name = "com", .loc = 0, .next = nullptr},
// };
// DnsNameSegment* server = &example[0];
//
// Which serializes to
// 4 a b l e 5 b a k e r 7 c h a r l i e 3 c o m 0
//
// If the two most significant bits of the serialized length are set, then the length
// is instead a two octet offset field within the DNS payload describing the
// continuation of the fully qualified domain name.
//
// i.e. deserialization might look something like this
// uint8_t length = *reinterpret_cast<uint8_t*>(ptr);
// if(length =< 63){
//     // Uncompressed string segment
// } else if (length & 0xC0){
//     uint16_t offset = ntohs(*reinterpret_cast<uint16_t*>(ptr)) & ~0xC000;
//     length = *reinterpret_cast<uint8_t*>(payload + offset);
// } else {
//     // error, inconsistent length.
// }
//
// The DnsNameSegment `loc` is then specific to a serialized DNS payload.
// It depends on what names were previously serialized in the payload and in what order.
// The `loc` field is cleared at the beginning of serialization
// and is set as part of serialization.
struct DnsNameSegment {
  std::string_view name;
  uint16_t loc;
  DnsNameSegment* next;
};

struct DnsPtrRecord {
  constexpr static uint16_t kType = 12;
  DnsNameSegment* name;
};

struct DnsAAAARecord {
  constexpr static uint16_t kType = 28;
  Ip6Addr addr;
};

struct DnsSrvRecord {
  constexpr static uint16_t kType = 33;
  uint16_t priority;
  uint16_t weight;
  uint16_t port;
  DnsNameSegment* target;
};

struct DnsRecord {
  DnsNameSegment* name;
  uint16_t record_class;
  // Note: any additional DNS record types added MUST define a kType constant
  //       and MUST be added to the the types in the `data_` variant.
  std::variant<DnsPtrRecord, DnsAAAARecord, DnsSrvRecord> data;
};

// Forward declaration.
class MdnsPacket;

class DnsContext {
  // Context for DNS serialization.
  // This class owns name segments and records.
  // Name segments are modified as part of serialization due to DNS name compression,
  // and records hold pointers to name segments.
  // This class Wraps up that data so that structures are modified in a consistent manner
  // that preserves invariants.
 public:
  DnsContext(fbl::Vector<DnsNameSegment> segments, fbl::Vector<DnsRecord> records)
      : name_segments_(std::move(segments)), records_(std::move(records)) {}

  // Serialize all owned mdns records to the packet.
  // Returns true if the packet had sufficient free space and the write succeeded, false otherwise.
  bool WriteRecords(zx::duration time_to_live, MdnsPacket& packet);

 private:
  // Serialize a fully-qualified domain name (FQDN) to the packet.
  // Returns true if the packet had sufficient free space and the write succeeded, false otherwise.
  bool WriteFQDN(DnsNameSegment& segment, MdnsPacket& packet);

  // Serialize an mdns record to the packet.
  // Returns true if the packet was not full and the write succeeded, false otherwise.
  bool WriteRecord(zx::duration time_to_live, DnsRecord& record, MdnsPacket& packet);

  fbl::Vector<DnsNameSegment> name_segments_;
  fbl::Vector<DnsRecord> records_;
};

class MdnsPacket {
 public:
  MdnsPacket(const Ip6Addr& src,
             const DnsHeader& dns_header,
             fbl::Vector<DnsNameSegment> segments,
             fbl::Vector<DnsRecord> records)
      : data_{
          // Don't set the header size on construction.
          // It will be backpatched when the packet is serialized.
          {0, kUdpHdr, src, kIpV6MdnsDestAddr},
          // Don't set the header size or checksum on construction.
          // They will be backpatched when the packet is serialized.
          {htons(kMdnsPort), htons(kMdnsPort), 0, 0},
          dns_header,
          // Zero-initialize the payload for safety.
          {},
        },
        payload_end_(data_.payload.begin()),
        time_to_live_(std::nullopt),
        context_(std::move(segments), std::move(records)) {}

  // Lazily serialize the packet using the passed in time to live value.
  //
  // Return a span describing the serialized packet on success, EFI_BUFFER_TOO_SMALL on error.
  fit::result<efi_status, std::span<const uint8_t>> Serialize(zx::duration time_to_live);

  // Reset the payload end pointer and cached time-to-live.
  // Does NOT blank the payload contents and does NOT reset any headers.
  void ResetPayload() {
    time_to_live_ = std::nullopt;
    payload_end_ = data_.payload.begin();
  }

 private:
  friend DnsContext;

  // Calculate payload lengths and update corresponding headers.
  // Update the UDP checksum.
  // Returns a span on the contiguous span of bytes that is the
  // serialized representation of the packet.
  std::span<const uint8_t> Finalize();

  DnsPayload::iterator CurrentEnd() const { return payload_end_; }

  // Return the number of bytes currently used by DNS.
  // Includes the DNS header and any data written.
  size_t DnsBytesUsed() const {
    return payload_end_ - data_.payload.begin() + sizeof(data_.dns_header);
  }

  // If the DNS payload isn't full, write a byte and update the end iterator.
  // Return true if the payload wasn't full and the write succeeded, false otherwise.
  bool WriteU8(uint8_t c);

  // If the DNS payload isn't full, convert s to network byte order,
  // write it to the payload, and update the end iterator.
  // Return true if the payload wasn't full and the write succeeded, false otherwise.
  bool WriteU16(uint16_t s);

  // If the DNS payload isn't full, convert l to network byte order,
  // write it to the payload, and update the end iterator.
  // Return true if the payload wasn't full and the write succeeded, false otherwise.
  bool WriteU32(uint32_t l);

  // Write a stream of bytes to the payload.
  // Return true if the payload wasn't full and the write succeeded, false otherwise.
  bool WriteBytes(std::span<const uint8_t> b);

  // Inner container for the serialized data.
  // The headers don't need complicated logic to set up, for the most part.
  // The only complicated setup involves back-patching payload sizes
  // (which requires serializing the DNS records first)
  // and calculating the UDP checksum
  // (which requires all other data to be serialized).
  //
  // Keep this structure separate so that we can verify its properties
  // using static assertions.
  // These properties are necessary for efficient, memcpy based serialization.
  struct Data {
    Ip6Header ip6_header;
    UdpHeader udp_header;
    DnsHeader dns_header;
    DnsPayload payload;
  };
  static_assert(sizeof(Data) == kEthMTU - sizeof(MacAddr) - sizeof(MacAddr) - sizeof(uint16_t),
                "DNS packet size not equal to ethernet MTU");
  static_assert(sizeof(Data) ==
                    sizeof(Ip6Header) + sizeof(UdpHeader) + sizeof(DnsHeader) + sizeof(DnsPayload),
                "DNS packet is not densely packed, cannot serialize naively");
  static_assert(std::is_standard_layout_v<Data>,
                "DNS packet data is not standard layout: cannot serialize naively");
  Data data_;

  // The current end of `data.payload`.
  // The span returned by `Finalize` or `Serialize` has a start of `data` and an end of
  // `payload_end_`.
  DnsPayload::iterator payload_end_;

  // Cached value of the last used time-to-live.
  // If the cache is empty or the previous used value
  // doesn't match the proposed TTL, we need to reserialize the entire packet.
  std::optional<zx::duration> time_to_live_;

  // Holds more complicated, non-serialized data.
  DnsContext context_;
};

class MdnsAgent {
 public:
  static constexpr zx::duration kMdnsPollPeriod = zx::sec(10);
  static constexpr zx::duration kMdnsTtl = zx::sec(120);

  // Can't move or copy due to the timer.
  MdnsAgent(EthernetAgent& eth_agent, efi_system_table* sys);
  ~MdnsAgent();

  // If the poll timer has expired, send a broadcast mDNS packet describing this host and its
  // fastboot services. Does nothing if the timer has not expired.
  //
  // Returns fit::ok on success (or if the timer hasn't expired).
  // Can return an error if
  // *) Lazy initialization has failed.
  // *) Packet serialization has failed.
  // *) Transmission has failed.
  fit::result<efi_status> Poll();

  const DeviceName& DeviceName() const { return device_name_; }

 private:
  // Lazily create the transmission callback.
  // The transmission callback is responsible for resetting the poll timer after
  // transmission has completed.
  // Returns an error if the callback event initialization failed.
  fit::result<efi_status, efi_event> LazySetup();

  EthernetAgent& eth_agent_;
  ::gigaboot::DeviceName device_name_;
  Timer timer_;

  // The completion token is passed to SendV6Frame and needs to outlive the call.
  efi_managed_network_sync_completion_token token_;

  std::unique_ptr<MdnsPacket> packet_;

  // Set these attributes lazily so that the constructor doesn't need to handle
  // error cases.
  efi_event tx_callback_event_;
};

}  // namespace gigaboot

#endif  // SRC_FIRMWARE_GIGABOOT_CPP_MDNS_H_
