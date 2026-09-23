// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async-testing/test_loop.h>
#include <lib/syslog/cpp/macros.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/socket.h>

#include <iomanip>
#include <iostream>

#include <gtest/gtest.h>

#include "src/connectivity/network/mdns/service/common/types.h"
#include "src/connectivity/network/mdns/service/encoding/dns_reading.h"
#include "src/connectivity/network/mdns/service/encoding/dns_writing.h"
#include "src/connectivity/network/mdns/service/encoding/packet_reader.h"
#include "src/connectivity/network/mdns/service/encoding/packet_writer.h"
#include "src/connectivity/network/mdns/service/transport/mdns_interface_transceiver.h"
#include "src/lib/fostr/hex_dump.h"

namespace mdns::test {

class MdnsInterfaceTransceiverTest : public MdnsInterfaceTransceiver {
 public:
  MdnsInterfaceTransceiverTest(inet::IpAddress address, const std::string& name, uint32_t id,
                               Media media)
      : MdnsInterfaceTransceiver(address, name, id, media),
        ip_versions_(address.is_v4() ? IpVersions::kV4 : IpVersions::kV6) {}

  virtual ~MdnsInterfaceTransceiverTest() override {}

  bool Start(InboundMessageCallback callback) override {
    set_inbound_message_callback(std::move(callback));
    WaitForInbound();
    return true;
  }

  using MdnsInterfaceTransceiver::InboundReady;

  // Configured by the test before calling InboundReady.
  ssize_t receive_message_result_ = 0;
  std::vector<uint8_t> receive_message_packet_;
  inet::SocketAddress receive_message_source_address_;
  inet::IpAddress receive_message_destination_address_;

  // Set by ReceiveMessage during test.
  int receive_message_sockfd_ = -1;
  int receive_message_flags_ = -1;
  uint32_t receive_message_count_ = 0;
  uint32_t wait_for_inbound_count_ = 0;

  // Set by |SendTo|.
  const void* send_to_buffer_{};
  size_t send_to_size_{};
  inet::SocketAddress send_to_address_{};

  // Dumps a golden for |SendTo|.
  void DumpSendToGolden() {
    FX_CHECK(send_to_buffer_ != nullptr);
    FX_CHECK(send_to_size_ != 0);

    std::cout << fostr::HexDump(send_to_buffer_, send_to_size_, 0) << "\n\n";

    std::cout << "  std::vector<uint8_t> expected_message = {";

    for (size_t i = 0; i < send_to_size_; ++i) {
      if (i % 12 == 0) {
        std::cout << "\n      ";
      }

      std::cout << "0x" << std::hex << std::setw(2) << std::setfill('0')
                << static_cast<uint16_t>(reinterpret_cast<const uint8_t*>(send_to_buffer_)[i])
                << std::dec << ", ";
    }

    std::cout << "};\n";
  }

 protected:
  // MdnsInterfaceTransceiver overrides.
  enum IpVersions IpVersions() override { return ip_versions_; }
  int SetOptionDisableMulticastLoop() override { return 0; }
  int SetOptionJoinMulticastGroup() override { return 0; }
  int SetOptionOutboundInterface() override { return 0; }
  int SetOptionUnicastTtl() override { return 0; }
  int SetOptionMulticastTtl() override { return 0; }
  int SetOptionFamilySpecific() override { return 0; }
  int Bind() override { return 0; }

  ssize_t SendTo(const void* buffer, size_t size, const inet::SocketAddress& address) override {
    send_to_buffer_ = buffer;
    send_to_size_ = size;
    send_to_address_ = address;
    return 0;
  }

  void WaitForInbound() override { ++wait_for_inbound_count_; }

  ssize_t ReceiveMessage(int sockfd, struct msghdr* msg, int flags) override {
    ++receive_message_count_;
    receive_message_sockfd_ = sockfd;
    receive_message_flags_ = flags;

    if (receive_message_result_ < 0) {
      return receive_message_result_;
    }

    // Copy packet payload
    size_t copy_size = std::min(receive_message_packet_.size(), msg->msg_iov[0].iov_len);
    std::memcpy(msg->msg_iov[0].iov_base, receive_message_packet_.data(), copy_size);

    // Set source address
    if (receive_message_source_address_.is_v4()) {
      auto* sin = reinterpret_cast<sockaddr_in*>(msg->msg_name);
      *sin = receive_message_source_address_.as_sockaddr_in();
      msg->msg_namelen = sizeof(sockaddr_in);
    } else {
      auto* sin6 = reinterpret_cast<sockaddr_in6*>(msg->msg_name);
      *sin6 = receive_message_source_address_.as_sockaddr_in6();
      msg->msg_namelen = sizeof(sockaddr_in6);
    }

    // Set destination address in control message
    if (receive_message_destination_address_.is_v4()) {
      cmsghdr* cmsg = CMSG_FIRSTHDR(msg);
      cmsg->cmsg_level = IPPROTO_IP;
      cmsg->cmsg_type = IP_PKTINFO;
      cmsg->cmsg_len = CMSG_LEN(sizeof(in_pktinfo));

      auto* pktinfo = reinterpret_cast<in_pktinfo*>(CMSG_DATA(cmsg));
      pktinfo->ipi_addr = receive_message_destination_address_.as_in_addr();
      msg->msg_controllen = CMSG_SPACE(sizeof(in_pktinfo));
    } else if (receive_message_destination_address_.is_v6()) {
      cmsghdr* cmsg = CMSG_FIRSTHDR(msg);
      cmsg->cmsg_level = IPPROTO_IPV6;
      cmsg->cmsg_type = IPV6_PKTINFO;
      cmsg->cmsg_len = CMSG_LEN(sizeof(in6_pktinfo));

      auto* pktinfo = reinterpret_cast<in6_pktinfo*>(CMSG_DATA(cmsg));
      pktinfo->ipi6_addr = receive_message_destination_address_.as_in6_addr();
      msg->msg_controllen = CMSG_SPACE(sizeof(in6_pktinfo));
    } else {
      msg->msg_controllen = 0;
    }

    return static_cast<ssize_t>(copy_size);
  }

