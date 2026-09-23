// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "bt_transport_usb.h"

#include <fidl/fuchsia.hardware.bluetooth/cpp/fidl.h>
#include <fuchsia/hardware/usb/cpp/banjo.h>
#include <lib/async/cpp/task.h>
#include <lib/async/cpp/wait.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/thread_checker.h>
#include <lib/sync/condition.h>
#include <lib/sync/mutex.h>

#include <bind/fuchsia/bluetooth/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <gtest/gtest.h>
#include <usb/request-cpp.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/devices/usb/testing/descriptor-builder/descriptor-builder.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace {
namespace fhbt = fuchsia_hardware_bluetooth;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

constexpr uint16_t kVendorId = 1;
constexpr uint16_t kProductId = 2;
constexpr size_t kInterruptPacketSize = 255u;
constexpr uint16_t kScoMaxPacketSize = 255 + 3;  // payload + 3 bytes header
constexpr uint8_t kEventAndAclInterfaceNum = 0u;
constexpr uint8_t kIsocInterfaceNum = 1u;

using Request = usb::Request<void>;
using UnownedRequest = usb::BorrowedRequest<void>;
using UnownedRequestQueue = usb::BorrowedRequestQueue<void>;

// The test fixture initializes bt-transport-usb as a child device of FakeUsbDevice.
// FakeUsbDevice implements the ddk::UsbProtocol template interface. ddk::UsbProtocol forwards USB
// static function calls to the methods of this class.
class FakeUsbDevice : public ddk::UsbProtocol<FakeUsbDevice> {
 public:
  explicit FakeUsbDevice(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  void set_device_descriptor(usb::DeviceDescriptorBuilder& dev_builder) {
    device_descriptor_data_ = dev_builder.Generate();
  }

  void set_config_descriptor(usb::ConfigurationBuilder& config_builder) {
    config_descriptor_data_ = config_builder.Generate();
  }

  void ConfigureDefaultDescriptors(bool with_sco = true) {
    // Configure the USB endpoint configuration from Core Spec v5.3, Vol 4, Part B, Sec 2.1.1.

    // Interface 0 contains the bulk (ACL) and interrupt (HCI event) endpoints.
    // Endpoint indices are per direction (in/out).
    usb::InterfaceBuilder interface_0_builder(/*config_num=*/0);
    usb::EndpointBuilder bulk_in_endpoint_builder(
        /*config_num=*/0, static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
        /*endpoint_index=*/0, /*in=*/true);
    interface_0_builder.AddEndpoint(bulk_in_endpoint_builder);
    bulk_in_addr_ = usb::EpIndexToAddress(usb::kInEndpointStart);

    usb::EndpointBuilder bulk_out_endpoint_builder(
        /*config_num=*/0, static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
        /*endpoint_index=*/0, /*in=*/false);
    interface_0_builder.AddEndpoint(bulk_out_endpoint_builder);
    bulk_out_addr_ = usb::EpIndexToAddress(usb::kOutEndpointStart);

    usb::EndpointBuilder interrupt_endpoint_builder(
        /*config_num=*/0, static_cast<uint8_t>(fdescriptor::EndpointType::kInterrupt),
        /*endpoint_index=*/1, /*in=*/true);
    // The endpoint packet size must be large enough to send test packets.
    interrupt_endpoint_builder.set_max_packet_size(kInterruptPacketSize);
    interface_0_builder.AddEndpoint(interrupt_endpoint_builder);
    interrupt_addr_ = usb::EpIndexToAddress(usb::kInEndpointStart + 1);

    usb::ConfigurationBuilder config_builder(/*config_num=*/0);
    config_builder.AddInterface(interface_0_builder);

    if (with_sco) {
      for (uint8_t alt_setting = 0; alt_setting < 6; alt_setting++) {
        usb::InterfaceBuilder interface_1_builder(/*config_num=*/0, alt_setting);
        usb::EndpointBuilder isoc_out_endpoint_builder(
            /*config_num=*/0,
            static_cast<uint8_t>(fuchsia_hardware_usb_descriptor::EndpointType::kIsochronous),
            /*endpoint_index=*/1, /*in=*/false);
        isoc_out_endpoint_builder.set_max_packet_size(kScoMaxPacketSize);
        interface_1_builder.AddEndpoint(isoc_out_endpoint_builder);
        isoc_out_addr_ = usb::EpIndexToAddress(usb::kOutEndpointStart + 1);

        usb::EndpointBuilder isoc_in_endpoint_builder(
            /*config_num=*/0,
            static_cast<uint8_t>(fuchsia_hardware_usb_descriptor::EndpointType::kIsochronous),
            /*endpoint_index=*/2, /*in=*/true);
        isoc_in_endpoint_builder.set_max_packet_size(kScoMaxPacketSize);
        interface_1_builder.AddEndpoint(isoc_in_endpoint_builder);
        isoc_in_addr_ = usb::EpIndexToAddress(usb::kInEndpointStart + 2);
        config_builder.AddInterface(interface_1_builder);
      }
    }
    set_config_descriptor(config_builder);

    usb::DeviceDescriptorBuilder dev_builder;
    dev_builder.set_vendor_id(kVendorId);
    dev_builder.set_product_id(kProductId);
    dev_builder.AddConfiguration(config_builder);
    set_device_descriptor(dev_builder);
  }

  void Unplug() {
    unplugged_ = true;

    // All requests should have completed or been canceled before unplugging the USB device.
    ZX_ASSERT(bulk_out_requests_.is_empty());
    ZX_ASSERT(bulk_in_requests_.is_empty());
    ZX_ASSERT(interrupt_requests_.is_empty());
    ZX_ASSERT(isoc_in_requests_.is_empty());
  }

  void HangSco() { hanging_sco_ = true; }

  usb_protocol_t proto() const {
    usb_protocol_t proto;
    proto.ctx = const_cast<FakeUsbDevice*>(this);
    proto.ops = const_cast<usb_protocol_ops_t*>(&usb_protocol_ops_);
    return proto;
  }

  bool interrupt_enabled() const { return interrupt_enabled_; }

  bool bulk_in_enabled() const { return bulk_in_enabled_; }

  bool bulk_out_enabled() const { return bulk_out_enabled_; }

  bool isoc_in_enabled() const { return isoc_in_enabled_; }

  bool isoc_out_enabled() const { return isoc_out_enabled_; }

  const std::vector<std::vector<uint8_t>>& received_command_packets() const { return cmd_packets_; }

  const std::vector<std::vector<uint8_t>>& received_acl_packets() const { return acl_packets_; }

  const std::vector<std::vector<uint8_t>>& received_sco_packets() const { return sco_packets_; }

  // ddk::UsbProtocol methods:

  // Called by bt-transport-usb to send command packets.
  // UsbControlOut may be called by the read thread.
  zx_status_t UsbControlOut(uint8_t request_type, uint8_t request, uint16_t value, uint16_t index,
                            int64_t timeout, const uint8_t* write_buffer, size_t write_size) {
    ZX_ASSERT(write_buffer);
    std::vector<uint8_t> buffer_copy(write_buffer, write_buffer + write_size);
    cmd_packets_.push_back(std::move(buffer_copy));
    return ZX_OK;
  }

  zx_status_t UsbControlIn(uint8_t request_type, uint8_t request, uint16_t value, uint16_t index,
                           int64_t timeout, uint8_t* out_read_buffer, size_t read_size,
                           size_t* out_read_actual) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // UsbRequestQueue may be called from the read thread.
  void UsbRequestQueue(usb_request_t* usb_request,
                       const usb_request_complete_callback_t* complete_cb) {
    if (unplugged_) {
      async::PostTask(dispatcher_, [usb_request, complete_cb] {
        usb_request_complete(usb_request, ZX_ERR_IO_NOT_PRESENT, /*actual=*/0, complete_cb);
      });
      return;
    }

    UnownedRequest request(usb_request, *complete_cb, sizeof(usb_request_t));

    if (request.request()->header.ep_address == bulk_in_addr_) {
      ZX_ASSERT(bulk_in_enabled_);
      bulk_in_requests_.push(std::move(request));
      return;
    }

    // If the request is for an ACL packet write, copy the data and complete the request.
    if (request.request()->header.ep_address == bulk_out_addr_) {
      ZX_ASSERT(bulk_out_enabled_);
      std::vector<uint8_t> packet(request.request()->header.length);
      ssize_t actual_bytes_copied = request.CopyFrom(packet.data(), packet.size(), /*offset=*/0);
      EXPECT_EQ(actual_bytes_copied, static_cast<ssize_t>(packet.size()));
      acl_packets_.push_back(std::move(packet));
      async::PostTask(dispatcher_, [request = std::move(request), actual_bytes_copied]() mutable {
        request.Complete(ZX_OK, /*actual=*/actual_bytes_copied);
      });
      return;
    }

    if (isoc_in_addr_ && request.request()->header.ep_address == *isoc_in_addr_) {
      ZX_ASSERT(isoc_in_enabled_);
      ZX_ASSERT_MSG(isoc_interface_alt_ != 0,
                    "requests must not be sent to isoc interface with alt setting 0");
      isoc_in_requests_.push(std::move(request));
      return;
    }

    if (isoc_out_addr_ && request.request()->header.ep_address == *isoc_out_addr_) {
      ZX_ASSERT(isoc_out_enabled_);
      ZX_ASSERT_MSG(isoc_interface_alt_ != 0,
                    "requests must not be sent to isoc interface with alt setting 0");

      if (hanging_sco_) {
        isoc_out_requests_.push(std::move(request));
        return;
      }

      std::vector<uint8_t> packet(request.request()->header.length);
      ssize_t actual_bytes_copied = request.CopyFrom(packet.data(), packet.size(), /*offset=*/0);
      EXPECT_EQ(actual_bytes_copied, static_cast<ssize_t>(packet.size()));
      sco_packets_.push_back(std::move(packet));
      async::PostTask(dispatcher_, [request = std::move(request), actual_bytes_copied]() mutable {
        request.Complete(ZX_OK, /*actual=*/actual_bytes_copied);
      });
      return;
    }

    if (request.request()->header.ep_address == interrupt_addr_) {
      ZX_ASSERT(interrupt_enabled_);
      interrupt_requests_.push(std::move(request));
      return;
    }

    zxlogf(ERROR, "FakeUsbDevice: received request for unknown endpoint");
    async::PostTask(dispatcher_, [request = std::move(request)]() mutable {
      request.Complete(ZX_ERR_IO_NOT_PRESENT, /*actual=*/0);
    });
  }

  usb_speed_t UsbGetSpeed() { return USB_SPEED_FULL; }

  zx_status_t UsbSetInterface(uint8_t interface_number, uint8_t alt_setting) {
    if (interface_number == kEventAndAclInterfaceNum) {
      ZX_ASSERT(alt_setting == 0);
      event_and_acl_interface_set_count_++;
      return ZX_OK;
    }
    if (interface_number == kIsocInterfaceNum) {
      // Endpoints must be disabled before changing the interface.
      ZX_ASSERT(!isoc_in_enabled_);
      ZX_ASSERT(!isoc_out_enabled_);
      isoc_interface_alt_ = alt_setting;
      isoc_interface_set_count_++;
      return ZX_OK;
    }
    return ZX_ERR_NOT_SUPPORTED;
  }

  uint8_t UsbGetConfiguration() { return 0; }

  zx_status_t UsbSetConfiguration(uint8_t configuration) { return ZX_ERR_NOT_SUPPORTED; }

  zx_status_t UsbEnableEndpoint(const usb_endpoint_descriptor_t* ep_desc,
                                const usb_ss_ep_comp_descriptor_t* ss_com_desc, bool enable) {
    if (interrupt_addr_ && interrupt_addr_.value() == ep_desc->b_endpoint_address) {
      interrupt_enabled_ = enable;
      return ZX_OK;
    }

    if (bulk_in_addr_ && bulk_in_addr_.value() == ep_desc->b_endpoint_address) {
      bulk_in_enabled_ = enable;
      return ZX_OK;
    }

    if (bulk_out_addr_ && bulk_out_addr_.value() == ep_desc->b_endpoint_address) {
      bulk_out_enabled_ = enable;
      return ZX_OK;
    }

    if (isoc_in_addr_ && isoc_in_addr_.value() == ep_desc->b_endpoint_address) {
      isoc_in_enabled_ = enable;
      return ZX_OK;
    }

    if (isoc_out_addr_ && isoc_out_addr_.value() == ep_desc->b_endpoint_address) {
      isoc_out_enabled_ = enable;
      return ZX_OK;
    }

    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t UsbResetEndpoint(uint8_t ep_address) { return ZX_ERR_NOT_SUPPORTED; }

  zx_status_t UsbResetDevice() { return ZX_ERR_NOT_SUPPORTED; }

  size_t UsbGetMaxTransferSize(uint8_t ep_address) { return 0; }

  uint32_t UsbGetDeviceId() { return 0; }
  void UsbGetDeviceDescriptor(usb_device_descriptor_t* out_desc) {
    memcpy(out_desc, device_descriptor_data_.data(), sizeof(usb_device_descriptor_t));
  }

  zx_status_t UsbGetConfigurationDescriptorLength(uint8_t configuration, size_t* out_length) {
    *out_length = config_descriptor_data_.size();
    return ZX_OK;
  }

  zx_status_t UsbGetConfigurationDescriptor(uint8_t configuration, uint8_t* out_desc_buffer,
                                            size_t desc_size, size_t* out_desc_actual) {
    if (desc_size != config_descriptor_data_.size()) {
      return ZX_ERR_INVALID_ARGS;
    }
    memcpy(out_desc_buffer, config_descriptor_data_.data(), config_descriptor_data_.size());
    *out_desc_actual = config_descriptor_data_.size();
    return ZX_OK;
  }

  size_t UsbGetDescriptorsLength() { return device_descriptor_data_.size(); }

  void UsbGetDescriptors(uint8_t* out_descs_buffer, size_t descs_size, size_t* out_descs_actual) {
    ZX_ASSERT(descs_size == device_descriptor_data_.size());
    memcpy(out_descs_buffer, device_descriptor_data_.data(), descs_size);
  }

  zx_status_t UsbGetStringDescriptor(uint8_t desc_id, uint16_t lang_id, uint16_t* out_lang_id,
                                     uint8_t* out_string_buffer, size_t string_size,
                                     size_t* out_string_actual) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t UsbCancelAll(uint8_t ep_address) {
    if (bulk_in_addr_ && ep_address == *bulk_in_addr_) {
      auto requests = std::move(bulk_in_requests_);
      requests.CompleteAll(ZX_ERR_CANCELED, 0);
      return ZX_OK;
    }
    if (bulk_out_addr_ && ep_address == *bulk_out_addr_) {
      auto requests = std::move(bulk_out_requests_);
      requests.CompleteAll(ZX_ERR_CANCELED, 0);
      return ZX_OK;
    }
    if (interrupt_addr_ && ep_address == *interrupt_addr_) {
      auto requests = std::move(interrupt_requests_);
      requests.CompleteAll(ZX_ERR_CANCELED, 0);
      return ZX_OK;
    }

    if (isoc_in_addr_ && isoc_in_addr_.value() == ep_address) {
      isoc_in_canceled_count_++;
      UnownedRequestQueue isoc_in_reqs = std::move(isoc_in_requests_);
      isoc_in_reqs.CompleteAll(ZX_ERR_CANCELED, 0);
      return ZX_OK;
    }

    if (isoc_out_addr_ && isoc_out_addr_.value() == ep_address) {
      isoc_out_canceled_count_++;
      UnownedRequestQueue isoc_out_reqs = std::move(isoc_out_requests_);
      isoc_out_reqs.CompleteAll(ZX_ERR_CANCELED, 0);
      return ZX_OK;
    }

    return ZX_ERR_NOT_SUPPORTED;
  }

  uint64_t UsbGetCurrentFrame() { return 0; }

  size_t UsbGetRequestSize() { return UnownedRequest::RequestSize(sizeof(usb_request_t)); }

  void StallOneBulkInRequest() {
    std::optional<usb::BorrowedRequest<>> value = bulk_in_requests_.pop();

    if (!value) {
      return;
    }
    value->Complete(ZX_ERR_IO_INVALID, /*actual=*/0);
  }

  void StallOneIsocInRequest() {
    std::optional<usb::BorrowedRequest<>> value = isoc_in_requests_.pop();
    if (!value) {
      return;
    }
    value->Complete(ZX_ERR_IO_INVALID, /*actual=*/0);
  }

  // Sends 1 ACL data packet.
  // Returns true if a response was sent.
  bool SendOneBulkInResponse(std::vector<uint8_t> buffer) {
    std::optional<usb::BorrowedRequest<>> req = bulk_in_requests_.pop();

    if (!req) {
      return false;
    }

    // Copy data into the request's VMO. The request must have been allocated with a large enough
    // VMO (usb_request_alloc's data_size parameter). bt-transport-usb currently uses the max ACL
    // frame size for the data_size.
    ssize_t actual_copy_size = req->CopyTo(buffer.data(), buffer.size(), /*offset=*/0);
    EXPECT_EQ(actual_copy_size, static_cast<ssize_t>(buffer.size()));
    // This calls the request callback and sets response.status and response.actual.
    req->Complete(ZX_OK, /*actual=*/actual_copy_size);

    return true;
  }

  // Sends 1 SCO data packet.
  // Returns true if a response was sent.
  bool SendOneIsocInResponse(std::vector<uint8_t> buffer) {
    std::optional<usb::BorrowedRequest<>> req = isoc_in_requests_.pop();

    if (!req) {
      return false;
    }

    // Copy data into the request's VMO. The request must have been allocated with a large enough
    // VMO (usb_request_alloc's data_size parameter). bt-transport-usb currently uses the max SCO
    // frame size for the data_size.
    ssize_t actual_copy_size = req->CopyTo(buffer.data(), buffer.size(), /*offset=*/0);
    EXPECT_EQ(actual_copy_size, static_cast<ssize_t>(buffer.size()));
    // This calls the request callback and sets response.status and response.actual.
    req->Complete(ZX_OK, /*actual=*/actual_copy_size);
    return true;
  }

  // The first even chunk buffer should be at least 2 bytes, and must specify an accurate
  // parameter_total_size (second byte).
  // Returns true if a response was sent.
  bool SendHciEvent(const std::vector<uint8_t>& buffer) {
    std::optional<usb::BorrowedRequest<>> req = interrupt_requests_.pop();

    if (!req) {
      return false;
    }

    // Copy data into the request's VMO. The request must have been allocated with a large enough
    // VMO (usb_request_alloc's data_size parameter).
    ssize_t actual_copy_size = req->CopyTo(buffer.data(), buffer.size(), /*offset=*/0);
    EXPECT_EQ(actual_copy_size, static_cast<ssize_t>(buffer.size()));
    // This calls the request callback and sets response.status and response.actual.
    req->Complete(ZX_OK, /*actual=*/actual_copy_size);

    return true;
  }

  uint8_t isoc_interface_alt() { return isoc_interface_alt_; }

  int isoc_interface_set_count() { return isoc_interface_set_count_; }

 private:
  async_dispatcher_t* dispatcher_;

  bool unplugged_ = false;
  // Put the usb driver into a state that ignores requests.
  bool hanging_sco_ = false;

  // Command packets received from bt-transport-usb.
  std::vector<std::vector<uint8_t>> cmd_packets_;

  // Outbound ACL packets received from bt-transport-usb.
  std::vector<std::vector<uint8_t>> acl_packets_;

  // Outbound SCO packets received from bt-transport-usb.
  std::vector<std::vector<uint8_t>> sco_packets_;

  // ACL data in/out requests.
  UnownedRequestQueue bulk_in_requests_;
  UnownedRequestQueue bulk_out_requests_;

  // Requests for HCI events
  UnownedRequestQueue interrupt_requests_;

  // Inbound SCO requests.
  UnownedRequestQueue isoc_in_requests_;
  UnownedRequestQueue isoc_out_requests_;

  std::vector<uint8_t> device_descriptor_data_;
  std::vector<uint8_t> config_descriptor_data_;
  std::optional<uint8_t> bulk_in_addr_;
  bool bulk_in_enabled_ = false;
  std::optional<uint8_t> bulk_out_addr_;
  bool bulk_out_enabled_ = false;
  std::optional<uint8_t> isoc_out_addr_;
  bool isoc_out_enabled_ = false;
  int isoc_out_canceled_count_ = 0;
  std::optional<uint8_t> isoc_in_addr_;
  bool isoc_in_enabled_ = false;
  int isoc_in_canceled_count_ = 0;
  std::optional<uint8_t> interrupt_addr_;
  bool interrupt_enabled_ = false;
  uint8_t isoc_interface_alt_ = 0;
  int isoc_interface_set_count_ = 0;
  int event_and_acl_interface_set_count_ = 0;
};

class BtTransportUsbTest : public ::gtest::TestLoopFixture {
 public:
  void SetUp() override {
    fake_usb_device_.emplace(dispatcher());
    root_device_ = MockDevice::FakeRootParent();
    root_device_->AddProtocol(ZX_PROTOCOL_USB, fake_usb_device_->proto().ops,
                              fake_usb_device_->proto().ctx);

    fake_usb_device_->ConfigureDefaultDescriptors();

    ASSERT_EQ(bt_transport_usb::Device::Create(root_device_.get(), dispatcher()), ZX_OK);
    ASSERT_EQ(1u, root_device()->child_count());
    ASSERT_TRUE(dut());
    EXPECT_TRUE(fake_usb_device_->interrupt_enabled());
    EXPECT_TRUE(fake_usb_device_->bulk_in_enabled());
    EXPECT_TRUE(fake_usb_device_->bulk_out_enabled());
    EXPECT_FALSE(fake_usb_device_->isoc_in_enabled());
    EXPECT_FALSE(fake_usb_device_->isoc_out_enabled());
  }

  void TearDown() override {
    RunLoopUntilIdle();

    dut()->UnbindOp();
    RunLoopUntilIdle();
    EXPECT_EQ(dut()->UnbindReplyCallStatus(), ZX_OK);
    EXPECT_FALSE(fake_usb_device_->interrupt_enabled());
    EXPECT_FALSE(fake_usb_device_->bulk_in_enabled());
    EXPECT_FALSE(fake_usb_device_->bulk_out_enabled());
    EXPECT_FALSE(fake_usb_device_->isoc_in_enabled());
    EXPECT_FALSE(fake_usb_device_->isoc_out_enabled());

    dut()->ReleaseOp();

    fake_usb_device_->Unplug();
  }

  // The root device that bt-transport-usb binds to.
  MockDevice* root_device() const { return root_device_.get(); }

  // Returns the MockDevice corresponding to the bt-transport-usb driver.
  MockDevice* dut() const { return root_device_->GetLatestChild(); }

  FakeUsbDevice* fake_usb() { return &fake_usb_device_.value(); }

 private:
  std::shared_ptr<MockDevice> root_device_;
  std::optional<FakeUsbDevice> fake_usb_device_;
};

class BtTransportUsbHciTransportProtocolTest
    : public BtTransportUsbTest,
      public fidl::AsyncEventHandler<fhbt::Snoop>,
      public fidl::WireAsyncEventHandler<fhbt::HciTransport>,
      public fidl::WireAsyncEventHandler<fhbt::ScoConnection> {
 public:
  void SetUp() override {
    BtTransportUsbTest::SetUp();
    // Open HciTransport protocol.
    auto [hci_transport_client_end, hci_transport_server_end] =
        fidl::CreateEndpoints<fhbt::HciTransport>().value();

    hci_transport_client_.Bind(std::move(hci_transport_client_end), dispatcher(), this);
    dut()->GetDeviceContext<bt_transport_usb::Device>()->ConnectHciTransport(
        std::move(hci_transport_server_end));

    auto [snoop_client_end, snoop_server_end] = fidl::CreateEndpoints<fhbt::Snoop>().value();
    snoop_client_.Bind(std::move(snoop_client_end), dispatcher(), this);
    dut()->GetDeviceContext<bt_transport_usb::Device>()->ConnectSnoop(std::move(snoop_server_end));

    RunLoopUntilIdle();
  }

  void SetAckSnoop(bool ack_snoop) { ack_snoop_ = ack_snoop; }

  void AckSnoop() {
    // Acknowledge the snoop packet with |current_snoop_seq_|.
    fit::result<fidl::OneWayError> result = snoop_client_->AcknowledgePackets(current_snoop_seq_);
    ASSERT_FALSE(result.is_error());
  }

  // fhbt::Snoop request handlers
  void OnObservePacket(
      ::fidl::Event<::fuchsia_hardware_bluetooth::Snoop::OnObservePacket>& event) override {
    ASSERT_TRUE(event.sequence().has_value());
    ASSERT_TRUE(event.direction().has_value());
    ASSERT_TRUE(event.packet().has_value());

    current_snoop_seq_ = event.sequence().value();

    if (event.direction().value() == fhbt::PacketDirection::kHostToController) {
      switch (event.packet()->Which()) {
        case fhbt::SnoopPacket::Tag::kAcl:
          snoop_sent_acl_packets_.push_back(event.packet()->acl().value());
          break;
        case fhbt::SnoopPacket::Tag::kCommand:
          snoop_sent_command_packets_.push_back(event.packet()->command().value());
          break;
        case fhbt::SnoopPacket::Tag::kSco:
          snoop_sent_sco_packets_.push_back(event.packet()->sco().value());
          break;
        case fhbt::SnoopPacket::Tag::kIso:
          // No Iso data should be sent.
          FAIL();
        default:
          // Unknown packet type sent.
          FAIL();
      };
    } else if (event.direction().value() == fhbt::PacketDirection::kControllerToHost) {
      switch (event.packet()->Which()) {
        case fhbt::SnoopPacket::Tag::kEvent:
          snoop_received_event_packets_.push_back(event.packet()->event().value());
          break;
        case fhbt::SnoopPacket::Tag::kAcl:
          snoop_received_acl_packets_.push_back(event.packet()->acl().value());
          break;
        case fhbt::SnoopPacket::Tag::kSco:
          snoop_received_sco_packets_.push_back(event.packet()->sco().value());
          break;
        case fhbt::SnoopPacket::Tag::kIso:
          // No Iso data should be received.
          FAIL();
        default:
          // Unknown packet type received.
          FAIL();
      };
    }

    if (ack_snoop_) {
      AckSnoop();
    }
  }

  void OnDroppedPackets(
      ::fidl::Event<::fuchsia_hardware_bluetooth::Snoop::OnDroppedPackets>&) override {
    // Do nothing, the driver shouldn't drop any packet for now.
    FAIL();
  }

  void handle_unknown_event(::fidl::UnknownEventMetadata<fhbt::Snoop> metadata) override {
    // Shouldn't receive unknown event from fhbt::Snoop protocol.
    FAIL();
  }

  // fhbt::HciTransport event handler overrides
  void OnReceive(fhbt::wire::ReceivedPacket* packet) override {
    std::vector<uint8_t> received;
    switch (packet->Which()) {
      case fhbt::wire::ReceivedPacket::Tag::kEvent:
        received.assign(packet->event().get().begin(), packet->event().get().end());
        received_event_packets_.push_back(received);
        break;
      case fhbt::wire::ReceivedPacket::Tag::kAcl:
        received.assign(packet->acl().get().begin(), packet->acl().get().end());
        received_acl_packets_.push_back(received);
        break;
      case fhbt::wire::ReceivedPacket::Tag::kIso:
        // No Iso data should be received.
        FAIL();
      default:
        // Unknown packet type received.
        FAIL();
    }
  }

  void handle_unknown_event(fidl::UnknownEventMetadata<fhbt::HciTransport> metadata) override {
    FAIL();
  }

  void on_fidl_error(fidl::UnbindInfo error) override {}

  // fhbt::ScoConnection event handler overrides
  void OnReceive(fhbt::wire::ScoPacket* packet) override {
    std::vector<uint8_t> received;
    received.assign(packet->packet.begin(), packet->packet.end());
    received_sco_packets_.push_back(received);
  }

  void handle_unknown_event(fidl::UnknownEventMetadata<fhbt::ScoConnection> metadata) override {
    FAIL();
  }

  void ConfigureSco(fhbt::ScoCodingFormat coding_format, fhbt::ScoEncoding encoding,
                    fhbt::ScoSampleRate sample_rate) {
    fidl::Arena arena;
    auto [sco_client_end, sco_server_end] = fidl::CreateEndpoints<fhbt::ScoConnection>().value();

    auto sco_request_builder = fhbt::wire::HciTransportConfigureScoRequest::Builder(arena);
    auto configure_sco_result =
        hci_transport_client_->ConfigureSco(sco_request_builder.coding_format(coding_format)
                                                .encoding(encoding)
                                                .sample_rate(sample_rate)
                                                .connection(std::move(sco_server_end))
                                                .Build());
    ASSERT_EQ(configure_sco_result.status(), ZX_OK);

    sco_client_.emplace(std::move(sco_client_end), dispatcher(), this);
  }
  void StopScoConnection() {
    auto result = sco_client_.value()->Stop();
    ASSERT_TRUE(result.ok());
    auto client_end = sco_client_->UnbindMaybeGetEndpoint();
    RunLoopUntilIdle();
    sco_client_.reset();
  }

  const std::vector<std::vector<uint8_t>>& snoop_sent_acl_packets() const {
    return snoop_sent_acl_packets_;
  }

  const std::vector<std::vector<uint8_t>>& snoop_sent_command_packets() const {
    return snoop_sent_command_packets_;
  }

  const std::vector<std::vector<uint8_t>>& snoop_sent_sco_packets() const {
    return snoop_sent_sco_packets_;
  }

  const std::vector<std::vector<uint8_t>>& snoop_received_acl_packets() const {
    return snoop_received_acl_packets_;
  }

  const std::vector<std::vector<uint8_t>>& snoop_received_event_packets() const {
    return snoop_received_event_packets_;
  }

  const std::vector<std::vector<uint8_t>>& snoop_received_sco_packets() const {
    return snoop_received_sco_packets_;
  }

  const std::vector<std::vector<uint8_t>>& received_event_packets() const {
    return received_event_packets_;
  }

  const std::vector<std::vector<uint8_t>>& received_acl_packets() const {
    return received_acl_packets_;
  }

  const std::vector<std::vector<uint8_t>>& received_sco_packets() const {
    return received_sco_packets_;
  }

  fidl::Client<fhbt::Snoop>& snoop_client() { return snoop_client_; }

  fidl::WireClient<fhbt::HciTransport> hci_transport_client_;
  std::optional<fidl::WireClient<fhbt::ScoConnection>> sco_client_;

 private:
  std::shared_ptr<fdf_testing::DriverRuntime> runtime_ = mock_ddk::GetDriverRuntime();
  fdf::UnownedSynchronizedDispatcher hci_transport_server_dispatcher_ =
      runtime_->StartBackgroundDispatcher();

  std::optional<fidl::ServerBindingRef<fhbt::HciTransport>> hci_transport_server_;
  fidl::Client<fhbt::Snoop> snoop_client_;

  std::vector<std::vector<uint8_t>> received_event_packets_;
  std::vector<std::vector<uint8_t>> received_acl_packets_;
  std::vector<std::vector<uint8_t>> received_sco_packets_;

  std::vector<std::vector<uint8_t>> snoop_sent_acl_packets_;
  std::vector<std::vector<uint8_t>> snoop_sent_command_packets_;
  std::vector<std::vector<uint8_t>> snoop_sent_sco_packets_;

  std::vector<std::vector<uint8_t>> snoop_received_acl_packets_;
  std::vector<std::vector<uint8_t>> snoop_received_event_packets_;
  std::vector<std::vector<uint8_t>> snoop_received_sco_packets_;

  bool ack_snoop_ = true;
  uint64_t current_snoop_seq_ = 0;
};

class BtTransportUsbBindFailureTest : public ::gtest::TestLoopFixture {};

// This tests the test fixture setup and teardown.
TEST_F(BtTransportUsbTest, Lifecycle) {}

TEST_F(BtTransportUsbTest, IgnoresStalledRequest) {
  fake_usb()->StallOneBulkInRequest();
  EXPECT_FALSE(dut()->RemoveCalled());
}

TEST_F(BtTransportUsbTest, Name) { EXPECT_EQ(std::string(dut()->name()), "bt-transport-usb"); }

TEST_F(BtTransportUsbTest, Properties) {
  cpp20::span<const zx_device_str_prop_t> props = dut()->GetStringProperties();
  ASSERT_EQ(props.size(), 3u);
  EXPECT_EQ(props[0].key, std::string_view(bind_fuchsia::PROTOCOL));
  EXPECT_EQ(props[0].property_value.data.int_val, bind_fuchsia_bluetooth::BIND_PROTOCOL_TRANSPORT);
  EXPECT_EQ(props[1].key, std::string_view(bind_fuchsia::USB_VID));
  EXPECT_EQ(props[1].property_value.data.int_val, kVendorId);
  EXPECT_EQ(props[2].key, std::string_view(bind_fuchsia::USB_PID));
  EXPECT_EQ(props[2].property_value.data.int_val, kProductId);
}

TEST_F(BtTransportUsbBindFailureTest, NoConfigurationDescriptor) {
  FakeUsbDevice fake_usb_device(dispatcher());
  std::shared_ptr<MockDevice> root_device = MockDevice::FakeRootParent();
  root_device->AddProtocol(ZX_PROTOCOL_USB, fake_usb_device.proto().ops,
                           fake_usb_device.proto().ctx);
  EXPECT_EQ(bt_transport_usb::Device::Create(root_device.get(), dispatcher()),
            ZX_ERR_NOT_SUPPORTED);
}

TEST_F(BtTransportUsbBindFailureTest, ConfigurationDescriptorWithoutInterfaces) {
  FakeUsbDevice fake_usb_device(dispatcher());
  std::shared_ptr<MockDevice> root_device = MockDevice::FakeRootParent();
  root_device->AddProtocol(ZX_PROTOCOL_USB, fake_usb_device.proto().ops,
                           fake_usb_device.proto().ctx);

  usb::ConfigurationBuilder config_builder(/*config_num=*/0);
  usb::DeviceDescriptorBuilder dev_builder;
  dev_builder.AddConfiguration(config_builder);
  fake_usb_device.set_device_descriptor(dev_builder);

  EXPECT_EQ(bt_transport_usb::Device::Create(root_device.get(), dispatcher()),
            ZX_ERR_NOT_SUPPORTED);
}

TEST_F(BtTransportUsbBindFailureTest, ConfigurationDescriptorWithIncorrectNumberOfEndpoints) {
  FakeUsbDevice fake_usb_device(dispatcher());
  std::shared_ptr<MockDevice> root_device = MockDevice::FakeRootParent();
  root_device->AddProtocol(ZX_PROTOCOL_USB, fake_usb_device.proto().ops,
                           fake_usb_device.proto().ctx);

  usb::InterfaceBuilder interface_builder(/*config_num=*/0);
  usb::EndpointBuilder interrupt_endpoint_builder(
      /*config_num=*/0, static_cast<uint8_t>(fdescriptor::EndpointType::kInterrupt),
      /*endpoint_index=*/1, /*in=*/true);
  interface_builder.AddEndpoint(interrupt_endpoint_builder);
  usb::ConfigurationBuilder config_builder(/*config_num=*/0);
  config_builder.AddInterface(interface_builder);
  usb::DeviceDescriptorBuilder dev_builder;
  dev_builder.AddConfiguration(config_builder);
  fake_usb_device.set_device_descriptor(dev_builder);

  EXPECT_EQ(bt_transport_usb::Device::Create(root_device.get(), dispatcher()),
            ZX_ERR_NOT_SUPPORTED);
}

TEST_F(BtTransportUsbBindFailureTest,
       ConfigurationDescriptorWithIncorrectEndpointTypesInInterface0) {
  FakeUsbDevice fake_usb_device(dispatcher());
  std::shared_ptr<MockDevice> root_device = MockDevice::FakeRootParent();
  root_device->AddProtocol(ZX_PROTOCOL_USB, fake_usb_device.proto().ops,
                           fake_usb_device.proto().ctx);

  usb::InterfaceBuilder interface_0_builder(/*config_num=*/0);
  usb::EndpointBuilder interrupt_endpoint_builder(
      /*config_num=*/0, static_cast<uint8_t>(fdescriptor::EndpointType::kInterrupt),
      /*endpoint_index=*/0, /*in=*/true);
  interface_0_builder.AddEndpoint(interrupt_endpoint_builder);
  usb::EndpointBuilder bulk_in_endpoint_builder(
      /*config_num=*/0, static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
      /*endpoint_index=*/1, /*in=*/true);
  interface_0_builder.AddEndpoint(bulk_in_endpoint_builder);

  // Add isoc endpoint instead of expected bulk out endpoint.
  usb::EndpointBuilder bulk_out_endpoint_builder(
      /*config_num=*/0,
      static_cast<uint8_t>(fuchsia_hardware_usb_descriptor::EndpointType::kIsochronous),
      /*endpoint_index=*/0, /*in=*/false);
  interface_0_builder.AddEndpoint(bulk_out_endpoint_builder);

  usb::ConfigurationBuilder config_builder(/*config_num=*/0);
  config_builder.AddInterface(interface_0_builder);

  usb::DeviceDescriptorBuilder dev_builder;
  dev_builder.AddConfiguration(config_builder);
  fake_usb_device.set_device_descriptor(dev_builder);

  EXPECT_EQ(bt_transport_usb::Device::Create(root_device.get(), dispatcher()),
            ZX_ERR_NOT_SUPPORTED);
}

TEST_F(BtTransportUsbHciTransportProtocolTest, ReceiveManySmallHciEvents) {
  std::vector<uint8_t> kEventBuffer = {
      0x01,  // arbitrary event code
      0x01,  // parameter_total_size
      0x02   // arbitrary parameter
  };
  const int kNumEvents = 50;

  for (int i = 0; i < kNumEvents; i++) {
    ASSERT_TRUE(fake_usb()->SendHciEvent(kEventBuffer));
    RunLoopUntilIdle();
  }

  ASSERT_EQ(received_event_packets().size(), static_cast<size_t>(kNumEvents));
  for (const std::vector<uint8_t>& event : received_event_packets()) {
    EXPECT_EQ(event, kEventBuffer);
  }

  auto packets = snoop_received_event_packets();
  ASSERT_EQ(packets.size(), static_cast<size_t>(kNumEvents));
  for (const std::vector<uint8_t>& packet : packets) {
    EXPECT_EQ(packet, kEventBuffer);
  }
}

TEST_F(BtTransportUsbHciTransportProtocolTest, DropSnoopPackets) {
  SetAckSnoop(false);
  std::vector<uint8_t> kEventBuffer = {
      0x01,  // arbitrary event code
      0x01,  // parameter_total_size
      0x02   // arbitrary parameter
  };

  // If the test doesn't ack the snoop sequence until the total sent snoop packet number reaches
  // this limit, the driver will start dropping packets. i.e. The driver will drop the 22nd snoop
  // packet.
  const size_t kSnoopPacketLimit = 21;

  for (size_t i = 0; i < kSnoopPacketLimit + 1; i++) {
    ASSERT_TRUE(fake_usb()->SendHciEvent(kEventBuffer));
    RunLoopUntilIdle();
  }

  auto packets = snoop_received_event_packets();
  // Test will only receive |kSnoopPacketLimit| of packet after |kSnoopPacketLimit + 1| packets have
  // been sent.
  ASSERT_EQ(packets.size(), kSnoopPacketLimit);

  // The acked sequence number will catch up in the driver and it resumes sending snoop packets
  // again.
  AckSnoop();
  RunLoopUntilIdle();

  ASSERT_TRUE(fake_usb()->SendHciEvent(kEventBuffer));
  RunLoopUntilIdle();
  packets = snoop_received_event_packets();
  ASSERT_EQ(packets.size(), kSnoopPacketLimit + 1);
}

TEST_F(BtTransportUsbHciTransportProtocolTest, ReceiveManyHciEventsSplitIntoTwoResponses) {
  const std::vector<uint8_t> kEventBuffer = {
      0x01,  // event code
      0x02,  // parameter_total_size
      0x03,  // arbitrary parameter
      0x04   // arbitrary parameter
  };
  const std::vector<uint8_t> kPart1(kEventBuffer.begin(), kEventBuffer.begin() + 3);
  const std::vector<uint8_t> kPart2(kEventBuffer.begin() + 3, kEventBuffer.end());

  const int kNumEvents = 50;
  for (int i = 0; i < kNumEvents; i++) {
    EXPECT_TRUE(fake_usb()->SendHciEvent(kPart1));
    EXPECT_TRUE(fake_usb()->SendHciEvent(kPart2));
    RunLoopUntilIdle();
  }

  ASSERT_EQ(received_event_packets().size(), static_cast<size_t>(kNumEvents));
  for (const std::vector<uint8_t>& event : received_event_packets()) {
    EXPECT_EQ(event, kEventBuffer);
  }

  auto packets = snoop_received_event_packets();
  ASSERT_EQ(packets.size(), static_cast<size_t>(kNumEvents));
  for (const std::vector<uint8_t>& packet : packets) {
    EXPECT_EQ(packet, kEventBuffer);
  }
}

TEST_F(BtTransportUsbHciTransportProtocolTest, SendHciCommands) {
  fidl::Arena arena;
  std::vector<uint8_t> kCmd0 = {
      0x00,  // arbitrary payload
  };

  {
    auto packet_view = fidl::VectorView<uint8_t>::FromExternal(kCmd0);
    hci_transport_client_->Send(fhbt::wire::SentPacket::WithCommand(arena, packet_view))
        .Then([](fidl::WireUnownedResult<fhbt::HciTransport::Send>& result) {
          ASSERT_TRUE(result.ok());
        });
  }

  std::vector<uint8_t> kCmd1 = {
      0x01,  // arbitrary payload (longer than before)
      0xC0,
      0xDE,
  };

  {
    auto packet_view = fidl::VectorView<uint8_t>::FromExternal(kCmd1);
    hci_transport_client_->Send(fhbt::wire::SentPacket::WithCommand(arena, packet_view))
        .Then([](fidl::WireUnownedResult<fhbt::HciTransport::Send>& result) {
          ASSERT_TRUE(result.ok());
        });
  }

  RunLoopUntilIdle();
  std::vector<std::vector<uint8_t>> cmd_packets = fake_usb()->received_command_packets();
  ASSERT_EQ(cmd_packets.size(), 2u);
  EXPECT_EQ(cmd_packets[0], kCmd0);
  EXPECT_EQ(cmd_packets[1], kCmd1);

  std::vector<std::vector<uint8_t>> snoop_packets = snoop_sent_command_packets();
  ASSERT_EQ(snoop_packets.size(), 2u);
  EXPECT_EQ(snoop_packets[0], kCmd0);
  EXPECT_EQ(snoop_packets[1], kCmd1);
}

TEST_F(BtTransportUsbHciTransportProtocolTest, ReceiveManyAclPackets) {
  const std::vector<uint8_t> kAclBuffer = {
      0x04, 0x05  // arbitrary payload
  };

  const int kNumPackets = 50;
  for (int i = 0; i < kNumPackets; i++) {
    EXPECT_TRUE(fake_usb()->SendOneBulkInResponse(kAclBuffer));
    RunLoopUntilIdle();
  }

  ASSERT_EQ(received_acl_packets().size(), static_cast<size_t>(kNumPackets));
  for (const std::vector<uint8_t>& packet : received_acl_packets()) {
    EXPECT_EQ(packet.size(), kAclBuffer.size());
    EXPECT_EQ(packet, kAclBuffer);
  }

  RunLoopUntilIdle();
  auto packets = snoop_received_acl_packets();
  ASSERT_EQ(packets.size(), static_cast<size_t>(kNumPackets));
  for (const std::vector<uint8_t>& packet : packets) {
    EXPECT_EQ(packet, kAclBuffer);
  }
}

TEST_F(BtTransportUsbHciTransportProtocolTest, SendManyAclPackets) {
  const uint8_t kNumPackets = 8;
  fidl::Arena arena;
  for (uint8_t i = 0; i < kNumPackets; i++) {
    std::vector<uint8_t> packet;
    // Vary the length of the packets (start small)
    if (i % 2) {
      packet = std::vector<uint8_t>(1, i);
    } else {
      packet = std::vector<uint8_t>(10, i);
    }
    auto packet_view = fidl::VectorView<uint8_t>::FromExternal(packet);
    hci_transport_client_->Send(fhbt::wire::SentPacket::WithAcl(arena, packet_view))
        .Then([](fidl::WireUnownedResult<fhbt::HciTransport::Send>& result) {
          ASSERT_TRUE(result.ok());
        });
  }
  RunLoopUntilIdle();

  std::vector<std::vector<uint8_t>> packets = fake_usb()->received_acl_packets();
  ASSERT_EQ(packets.size(), kNumPackets);
  for (uint8_t i = 0; i < kNumPackets; i++) {
    if (i % 2) {
      EXPECT_EQ(packets[i], std::vector<uint8_t>(1, i));
    } else {
      EXPECT_EQ(packets[i], std::vector<uint8_t>(10, i));
    }
  }

  std::vector<std::vector<uint8_t>> snoop_packets = snoop_sent_acl_packets();
  ASSERT_EQ(snoop_packets.size(), kNumPackets);
  for (uint8_t i = 0; i < kNumPackets; i++) {
    std::vector<uint8_t> expectedSnoopPacket;
    if (i % 2) {
      expectedSnoopPacket = std::vector<uint8_t>(1, i);
    } else {
      expectedSnoopPacket = std::vector<uint8_t>(10, i);
    }
    EXPECT_EQ(snoop_packets[i], expectedSnoopPacket);
  }
}

TEST_F(BtTransportUsbHciTransportProtocolTest, SendManyScoPackets) {
  ConfigureSco(fhbt::ScoCodingFormat::kCvsd, fhbt::ScoEncoding::kBits8, fhbt::ScoSampleRate::kKhz8);
  RunLoopUntilIdle();
  EXPECT_EQ(fake_usb()->isoc_interface_alt(), 1);
  const uint8_t kNumPackets = 8;
  for (uint8_t i = 0; i < kNumPackets; i++) {
    std::vector<uint8_t> kScoPacket = {i};

    auto packet_view = fidl::VectorView<uint8_t>::FromExternal(kScoPacket);
    sco_client_.value()
        ->Send(packet_view)
        .Then([](fidl::WireUnownedResult<fhbt::ScoConnection::Send>& result) {
          ASSERT_TRUE(result.ok());
        });
  }

  // Completes USB requests.
  RunLoopUntilIdle();

  std::vector<std::vector<uint8_t>> sco_packets = fake_usb()->received_sco_packets();
  ASSERT_EQ(sco_packets.size(), kNumPackets);
  for (uint8_t i = 0; i < kNumPackets; i++) {
    EXPECT_EQ(sco_packets[i], std::vector<uint8_t>{i});
  }

  std::vector<std::vector<uint8_t>> snoop_packets = snoop_sent_sco_packets();
  ASSERT_EQ(snoop_packets.size(), kNumPackets);
  for (uint8_t i = 0; i < kNumPackets; i++) {
    std::vector<uint8_t> expectedSnoopPacket = {i};
    EXPECT_EQ(snoop_packets[i], expectedSnoopPacket);
  }
}

TEST_F(BtTransportUsbHciTransportProtocolTest, TooManyScoPacketsQueuedInDriver) {
  ConfigureSco(fhbt::ScoCodingFormat::kCvsd, fhbt::ScoEncoding::kBits8, fhbt::ScoSampleRate::kKhz8);
  RunLoopUntilIdle();
  EXPECT_EQ(fake_usb()->isoc_interface_alt(), 1);

  fake_usb()->HangSco();

  const uint8_t kNumPackets = 9;
  for (uint8_t i = 0; i < kNumPackets; i++) {
    std::vector<uint8_t> kScoPacket = {i};

    auto packet_view = fidl::VectorView<uint8_t>::FromExternal(kScoPacket);
    sco_client_.value()
        ->Send(packet_view)
        .Then([](fidl::WireUnownedResult<fhbt::ScoConnection::Send>& result) {
          // The completers of all the packets will never be called since the driver won't
          // process the data in queue, and the completers will all be closed when the server end of
          // SCO connection is closed.
          ASSERT_FALSE(result.ok());
        });
  }

  // Complete any requests.
  RunLoopUntilIdle();
}

TEST_F(BtTransportUsbHciTransportProtocolTest, ReceiveManyScoPackets) {
  ConfigureSco(fhbt::ScoCodingFormat::kCvsd, fhbt::ScoEncoding::kBits8, fhbt::ScoSampleRate::kKhz8);
  RunLoopUntilIdle();

  const std::vector<uint8_t> kScoBuffer = {
      0x01,  // arbitrary header fields
      0x02,
      0x03,  // payload length
      0x04,  // arbitrary payload
      0x05, 0x06,
  };
  // Split the packet into 2 chunks to test recombination.
  const std::vector<uint8_t> kScoBufferChunk0(kScoBuffer.begin(), kScoBuffer.begin() + 4);
  const std::vector<uint8_t> kScoBufferChunk1(kScoBuffer.begin() + 4, kScoBuffer.end());

  const int kNumPackets = 25;
  for (int i = 0; i < kNumPackets; i++) {
    EXPECT_TRUE(fake_usb()->SendOneIsocInResponse(kScoBufferChunk0));
    EXPECT_TRUE(fake_usb()->SendOneIsocInResponse(kScoBufferChunk1));
    RunLoopUntilIdle();
  }

  ASSERT_EQ(received_sco_packets().size(), static_cast<size_t>(kNumPackets));
  for (const std::vector<uint8_t>& packet : received_sco_packets()) {
    EXPECT_EQ(packet.size(), kScoBuffer.size());
    EXPECT_EQ(packet, kScoBuffer);
  }

  auto packets = snoop_received_sco_packets();
  ASSERT_EQ(packets.size(), static_cast<size_t>(kNumPackets));
  for (const std::vector<uint8_t>& packet : packets) {
    EXPECT_EQ(packet, kScoBuffer);
  }
}

TEST_F(BtTransportUsbHciTransportProtocolTest, TwoScoConnections) {
  ConfigureSco(fhbt::ScoCodingFormat::kCvsd, fhbt::ScoEncoding::kBits8, fhbt::ScoSampleRate::kKhz8);
  RunLoopUntilIdle();

  {
    std::vector<uint8_t> kScoPacket = {1};

    auto packet_view = fidl::VectorView<uint8_t>::FromExternal(kScoPacket);
    sco_client_.value()
        ->Send(packet_view)
        .Then([](fidl::WireUnownedResult<fhbt::ScoConnection::Send>& result) {
          ASSERT_TRUE(result.ok());
        });

    RunLoopUntilIdle();

    std::vector<std::vector<uint8_t>> sco_packets = fake_usb()->received_sco_packets();
    ASSERT_EQ(sco_packets.size(), 1U);
    EXPECT_EQ(sco_packets[0], std::vector<uint8_t>{1});
  }

  StopScoConnection();
  RunLoopUntilIdle();

  // Establish a new ScoConnection with the driver.
  ConfigureSco(fhbt::ScoCodingFormat::kCvsd, fhbt::ScoEncoding::kBits8, fhbt::ScoSampleRate::kKhz8);

  // Send Another SCO packet and make sure it's received.
  {
    std::vector<uint8_t> kScoPacket = {2};

    auto packet_view = fidl::VectorView<uint8_t>::FromExternal(kScoPacket);
    sco_client_.value()
        ->Send(packet_view)
        .Then([](fidl::WireUnownedResult<fhbt::ScoConnection::Send>& result) {
          ASSERT_TRUE(result.ok());
        });

    RunLoopUntilIdle();

    std::vector<std::vector<uint8_t>> sco_packets = fake_usb()->received_sco_packets();
    ASSERT_EQ(sco_packets.size(), 2U);
    EXPECT_EQ(sco_packets[1], std::vector<uint8_t>{2});
  }
}

}  // namespace