 private:
  enum IpVersions ip_versions_;
};

// Constructs an |MdnsInterfaceTransceiverTest| and checks the values of its
// identifying properties.
TEST(InterfaceTransceiverTest, Construct) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  inet::IpAddress nic_address(1, 2, 3, 4);
  std::string nic_name = "testnic";
  uint32_t nic_id = 1234;

  MdnsInterfaceTransceiverTest under_test(nic_address, nic_name, nic_id, Media::kWired);
  under_test.SetInterfaceAddresses({nic_address});

  EXPECT_EQ(nic_address, under_test.address());
  EXPECT_EQ(nic_name, under_test.name());
  EXPECT_EQ(nic_id, under_test.id());
  EXPECT_EQ(Media::kWired, under_test.media());
}

// Sends a message containing no A or AAAA resources.
TEST(InterfaceTransceiverTest, SendSimpleMessage) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  inet::IpAddress nic_address(1, 2, 3, 4);
  std::string nic_name = "testnic";
  uint32_t nic_id = 1234;

  inet::SocketAddress to_address(inet::IpAddress(4, 3, 2, 1), inet::IpPort::From_uint16_t(4321));

  MdnsInterfaceTransceiverTest under_test(nic_address, nic_name, nic_id, Media::kWired);
  under_test.SetInterfaceAddresses({nic_address});

  auto ptr_resource =
      std::make_shared<DnsResource>(DnsName("_test_name._whatever."), DnsType::kPtr);
  ptr_resource->time_to_live_ = 234;
  ptr_resource->ptr_.pointer_domain_name_ = DnsName("_test_ptr_name._whatever.");

  DnsMessage message;
  message.additionals_.push_back(ptr_resource);
  message.UpdateCounts();

  under_test.SendMessage(message, to_address);
  EXPECT_NE(nullptr, under_test.send_to_buffer_);

  // 0000  00 00 00 00 00 00 00 00  00 00 00 01 0a 5f 74 65  ............._te
  // 0010  73 74 5f 6e 61 6d 65 09  5f 77 68 61 74 65 76 65  st_name._whateve
  // 0020  72 00 00 0c 00 01 00 00  00 ea 00 11 0e 5f 74 65  r............_te
  // 0030  73 74 5f 70 74 72 5f 6e  61 6d 65 c0 17           st_ptr_name..

  std::vector<uint8_t> expected_message = {
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x0a,
      0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0x09, 0x5f, 0x77,
      0x68, 0x61, 0x74, 0x65, 0x76, 0x65, 0x72, 0x00, 0x00, 0x0c, 0x00, 0x01, 0x00,
      0x00, 0x00, 0xea, 0x00, 0x11, 0x0e, 0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x70,
      0x74, 0x72, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0, 0x17};

  EXPECT_EQ(expected_message.size(), under_test.send_to_size_);
  EXPECT_EQ(0,
            memcmp(expected_message.data(), under_test.send_to_buffer_, under_test.send_to_size_));
  EXPECT_EQ(to_address, under_test.send_to_address_);
}

// Sends a message containing a leading A resource.
TEST(InterfaceTransceiverTest, SendLeadingA) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  inet::IpAddress nic_address(1, 2, 3, 4);
  std::string nic_name = "testnic";
  uint32_t nic_id = 1234;

  inet::SocketAddress to_address(inet::IpAddress(4, 3, 2, 1), inet::IpPort::From_uint16_t(4321));

  MdnsInterfaceTransceiverTest under_test(nic_address, nic_name, nic_id, Media::kWired);
  under_test.SetInterfaceAddresses({nic_address});

  auto a_resource = std::make_shared<DnsResource>(DnsName("_test_a_name._whatever."), DnsType::kA);

  auto ptr_resource =
      std::make_shared<DnsResource>(DnsName("_test_name._whatever."), DnsType::kPtr);
  ptr_resource->time_to_live_ = 234;
  ptr_resource->ptr_.pointer_domain_name_ = DnsName("_test_ptr_name._whatever.");

  DnsMessage message;
  message.additionals_.push_back(a_resource);
  message.additionals_.push_back(ptr_resource);
  message.UpdateCounts();

  under_test.SendMessage(message, to_address);
  EXPECT_NE(nullptr, under_test.send_to_buffer_);

  // under_test.DumpSendToGolden();

  // 0000  00 00 00 00 00 00 00 00  00 00 00 02 0a 5f 74 65  ............._te
  // 0010  73 74 5f 6e 61 6d 65 09  5f 77 68 61 74 65 76 65  st_name._whateve
  // 0020  72 00 00 0c 00 01 00 00  00 ea 00 11 0e 5f 74 65  r............_te
  // 0030  73 74 5f 70 74 72 5f 6e  61 6d 65 c0 17 0c 5f 74  st_ptr_name..._t
  // 0040  65 73 74 5f 61 5f 6e 61  6d 65 c0 17 00 01 80 01  est_a_name......
  // 0050  00 00 00 78 00 04 01 02  03 04                    ...x......

  std::vector<uint8_t> expected_message = {
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x0a, 0x5f, 0x74,
      0x65, 0x73, 0x74, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0x09, 0x5f, 0x77, 0x68, 0x61, 0x74, 0x65,
      0x76, 0x65, 0x72, 0x00, 0x00, 0x0c, 0x00, 0x01, 0x00, 0x00, 0x00, 0xea, 0x00, 0x11, 0x0e,
      0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x70, 0x74, 0x72, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0,
      0x17, 0x0c, 0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x61, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0,
      0x17, 0x00, 0x01, 0x80, 0x01, 0x00, 0x00, 0x00, 0x78, 0x00, 0x04, 0x01, 0x02, 0x03, 0x04,
  };

  EXPECT_EQ(expected_message.size(), under_test.send_to_size_);
  EXPECT_EQ(0,
            memcmp(expected_message.data(), under_test.send_to_buffer_, under_test.send_to_size_));
  EXPECT_EQ(to_address, under_test.send_to_address_);
}

// Sends a message containing leading A and AAAA resources.
TEST(InterfaceTransceiverTest, SendLeadingAAndAAAA) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  inet::IpAddress nic_address(1, 2, 3, 4);
  std::string nic_name = "testnic";
  uint32_t nic_id = 1234;

  inet::SocketAddress to_address(inet::IpAddress(4, 3, 2, 1), inet::IpPort::From_uint16_t(4321));

  MdnsInterfaceTransceiverTest under_test(nic_address, nic_name, nic_id, Media::kWired);
  under_test.SetInterfaceAddresses({nic_address});

  auto a_resource = std::make_shared<DnsResource>(DnsName("_test_a_name._whatever."), DnsType::kA);

  auto aaaa_resource =
      std::make_shared<DnsResource>(DnsName("_test_a_name._whatever."), DnsType::kAaaa);

  auto ptr_resource =
      std::make_shared<DnsResource>(DnsName("_test_name._whatever."), DnsType::kPtr);
  ptr_resource->time_to_live_ = 234;
  ptr_resource->ptr_.pointer_domain_name_ = DnsName("_test_ptr_name._whatever.");

  DnsMessage message;
  message.additionals_.push_back(a_resource);
  message.additionals_.push_back(aaaa_resource);
  message.additionals_.push_back(ptr_resource);
  message.UpdateCounts();

  under_test.SendMessage(message, to_address);
  EXPECT_NE(nullptr, under_test.send_to_buffer_);

  // under_test.DumpSendToGolden();

  // 0000  00 00 00 00 00 00 00 00  00 00 00 02 0a 5f 74 65  ............._te
  // 0010  73 74 5f 6e 61 6d 65 09  5f 77 68 61 74 65 76 65  st_name._whateve
  // 0020  72 00 00 0c 00 01 00 00  00 ea 00 11 0e 5f 74 65  r............_te
  // 0030  73 74 5f 70 74 72 5f 6e  61 6d 65 c0 17 0c 5f 74  st_ptr_name..._t
  // 0040  65 73 74 5f 61 5f 6e 61  6d 65 c0 17 00 01 80 01  est_a_name......
  // 0050  00 00 00 78 00 04 01 02  03 04                    ...x......

  std::vector<uint8_t> expected_message = {
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x0a, 0x5f, 0x74,
      0x65, 0x73, 0x74, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0x09, 0x5f, 0x77, 0x68, 0x61, 0x74, 0x65,
      0x76, 0x65, 0x72, 0x00, 0x00, 0x0c, 0x00, 0x01, 0x00, 0x00, 0x00, 0xea, 0x00, 0x11, 0x0e,
      0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x70, 0x74, 0x72, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0,
      0x17, 0x0c, 0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x61, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0,
      0x17, 0x00, 0x01, 0x80, 0x01, 0x00, 0x00, 0x00, 0x78, 0x00, 0x04, 0x01, 0x02, 0x03, 0x04,
  };

  EXPECT_EQ(expected_message.size(), under_test.send_to_size_);
  EXPECT_EQ(0,
            memcmp(expected_message.data(), under_test.send_to_buffer_, under_test.send_to_size_));
  EXPECT_EQ(to_address, under_test.send_to_address_);
}

// Sends a message containing trailing A and AAAA resources.
TEST(InterfaceTransceiverTest, SendTrailingAAndAAAA) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  inet::IpAddress nic_address(1, 2, 3, 4);
  std::string nic_name = "testnic";
  uint32_t nic_id = 1234;

  inet::SocketAddress to_address(inet::IpAddress(4, 3, 2, 1), inet::IpPort::From_uint16_t(4321));

  MdnsInterfaceTransceiverTest under_test(nic_address, nic_name, nic_id, Media::kWired);
  under_test.SetInterfaceAddresses({nic_address});

  auto a_resource = std::make_shared<DnsResource>(DnsName("_test_a_name._whatever."), DnsType::kA);

  auto aaaa_resource =
      std::make_shared<DnsResource>(DnsName("_test_a_name._whatever."), DnsType::kAaaa);

  auto ptr_resource =
      std::make_shared<DnsResource>(DnsName("_test_name._whatever."), DnsType::kPtr);
  ptr_resource->time_to_live_ = 234;
  ptr_resource->ptr_.pointer_domain_name_ = DnsName("_test_ptr_name._whatever.");

  DnsMessage message;
  message.additionals_.push_back(ptr_resource);
  message.additionals_.push_back(a_resource);
  message.additionals_.push_back(aaaa_resource);
  message.UpdateCounts();

  under_test.SendMessage(message, to_address);
  EXPECT_NE(nullptr, under_test.send_to_buffer_);

  // under_test.DumpSendToGolden();

  // 0000  00 00 00 00 00 00 00 00  00 00 00 02 0a 5f 74 65  ............._te
  // 0010  73 74 5f 6e 61 6d 65 09  5f 77 68 61 74 65 76 65  st_name._whateve
  // 0020  72 00 00 0c 00 01 00 00  00 ea 00 11 0e 5f 74 65  r............_te
  // 0030  73 74 5f 70 74 72 5f 6e  61 6d 65 c0 17 0c 5f 74  st_ptr_name..._t
  // 0040  65 73 74 5f 61 5f 6e 61  6d 65 c0 17 00 01 80 01  est_a_name......
  // 0050  00 00 00 78 00 04 01 02  03 04                    ...x......

  std::vector<uint8_t> expected_message = {
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x0a, 0x5f, 0x74,
      0x65, 0x73, 0x74, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0x09, 0x5f, 0x77, 0x68, 0x61, 0x74, 0x65,
      0x76, 0x65, 0x72, 0x00, 0x00, 0x0c, 0x00, 0x01, 0x00, 0x00, 0x00, 0xea, 0x00, 0x11, 0x0e,
      0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x70, 0x74, 0x72, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0,
      0x17, 0x0c, 0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x61, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0,
      0x17, 0x00, 0x01, 0x80, 0x01, 0x00, 0x00, 0x00, 0x78, 0x00, 0x04, 0x01, 0x02, 0x03, 0x04,
  };

  EXPECT_EQ(expected_message.size(), under_test.send_to_size_);
  EXPECT_EQ(0,
            memcmp(expected_message.data(), under_test.send_to_buffer_, under_test.send_to_size_));
  EXPECT_EQ(to_address, under_test.send_to_address_);
}

// Sends a message containing bracketing A and AAAA resources.
TEST(InterfaceTransceiverTest, SendBracketingAAndAAAA) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  inet::IpAddress nic_address(1, 2, 3, 4);
  std::string nic_name = "testnic";
  uint32_t nic_id = 1234;

  inet::SocketAddress to_address(inet::IpAddress(4, 3, 2, 1), inet::IpPort::From_uint16_t(4321));

  MdnsInterfaceTransceiverTest under_test(nic_address, nic_name, nic_id, Media::kWired);
  under_test.SetInterfaceAddresses({nic_address});

  auto a_resource = std::make_shared<DnsResource>(DnsName("_test_a_name._whatever."), DnsType::kA);

  auto aaaa_resource =
      std::make_shared<DnsResource>(DnsName("_test_a_name._whatever."), DnsType::kAaaa);

  auto ptr_resource =
      std::make_shared<DnsResource>(DnsName("_test_name._whatever."), DnsType::kPtr);
  ptr_resource->time_to_live_ = 234;
  ptr_resource->ptr_.pointer_domain_name_ = DnsName("_test_ptr_name._whatever.");

  DnsMessage message;
  message.additionals_.push_back(a_resource);
  message.additionals_.push_back(ptr_resource);
  message.additionals_.push_back(aaaa_resource);
  message.UpdateCounts();

  under_test.SendMessage(message, to_address);
  EXPECT_NE(nullptr, under_test.send_to_buffer_);

  // under_test.DumpSendToGolden();

  // 0000  00 00 00 00 00 00 00 00  00 00 00 02 0a 5f 74 65  ............._te
  // 0010  73 74 5f 6e 61 6d 65 09  5f 77 68 61 74 65 76 65  st_name._whateve
  // 0020  72 00 00 0c 00 01 00 00  00 ea 00 11 0e 5f 74 65  r............_te
  // 0030  73 74 5f 70 74 72 5f 6e  61 6d 65 c0 17 0c 5f 74  st_ptr_name..._t
  // 0040  65 73 74 5f 61 5f 6e 61  6d 65 c0 17 00 01 80 01  est_a_name......
  // 0050  00 00 00 78 00 04 01 02  03 04                    ...x......

  std::vector<uint8_t> expected_message = {
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x0a, 0x5f, 0x74,
      0x65, 0x73, 0x74, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0x09, 0x5f, 0x77, 0x68, 0x61, 0x74, 0x65,
      0x76, 0x65, 0x72, 0x00, 0x00, 0x0c, 0x00, 0x01, 0x00, 0x00, 0x00, 0xea, 0x00, 0x11, 0x0e,
      0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x70, 0x74, 0x72, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0,
      0x17, 0x0c, 0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x61, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0,
      0x17, 0x00, 0x01, 0x80, 0x01, 0x00, 0x00, 0x00, 0x78, 0x00, 0x04, 0x01, 0x02, 0x03, 0x04,
  };

  EXPECT_EQ(expected_message.size(), under_test.send_to_size_);
  EXPECT_EQ(0,
            memcmp(expected_message.data(), under_test.send_to_buffer_, under_test.send_to_size_));
  EXPECT_EQ(to_address, under_test.send_to_address_);
}

// Sends a message containing a leading A resource with alternate address.
TEST(InterfaceTransceiverTest, SendLeadingAWithAlternate) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  inet::IpAddress nic_address(1, 2, 3, 4);
  inet::IpAddress alternate_address(1, 2);
  std::string nic_name = "testnic";
  uint32_t nic_id = 1234;

  inet::SocketAddress to_address(inet::IpAddress(4, 3, 2, 1), inet::IpPort::From_uint16_t(4321));

  MdnsInterfaceTransceiverTest under_test(nic_address, nic_name, nic_id, Media::kWired);
  under_test.SetInterfaceAddresses({nic_address, alternate_address});

  auto a_resource = std::make_shared<DnsResource>(DnsName("_test_a_name._whatever."), DnsType::kA);

  auto ptr_resource =
      std::make_shared<DnsResource>(DnsName("_test_name._whatever."), DnsType::kPtr);
  ptr_resource->time_to_live_ = 234;
  ptr_resource->ptr_.pointer_domain_name_ = DnsName("_test_ptr_name._whatever.");

  DnsMessage message;
  message.additionals_.push_back(a_resource);
  message.additionals_.push_back(ptr_resource);
  message.UpdateCounts();

  under_test.SendMessage(message, to_address);
  EXPECT_NE(nullptr, under_test.send_to_buffer_);

  // under_test.DumpSendToGolden();

  // 0000  00 00 00 00 00 00 00 00  00 00 00 03 0a 5f 74 65  ............._te
  // 0010  73 74 5f 6e 61 6d 65 09  5f 77 68 61 74 65 76 65  st_name._whateve
  // 0020  72 00 00 0c 00 01 00 00  00 ea 00 11 0e 5f 74 65  r............_te
  // 0030  73 74 5f 70 74 72 5f 6e  61 6d 65 c0 17 0c 5f 74  st_ptr_name..._t
  // 0040  65 73 74 5f 61 5f 6e 61  6d 65 c0 17 00 01 80 01  est_a_name......
  // 0050  00 00 00 78 00 04 01 02  03 04 c0 3d 00 1c 80 01  ...x.......=....
  // 0060  00 00 00 78 00 10 00 01  00 00 00 00 00 00 00 00  ...x............
  // 0070  00 00 00 00 00 02                                 ......

  std::vector<uint8_t> expected_message = {
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x0a, 0x5f, 0x74,
      0x65, 0x73, 0x74, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0x09, 0x5f, 0x77, 0x68, 0x61, 0x74, 0x65,
      0x76, 0x65, 0x72, 0x00, 0x00, 0x0c, 0x00, 0x01, 0x00, 0x00, 0x00, 0xea, 0x00, 0x11, 0x0e,
      0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x70, 0x74, 0x72, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0,
      0x17, 0x0c, 0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x61, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0,
      0x17, 0x00, 0x01, 0x80, 0x01, 0x00, 0x00, 0x00, 0x78, 0x00, 0x04, 0x01, 0x02, 0x03, 0x04,
      0xc0, 0x3d, 0x00, 0x1c, 0x80, 0x01, 0x00, 0x00, 0x00, 0x78, 0x00, 0x10, 0x00, 0x01, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
  };

  EXPECT_EQ(expected_message.size(), under_test.send_to_size_);
  EXPECT_EQ(0,
            memcmp(expected_message.data(), under_test.send_to_buffer_, under_test.send_to_size_));
  EXPECT_EQ(to_address, under_test.send_to_address_);
}

// Sends a message containing a leading A resource with a late-arriving alternate address.
TEST(InterfaceTransceiverTest, SendLeadingAWithLateAlternate) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  inet::IpAddress nic_address(1, 2, 3, 4);
  inet::IpAddress alternate_address(1, 2);
  std::string nic_name = "testnic";
  uint32_t nic_id = 1234;

  inet::SocketAddress to_address(inet::IpAddress(4, 3, 2, 1), inet::IpPort::From_uint16_t(4321));

  MdnsInterfaceTransceiverTest under_test(nic_address, nic_name, nic_id, Media::kWired);
  under_test.SetInterfaceAddresses({nic_address});

  auto a_resource = std::make_shared<DnsResource>(DnsName("_test_a_name._whatever."), DnsType::kA);

  auto ptr_resource =
      std::make_shared<DnsResource>(DnsName("_test_name._whatever."), DnsType::kPtr);
  ptr_resource->time_to_live_ = 234;
  ptr_resource->ptr_.pointer_domain_name_ = DnsName("_test_ptr_name._whatever.");

  DnsMessage message;
  message.additionals_.push_back(a_resource);
  message.additionals_.push_back(ptr_resource);
  message.UpdateCounts();

  under_test.SendMessage(message, to_address);
  EXPECT_NE(nullptr, under_test.send_to_buffer_);

  // under_test.DumpSendToGolden();

  // 0000  00 00 00 00 00 00 00 00  00 00 00 02 0a 5f 74 65  ............._te
  // 0010  73 74 5f 6e 61 6d 65 09  5f 77 68 61 74 65 76 65  st_name._whateve
  // 0020  72 00 00 0c 00 01 00 00  00 ea 00 11 0e 5f 74 65  r............_te
  // 0030  73 74 5f 70 74 72 5f 6e  61 6d 65 c0 17 0c 5f 74  st_ptr_name..._t
  // 0040  65 73 74 5f 61 5f 6e 61  6d 65 c0 17 00 01 80 01  est_a_name......
  // 0050  00 00 00 78 00 04 01 02  03 04                    ...x......

  std::vector<uint8_t> expected_message = {
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x0a, 0x5f, 0x74,
      0x65, 0x73, 0x74, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0x09, 0x5f, 0x77, 0x68, 0x61, 0x74, 0x65,
      0x76, 0x65, 0x72, 0x00, 0x00, 0x0c, 0x00, 0x01, 0x00, 0x00, 0x00, 0xea, 0x00, 0x11, 0x0e,
      0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x70, 0x74, 0x72, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0,
      0x17, 0x0c, 0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x61, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0,
      0x17, 0x00, 0x01, 0x80, 0x01, 0x00, 0x00, 0x00, 0x78, 0x00, 0x04, 0x01, 0x02, 0x03, 0x04,
  };

  EXPECT_EQ(expected_message.size(), under_test.send_to_size_);
  EXPECT_EQ(0,
            memcmp(expected_message.data(), under_test.send_to_buffer_, under_test.send_to_size_));
  EXPECT_EQ(to_address, under_test.send_to_address_);

  // Alternate address set after |SendMessage| is called.
  under_test.SetInterfaceAddresses({nic_address, alternate_address});

  under_test.SendMessage(message, to_address);
  EXPECT_NE(nullptr, under_test.send_to_buffer_);

  // under_test.DumpSendToGolden();

  // 0000  00 00 00 00 00 00 00 00  00 00 00 03 0a 5f 74 65  ............._te
  // 0010  73 74 5f 6e 61 6d 65 09  5f 77 68 61 74 65 76 65  st_name._whateve
  // 0020  72 00 00 0c 00 01 00 00  00 ea 00 11 0e 5f 74 65  r............_te
  // 0030  73 74 5f 70 74 72 5f 6e  61 6d 65 c0 17 0c 5f 74  st_ptr_name..._t
  // 0040  65 73 74 5f 61 5f 6e 61  6d 65 c0 17 00 01 80 01  est_a_name......
  // 0050  00 00 00 78 00 04 01 02  03 04 c0 3d 00 1c 80 01  ...x.......=....
  // 0060  00 00 00 78 00 10 00 01  00 00 00 00 00 00 00 00  ...x............
  // 0070  00 00 00 00 00 02                                 ......

  expected_message = {
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x0a, 0x5f, 0x74,
      0x65, 0x73, 0x74, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0x09, 0x5f, 0x77, 0x68, 0x61, 0x74, 0x65,
      0x76, 0x65, 0x72, 0x00, 0x00, 0x0c, 0x00, 0x01, 0x00, 0x00, 0x00, 0xea, 0x00, 0x11, 0x0e,
      0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x70, 0x74, 0x72, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0,
      0x17, 0x0c, 0x5f, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x61, 0x5f, 0x6e, 0x61, 0x6d, 0x65, 0xc0,
      0x17, 0x00, 0x01, 0x80, 0x01, 0x00, 0x00, 0x00, 0x78, 0x00, 0x04, 0x01, 0x02, 0x03, 0x04,
      0xc0, 0x3d, 0x00, 0x1c, 0x80, 0x01, 0x00, 0x00, 0x00, 0x78, 0x00, 0x10, 0x00, 0x01, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
  };

  EXPECT_EQ(expected_message.size(), under_test.send_to_size_);
  EXPECT_EQ(0,
            memcmp(expected_message.data(), under_test.send_to_buffer_, under_test.send_to_size_));
  EXPECT_EQ(to_address, under_test.send_to_address_);
}

// Sends a message containing multiple A resources for different hostnames.
TEST(InterfaceTransceiverTest, SendMultipleAddresses) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  inet::IpAddress nic_address(1, 2, 3, 4);
  std::string nic_name = "testnic";
  uint32_t nic_id = 1234;

  inet::SocketAddress to_address(inet::IpAddress(4, 3, 2, 1), inet::IpPort::From_uint16_t(4321));

  MdnsInterfaceTransceiverTest under_test(nic_address, nic_name, nic_id, Media::kWired);
  under_test.SetInterfaceAddresses({nic_address});

  auto host1_a_resource = std::make_shared<DnsResource>(DnsName("host1.local."), DnsType::kA);
  auto host2_a_resource = std::make_shared<DnsResource>(DnsName("host2.local."), DnsType::kA);

  DnsMessage message;
  message.additionals_.push_back(host1_a_resource);
  message.additionals_.push_back(host2_a_resource);
  message.UpdateCounts();

  under_test.SendMessage(message, to_address);
  EXPECT_NE(nullptr, under_test.send_to_buffer_);

  // Deserialize the sent message to verify that both hostnames got their placeholder records
  // replaced.
  std::vector<uint8_t> sent_packet(
      reinterpret_cast<const uint8_t*>(under_test.send_to_buffer_),
      reinterpret_cast<const uint8_t*>(under_test.send_to_buffer_) + under_test.send_to_size_);

  PacketReader reader(sent_packet);
  reader.SetBytesRemaining(under_test.send_to_size_);
  DnsMessage sent_message;
  reader >> sent_message;

  ASSERT_TRUE(reader.complete());
  ASSERT_EQ(2u, sent_message.additionals_.size());

  auto res1 = sent_message.additionals_[0];
  EXPECT_EQ(DnsName("host1.local."), res1->name_);
  EXPECT_EQ(DnsType::kA, res1->type_);
  EXPECT_EQ(nic_address, res1->a_.address_.address_);

  auto res2 = sent_message.additionals_[1];
  EXPECT_EQ(DnsName("host2.local."), res2->name_);
  EXPECT_EQ(DnsType::kA, res2->type_);
  EXPECT_EQ(nic_address, res2->a_.address_.address_);
}

// Verifies that InboundReady handles a normal inbound packet correctly.
TEST(InterfaceTransceiverTest, InboundReadyNormal) {
  async::TestLoop loop;
  inet::IpAddress nic_address(1, 2, 3, 4);
  std::string nic_name = "testnic";
  uint32_t nic_id = 1234;

  MdnsInterfaceTransceiverTest under_test(nic_address, nic_name, nic_id, Media::kWired);

  // Setup a valid inbound message
  DnsMessage inbound_msg;
  inbound_msg.questions_.push_back(
      std::make_shared<DnsQuestion>(DnsName("test.local."), DnsType::kAny));
  inbound_msg.UpdateCounts();

  std::vector<uint8_t> buffer(1024);
  PacketWriter writer(std::move(buffer));
  writer << inbound_msg;
  size_t packet_size = writer.position();
  buffer = writer.GetPacket();
  buffer.resize(packet_size);

  under_test.receive_message_packet_ = std::move(buffer);
  under_test.receive_message_source_address_ =
      inet::SocketAddress(inet::IpAddress(169, 254, 1, 2), inet::IpPort::From_uint16_t(5353));
  under_test.receive_message_destination_address_ = MdnsAddresses::v4_multicast().address();

  bool callback_called = false;
  std::unique_ptr<DnsMessage> received_msg;
  ReplyAddress received_reply_address;
  under_test.Start([&](std::unique_ptr<DnsMessage> message, const ReplyAddress& reply_address) {
    callback_called = true;
    received_msg = std::move(message);
    received_reply_address = reply_address;
  });

  // Verify Start called WaitForInbound
  EXPECT_EQ(1u, under_test.wait_for_inbound_count_);

  under_test.InboundReady(ZX_OK, POLLIN);

  EXPECT_EQ(1u, under_test.receive_message_count_);
  EXPECT_EQ(2u, under_test.wait_for_inbound_count_);
  EXPECT_TRUE(callback_called);
  ASSERT_NE(nullptr, received_msg);
  ASSERT_EQ(1u, received_msg->questions_.size());
  EXPECT_EQ(DnsName("test.local."), received_msg->questions_[0]->name_);
  EXPECT_EQ(inet::IpAddress(169, 254, 1, 2), received_reply_address.socket_address().address());
}

// Verifies that a packet originating from outside the local segment is discarded if sent to a
// unicast address.
TEST(InterfaceTransceiverTest, InboundReadyDiscardFromOutsideLocalSegment) {
  async::TestLoop loop;
  inet::IpAddress nic_address(1, 2, 3, 4);
  std::string nic_name = "testnic";
  uint32_t nic_id = 1234;

  MdnsInterfaceTransceiverTest under_test(nic_address, nic_name, nic_id, Media::kWired);

  // Setup a valid inbound message
  DnsMessage inbound_msg;
  inbound_msg.questions_.push_back(
      std::make_shared<DnsQuestion>(DnsName("test.local."), DnsType::kAny));
  inbound_msg.UpdateCounts();

  std::vector<uint8_t> buffer(1024);
  PacketWriter writer(std::move(buffer));
  writer << inbound_msg;
  size_t packet_size = writer.position();
  buffer = writer.GetPacket();
  buffer.resize(packet_size);

  under_test.receive_message_packet_ = std::move(buffer);
  // Source address is 5.6.7.8 (not link-local)
  under_test.receive_message_source_address_ =
      inet::SocketAddress(inet::IpAddress(5, 6, 7, 8), inet::IpPort::From_uint16_t(5353));
  // Destination address is unicast address of the transceiver
  under_test.receive_message_destination_address_ = nic_address;

  bool callback_called = false;
  under_test.Start([&](std::unique_ptr<DnsMessage> message, const ReplyAddress& reply_address) {
    callback_called = true;
  });

  under_test.InboundReady(ZX_OK, POLLIN);

  EXPECT_EQ(1u, under_test.receive_message_count_);
  EXPECT_EQ(2u, under_test.wait_for_inbound_count_);
  // The callback should NOT have been called because the packet is discarded
  EXPECT_FALSE(callback_called);
}

// Verifies that unicast queries from outside the local segment to multicast are converted to
// multicast responses.
TEST(InterfaceTransceiverTest, InboundReadyForceMulticastResponseFromOutsideLocalSegment) {
  async::TestLoop loop;
  inet::IpAddress nic_address(1, 2, 3, 4);
  std::string nic_name = "testnic";
  uint32_t nic_id = 1234;

  MdnsInterfaceTransceiverTest under_test(nic_address, nic_name, nic_id, Media::kWired);

  // Setup a valid inbound message requesting unicast response (unicast_response_ = true)
  DnsMessage inbound_msg;
  inbound_msg.questions_.push_back(
      std::make_shared<DnsQuestion>(DnsName("test.local."), DnsType::kAny, true));
  inbound_msg.UpdateCounts();

  std::vector<uint8_t> buffer(1024);
  PacketWriter writer(std::move(buffer));
  writer << inbound_msg;
  size_t packet_size = writer.position();
  buffer = writer.GetPacket();
  buffer.resize(packet_size);

  under_test.receive_message_packet_ = std::move(buffer);
  // Source address is 5.6.7.8 (not link-local)
  under_test.receive_message_source_address_ =
      inet::SocketAddress(inet::IpAddress(5, 6, 7, 8), inet::IpPort::From_uint16_t(5353));
  // Destination address is multicast
  under_test.receive_message_destination_address_ = MdnsAddresses::v4_multicast().address();

  bool callback_called = false;
  std::unique_ptr<DnsMessage> received_msg;
  under_test.Start([&](std::unique_ptr<DnsMessage> message, const ReplyAddress& reply_address) {
    callback_called = true;
    received_msg = std::move(message);
  });

  under_test.InboundReady(ZX_OK, POLLIN);

  EXPECT_EQ(1u, under_test.receive_message_count_);
  EXPECT_EQ(2u, under_test.wait_for_inbound_count_);
  EXPECT_TRUE(callback_called);
  ASSERT_NE(nullptr, received_msg);
  ASSERT_EQ(1u, received_msg->questions_.size());
  // The unicast_response_ field of the question should have been forced to false
  EXPECT_FALSE(received_msg->questions_[0]->unicast_response_);
}

// Verifies that when ReceiveMessage fails, InboundReady schedules a task to call WaitForInbound.
TEST(InterfaceTransceiverTest, InboundReadyReceiveMessageFailure) {
  async::TestLoop loop;
  inet::IpAddress nic_address(1, 2, 3, 4);
  std::string nic_name = "testnic";
  uint32_t nic_id = 1234;

  MdnsInterfaceTransceiverTest under_test(nic_address, nic_name, nic_id, Media::kWired);

  under_test.receive_message_result_ = -1;

  bool callback_called = false;
  under_test.Start([&](std::unique_ptr<DnsMessage> message, const ReplyAddress& reply_address) {
    callback_called = true;
  });

  EXPECT_EQ(1u, under_test.wait_for_inbound_count_);

  under_test.InboundReady(ZX_OK, POLLIN);

  EXPECT_EQ(1u, under_test.receive_message_count_);
  // After a failed ReceiveMessage, WaitForInbound is called after a 10s delay.
  // So it shouldn't have been called immediately.
  EXPECT_EQ(1u, under_test.wait_for_inbound_count_);

  loop.RunFor(zx::sec(10));
  EXPECT_EQ(2u, under_test.wait_for_inbound_count_);
  EXPECT_FALSE(callback_called);
}

// Verifies IpSubnet properties and SetInterfaces with explicit IpSubnet.
TEST(InterfaceTransceiverTest, SetInterfacesWithIpSubnet) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  inet::IpAddress nic_address(1, 2, 3, 4);
  IpSubnet subnet(nic_address, 24);
  EXPECT_EQ(nic_address, subnet.address());
  EXPECT_EQ(24, subnet.prefix_len());
  EXPECT_EQ(subnet, IpSubnet(nic_address, 24));
  EXPECT_NE(subnet, IpSubnet(nic_address, 16));

  MdnsInterfaceTransceiverTest under_test(nic_address, "testnic", 1234, Media::kWired);
  under_test.SetInterfaceAddresses({subnet});
  EXPECT_EQ(nic_address, under_test.address());
}

TEST(InterfaceTransceiverTest, IpSubnetContains) {
  // IPv4 /24
  IpSubnet v4_subnet(inet::IpAddress(192, 168, 1, 1), 24);
  EXPECT_TRUE(v4_subnet.Contains(inet::IpAddress(192, 168, 1, 1)));
  EXPECT_TRUE(v4_subnet.Contains(inet::IpAddress(192, 168, 1, 254)));
  EXPECT_FALSE(v4_subnet.Contains(inet::IpAddress(192, 168, 2, 1)));

  // IPv4 /16
  IpSubnet v4_subnet16(inet::IpAddress(172, 16, 0, 1), 16);
  EXPECT_TRUE(v4_subnet16.Contains(inet::IpAddress(172, 16, 255, 254)));
  EXPECT_FALSE(v4_subnet16.Contains(inet::IpAddress(172, 17, 0, 1)));

  // IPv6 /64
  IpSubnet v6_subnet(inet::IpAddress(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1), 64);
  EXPECT_TRUE(v6_subnet.Contains(inet::IpAddress(0x2001, 0xdb8, 0, 0, 0, 0, 0, 2)));
  EXPECT_FALSE(v6_subnet.Contains(inet::IpAddress(0x2001, 0xdb9, 0, 0, 0, 0, 0, 1)));

  // Invalid / mismatched family
  EXPECT_FALSE(v4_subnet.Contains(inet::IpAddress::kInvalid));
  EXPECT_FALSE(v4_subnet.Contains(inet::IpAddress(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)));
}

TEST(InterfaceTransceiverTest, IsOnLocalSubnet) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  inet::IpAddress nic_address(192, 168, 1, 10);
  MdnsInterfaceTransceiverTest under_test(nic_address, "testnic", 1234, Media::kWired);

  // After setting interface_addresses_, addresses in those subnets are link local.
  under_test.SetInterfaceAddresses({IpSubnet(nic_address, 24)});
  EXPECT_TRUE(under_test.IsOnLocalSubnet(inet::IpAddress(192, 168, 1, 50)));
  EXPECT_FALSE(under_test.IsOnLocalSubnet(inet::IpAddress(192, 168, 2, 50)));
  EXPECT_FALSE(under_test.IsOnLocalSubnet(inet::IpAddress(5, 6, 7, 8)));
}

}  // namespace mdns::test
