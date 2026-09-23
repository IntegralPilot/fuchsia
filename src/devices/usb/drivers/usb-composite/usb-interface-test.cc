// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-composite/usb-interface.h"

#include <lib/async-loop/cpp/loop.h>

#include <queue>
#include <vector>

#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/devices/usb/drivers/usb-composite/test-helper.h"

namespace usb_composite {

void UsbComposite::DdkInit(ddk::InitTxn txn) { txn.Reply(ZX_OK); }
void UsbComposite::DdkUnbind(ddk::UnbindTxn txn) { txn.Reply(); }
void UsbComposite::DdkChildPreRelease(void* child_ctx) {}
void UsbComposite::DdkRelease() { delete this; }

std::queue<std::tuple<uint8_t, uint8_t*, size_t>> expect_get_additional_descriptor_list_;
void ExpectGetAdditionalDescriptorList(uint8_t last_interface_id, uint8_t* desc_list,
                                       size_t desc_size) {
  expect_get_additional_descriptor_list_.emplace(last_interface_id, desc_list, desc_size);
}
zx_status_t UsbComposite::GetAdditionalDescriptorList(uint8_t last_interface_id,
                                                      uint8_t* out_desc_list, size_t desc_count,
                                                      size_t* out_desc_actual) {
  EXPECT_GT(expect_get_additional_descriptor_list_.size(), 0);
  auto expected = expect_get_additional_descriptor_list_.front();
  EXPECT_EQ(std::get<0>(expected), last_interface_id);
  EXPECT_GE(desc_count, std::get<2>(expected));
  memcpy(out_desc_list, std::get<1>(expected), std::get<2>(expected));
  *out_desc_actual = std::get<2>(expected);
  expect_get_additional_descriptor_list_.pop();
  return ZX_OK;
}

std::queue<uint8_t> expect_claim_interface_;
void ExpectClaimInterface(uint8_t interface_id) { expect_claim_interface_.push(interface_id); }
zx_status_t UsbComposite::ClaimInterface(uint8_t interface_id) {
  EXPECT_GT(expect_claim_interface_.size(), 0);
  EXPECT_EQ(expect_claim_interface_.front(), interface_id);
  expect_claim_interface_.pop();
  return ZX_OK;
}

std::queue<std::tuple<uint8_t, uint8_t>> expect_set_interface_;
void ExpectSetInterface(uint8_t interface_id, uint8_t alt_setting) {
  expect_set_interface_.emplace(interface_id, alt_setting);
}
zx_status_t UsbComposite::SetInterface(uint8_t interface_id, uint8_t alt_setting) {
  EXPECT_GT(expect_set_interface_.size(), 0);
  auto expected = expect_set_interface_.front();
  EXPECT_EQ(std::get<0>(expected), interface_id);
  EXPECT_EQ(std::get<1>(expected), alt_setting);
  expect_set_interface_.pop();
  return ZX_OK;
}

std::atomic_uint32_t interface_count_ = 0;
void UsbComposite::RemoveInterface(UsbInterface* interface) {
  EXPECT_GT(interface_count_.load(), 0);
  interface_count_--;
}

template <auto* descriptors>
class UsbInterfaceTest : public zxtest::Test {
 public:
  void SetUp() override {
    fake_parent_->AddProtocol(ZX_PROTOCOL_USB, usb_.GetProto()->ops, usb_.GetProto()->ctx);
    auto composite = std::make_unique<UsbComposite>(fake_parent_.get());
    EXPECT_OK(composite->DdkAdd("composite", DEVICE_ADD_NON_BINDABLE));
    composite.release();
    composite_dev_ = fake_parent_->GetLatestChild();
    composite_ = composite_dev_->GetDeviceContext<UsbComposite>();
  }

  void TearDown() override {
    EXPECT_TRUE(expect_get_additional_descriptor_list_.empty());
    EXPECT_TRUE(expect_claim_interface_.empty());
    EXPECT_TRUE(expect_set_interface_.empty());

    device_async_remove(composite_dev_);
    mock_ddk::ReleaseFlaggedDevices(fake_parent_.get());

    EXPECT_EQ(interface_count_.load(), 0);
  }

  void InitTest();

  void SetDeviceDesc(const usb_device_descriptor_t& dev_desc) {
    composite_->device_desc_ = dev_desc;
  }

  void SetConfigDesc(uint8_t* config_desc, size_t config_length) {
    composite_->config_desc_.reset(config_desc, config_length);
  }

  uint8_t last_interface_id() { return dut_->last_interface_id_; }

 protected:
  MockUsb usb_;

  usb_composite::UsbInterface* dut_;
  ddk::UsbProtocolClient usb_client_;
  UsbComposite* composite_;
  ddk::UsbCompositeProtocolClient composite_client_;

 private:
  template <typename T>
  void SetUpInterface(const T* descriptor, size_t desc_length) {
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb::Usb>();
    ASSERT_OK(endpoints);
    std::unique_ptr<UsbInterface> ifc;
    EXPECT_OK(UsbInterface::Create(
        composite_dev_, composite_, ddk::UsbProtocolClient(fake_parent_.get()),
        std::move(endpoints->client), descriptor, desc_length, loop_.dispatcher(), &ifc));
    ASSERT_NOT_NULL(ifc);

    EXPECT_OK(ifc->DdkAdd("test-interface", DEVICE_ADD_NON_BINDABLE));
    ifc.release();
    interface_count_.fetch_add(1);
    auto* child = composite_dev_->GetLatestChild();
    dut_ = child->GetDeviceContext<UsbInterface>();

    usb_client_ = ddk::UsbProtocolClient(child);
    ASSERT_TRUE(usb_client_.is_valid());

    usb_composite_protocol_t proto;
    dut_->DdkGetProtocol(ZX_PROTOCOL_USB_COMPOSITE, &proto);
    composite_client_ = ddk::UsbCompositeProtocolClient(&proto);
    ASSERT_TRUE(composite_client_.is_valid());
  }

  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  std::shared_ptr<MockDevice> fake_parent_ = MockDevice::FakeRootParent();
  MockDevice* composite_dev_;
};

// The interface configuration corresponding to a Interface with two endpoints and one alternate
// interface.
constexpr struct intf_config {
  usb_interface_descriptor_t interface;
  usb_endpoint_descriptor_t ep1;
  usb_endpoint_descriptor_t ep2;
  usb_interface_descriptor_t alt_interface1;
  usb_endpoint_descriptor_t ep1_alt1;
} kTestInterface = {
    .interface =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 0,
            .b_alternate_setting = 0,
            .b_num_endpoints = 2,
            .b_interface_class = 8,
            .b_interface_sub_class = 6,
            .b_interface_protocol = 80,
            .i_interface = 0,
        },
    .ep1 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 0x81,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .ep2 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 2,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .alt_interface1 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 0,
            .b_alternate_setting = 1,
            .b_num_endpoints = 1,
            .b_interface_class = 8,
            .b_interface_sub_class = 6,
            .b_interface_protocol = 80,
            .i_interface = 0,
        },
    .ep1_alt1 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 0x7,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
};

// NoAssociationInterfaceTest tests an UsbInterface's ability to process interface descriptors
// with no association descriptors.
using NoAssociationInterfaceTest = UsbInterfaceTest<&kTestInterface>;

template <>
void NoAssociationInterfaceTest::InitTest() {
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep2, {}, true);
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep1, {}, true);
  SetUpInterface(&kTestInterface.interface, sizeof(kTestInterface));
  EXPECT_EQ(dut_->usb_class(), kTestInterface.interface.b_interface_class);
  EXPECT_EQ(dut_->usb_subclass(), kTestInterface.interface.b_interface_sub_class);
  EXPECT_EQ(dut_->usb_protocol(), kTestInterface.interface.b_interface_protocol);
}

TEST_F(NoAssociationInterfaceTest, SetUpTest) { InitTest(); }

TEST_F(NoAssociationInterfaceTest, SetAltSettingTest) {
  InitTest();

  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep2, {}, false);
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep1_alt1, {}, true);
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep1, {}, false);
  usb_.ExpectControlOut(ZX_OK, USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_INTERFACE,
                        USB_REQ_SET_INTERFACE, 1, 0, ZX_TIME_INFINITE, std::vector<uint8_t>{});
  EXPECT_OK(dut_->SetAltSetting(0, 1));
}

TEST_F(NoAssociationInterfaceTest, UsbProtocolTest) {
  // This set tests some of the simple pass through functions
  InitTest();

  usb_.ExpectControlOut(ZX_OK, 1, 2, 3, 4, ZX_TIME_INFINITE, std::vector<uint8_t>{5, 6});
  uint8_t write_buffer[] = {5, 6};
  EXPECT_OK(
      usb_client_.ControlOut(1, 2, 3, 4, ZX_TIME_INFINITE, write_buffer, sizeof(write_buffer)));

  std::vector<uint8_t> expected_read_buffer = {6, 5, 4};
  usb_.ExpectControlIn(ZX_OK, 10, 9, 8, 7, ZX_TIME_INFINITE, expected_read_buffer);
  size_t actual;
  std::vector<uint8_t> read_buffer(expected_read_buffer.size());
  EXPECT_OK(usb_client_.ControlIn(10, 9, 8, 7, ZX_TIME_INFINITE, read_buffer.data(),
                                  read_buffer.size(), &actual));
  EXPECT_EQ(actual, read_buffer.size());
  EXPECT_BYTES_EQ(read_buffer.data(), expected_read_buffer.data(), actual);

  // Made up values for testing
  usb_request_t expected_req = {
      .size = 1243,
      .offset = 235,
  };
  usb_request_complete_callback_t expected_callback = {
      .ctx = &expected_req,
  };
  usb_.ExpectRequestQueue(expected_req, expected_callback);
  usb_client_.RequestQueue(&expected_req, &expected_callback);

  usb_.ExpectGetSpeed(USB_SPEED_SUPER);
  EXPECT_EQ(usb_client_.GetSpeed(), USB_SPEED_SUPER);

  usb_.ExpectGetConfiguration(8);
  EXPECT_EQ(usb_client_.GetConfiguration(), 8);

  usb_.ExpectSetConfiguration(ZX_OK, 6);
  EXPECT_OK(usb_client_.SetConfiguration(6));

  EXPECT_EQ(usb_client_.EnableEndpoint(nullptr, nullptr, false), ZX_ERR_NOT_SUPPORTED);

  usb_.ExpectResetEndpoint(ZX_OK, 19);
  EXPECT_OK(usb_client_.ResetEndpoint(19));

  usb_.ExpectResetDevice(ZX_OK);
  EXPECT_OK(usb_client_.ResetDevice());

  usb_.ExpectGetMaxTransferSize(914, 14);
  EXPECT_EQ(usb_client_.GetMaxTransferSize(14), 914);

  usb_.ExpectGetDeviceId(9);
  EXPECT_EQ(usb_client_.GetDeviceId(), 9);

  usb_device_descriptor_t expected_device_desc = {
      .b_length = 3,
      .b_descriptor_type = 3,
      .bcd_usb = 3,
      .b_device_class = 3,
      .b_device_sub_class = 3,
      .b_device_protocol = 3,
      .b_max_packet_size0 = 3,
      .id_vendor = 3,
      .id_product = 3,
      .bcd_device = 3,
      .i_manufacturer = 3,
      .i_product = 3,
      .i_serial_number = 3,
      .b_num_configurations = 3,
  };
  usb_.ExpectGetDeviceDescriptor(expected_device_desc);
  usb_device_descriptor_t device_desc;
  usb_client_.GetDeviceDescriptor(&device_desc);
  EXPECT_BYTES_EQ(&expected_device_desc, &device_desc, sizeof(usb_device_descriptor_t));

  usb_.ExpectGetConfigurationDescriptorLength(ZX_OK, 2, 1024);
  uint64_t length;
  EXPECT_OK(usb_client_.GetConfigurationDescriptorLength(2, &length));
  EXPECT_EQ(length, 1024);

  std::vector<uint8_t> expected_config{0xA, 0xB, 0xC, 0xD, 0xE, 0xF};
  usb_.ExpectGetConfigurationDescriptor(ZX_OK, 4, expected_config);
  std::vector<uint8_t> config(expected_config.size());
  EXPECT_OK(usb_client_.GetConfigurationDescriptor(4, config.data(), config.size(), &actual));
  EXPECT_EQ(actual, expected_config.size());
  EXPECT_BYTES_EQ(expected_config.data(), config.data(), expected_config.size());

  usb_.ExpectCancelAll(ZX_OK, 5);
  EXPECT_OK(usb_client_.CancelAll(5));

  usb_.ExpectGetCurrentFrame(52);
  EXPECT_EQ(usb_client_.GetCurrentFrame(), 52);

  usb_.ExpectGetRequestSize(983);
  EXPECT_EQ(usb_client_.GetRequestSize(), 983);

  ExpectSetInterface(4, 2);
  EXPECT_OK(usb_client_.SetInterface(4, 2));
}

TEST_F(NoAssociationInterfaceTest, GetDescriptorsTest) {
  InitTest();

  EXPECT_EQ(usb_client_.GetDescriptorsLength(), sizeof(kTestInterface));

  uint8_t buffer[sizeof(kTestInterface)];
  size_t actual;
  usb_client_.GetDescriptors(buffer, sizeof(kTestInterface), &actual);
  EXPECT_EQ(actual, sizeof(kTestInterface));
  EXPECT_BYTES_EQ(buffer, &kTestInterface, sizeof(kTestInterface));
}

TEST_F(NoAssociationInterfaceTest, GetAdditionalDescriptorsTest) {
  InitTest();

  usb_configuration_descriptor_t expected_desc_list = {
      .b_length = sizeof(usb_configuration_descriptor_t),
      .b_descriptor_type = USB_DT_CONFIG,
      .w_total_length = 49,
      .b_num_interfaces = 0,
      .b_configuration_value = 1,
      .i_configuration = 4,
      .bm_attributes = 2,
      .b_max_power = 8,
  };
  ExpectGetAdditionalDescriptorList(0, reinterpret_cast<uint8_t*>(&expected_desc_list),
                                    sizeof(expected_desc_list));
  uint8_t desc_list[sizeof(expected_desc_list)];
  size_t actual;
  EXPECT_OK(composite_client_.GetAdditionalDescriptorList(desc_list, sizeof(desc_list), &actual));
  EXPECT_EQ(actual, sizeof(desc_list));
  EXPECT_BYTES_EQ(desc_list, &expected_desc_list, sizeof(desc_list));
}

TEST_F(NoAssociationInterfaceTest, GetAdditionalDescriptorsLengthTest) {
  InitTest();

  fbl::AllocChecker ac;
  uint16_t size = sizeof(usb_configuration_descriptor_t) + sizeof(kTestInterface) +
                  sizeof(usb_interface_info_descriptor_t);
  auto additional_intf_desc = new (&ac) uint8_t[size];
  EXPECT_TRUE(ac.check());
  auto* config = reinterpret_cast<usb_configuration_descriptor_t*>(additional_intf_desc);
  config->b_length = sizeof(usb_configuration_descriptor_t);
  config->b_descriptor_type = USB_DT_CONFIG;
  config->w_total_length = size;
  config->b_num_interfaces = 2;
  config->b_configuration_value = 0;
  config->i_configuration = 0;
  config->bm_attributes = 2;
  config->b_max_power = 4;
  memcpy(additional_intf_desc + sizeof(usb_configuration_descriptor_t), &kTestInterface,
         sizeof(kTestInterface));
  auto* additional_intf = reinterpret_cast<usb_interface_info_descriptor_t*>(
      additional_intf_desc + sizeof(usb_configuration_descriptor_t) + sizeof(kTestInterface));
  additional_intf->b_length = sizeof(usb_interface_info_descriptor_t);
  additional_intf->b_descriptor_type = USB_DT_INTERFACE;
  additional_intf->b_interface_number = 1;
  additional_intf->b_alternate_setting = 0;
  additional_intf->b_num_endpoints = 0;
  additional_intf->b_interface_class = 3;
  additional_intf->b_interface_sub_class = 2;
  additional_intf->b_interface_protocol = 1;
  additional_intf->i_interface = 1;
  SetConfigDesc(additional_intf_desc, size);

  EXPECT_EQ(composite_client_.GetAdditionalDescriptorLength(),
            sizeof(usb_interface_info_descriptor_t));
}

TEST_F(NoAssociationInterfaceTest, ClaimInterfaceTest) {
  InitTest();

  ExpectClaimInterface(1);
  usb_interface_descriptor_t interface_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = 1,
      .b_alternate_setting = 0,
      .b_num_endpoints = 3,
      .b_interface_class = 1,
      .b_interface_sub_class = 2,
      .b_interface_protocol = 3,
      .i_interface = 4,
  };
  EXPECT_OK(composite_client_.ClaimInterface(&interface_desc, sizeof(interface_desc)));

  EXPECT_EQ(last_interface_id(), 1);
  EXPECT_EQ(usb_client_.GetDescriptorsLength(), sizeof(kTestInterface) + sizeof(interface_desc));
  uint8_t desc[sizeof(kTestInterface) + sizeof(interface_desc)];
  size_t actual;
  usb_client_.GetDescriptors(desc, sizeof(desc), &actual);
  EXPECT_EQ(actual, sizeof(desc));
  EXPECT_BYTES_EQ(desc, &kTestInterface, sizeof(kTestInterface));
  EXPECT_BYTES_EQ(desc + sizeof(kTestInterface), &interface_desc, sizeof(interface_desc));
}

TEST_F(NoAssociationInterfaceTest, ContainsInterfaceTest) {
  InitTest();

  EXPECT_TRUE(dut_->ContainsInterface(0));
  EXPECT_FALSE(dut_->ContainsInterface(9));
}

// Alternative Interface creation using Device Descriptor class/subclass/protocol.
constexpr usb_interface_descriptor_t kDevDescInterface = {
    .b_length = sizeof(usb_interface_descriptor_t),
    .b_descriptor_type = USB_DT_INTERFACE,
    .b_interface_number = 0,
    .b_alternate_setting = 0,
    .b_num_endpoints = 0,
    .b_interface_class = 0,
    .b_interface_sub_class = 0,
    .b_interface_protocol = 0,
    .i_interface = 0,
};

// Alternative Interface creation using Device Descriptor class/subclass/protocol.
using DevDescInterfaceCreationTest = UsbInterfaceTest<&kDevDescInterface>;

template <>
void DevDescInterfaceCreationTest::InitTest() {
  SetUpInterface(&kDevDescInterface, sizeof(kDevDescInterface));
}

TEST_F(DevDescInterfaceCreationTest, SetUpTest) {
  constexpr usb_device_descriptor_t kDeviceDescriptor = {
      .b_length = sizeof(usb_device_descriptor_t),
      .b_descriptor_type = USB_DT_DEVICE,
      .b_device_class = 0x1,
      .b_device_sub_class = 0x2,
      .b_device_protocol = 0x3,
  };
  SetDeviceDesc(kDeviceDescriptor);

  InitTest();
  EXPECT_EQ(dut_->usb_class(), kDeviceDescriptor.b_device_class);
  EXPECT_EQ(dut_->usb_subclass(), kDeviceDescriptor.b_device_sub_class);
  EXPECT_EQ(dut_->usb_protocol(), kDeviceDescriptor.b_device_protocol);
}

// The interface configuration corresponding to a device with one interface association made up of
// two interfaces (each with one alt-interface) and an interface not in the interface association.
constexpr struct assoc_config {
  usb_interface_assoc_descriptor_t association;
  usb_interface_descriptor_t interface1;
  usb_endpoint_descriptor_t ep1_1;
  usb_endpoint_descriptor_t ep1_2;
  usb_interface_descriptor_t alt_interface1;
  usb_endpoint_descriptor_t ep1_alt1;
  usb_interface_descriptor_t interface2;
  usb_endpoint_descriptor_t ep2_1;
  usb_interface_descriptor_t alt_interface2;
} kTestInterfaceAssociation = {
    .association =
        {
            .b_length = sizeof(usb_interface_assoc_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE_ASSOCIATION,
            .b_first_interface = 0,
            .b_interface_count = 2,
            .b_function_class = 1,
            .b_function_sub_class = 1,
            .b_function_protocol = 1,
            .i_function = 1,
        },
    .interface1 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 0,
            .b_alternate_setting = 0,
            .b_num_endpoints = 2,
            .b_interface_class = 8,
            .b_interface_sub_class = 6,
            .b_interface_protocol = 80,
            .i_interface = 0,
        },
    .ep1_1 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 0x81,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .ep1_2 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 2,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .alt_interface1 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 0,
            .b_alternate_setting = 1,
            .b_num_endpoints = 1,
            .b_interface_class = 8,
            .b_interface_sub_class = 6,
            .b_interface_protocol = 80,
            .i_interface = 0,
        },
    .ep1_alt1 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 7,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .interface2 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 1,
            .b_alternate_setting = 0,
            .b_num_endpoints = 1,
            .b_interface_class = 8,
            .b_interface_sub_class = 6,
            .b_interface_protocol = 80,
            .i_interface = 0,
        },
    .ep2_1 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 0x89,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .alt_interface2 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 2,
            .b_alternate_setting = 1,
            .b_num_endpoints = 0,
            .b_interface_class = 8,
            .b_interface_sub_class = 6,
            .b_interface_protocol = 80,
            .i_interface = 0,
        },
};

// AssociationInterfaceTest tests an UsbInterface's ability to process interface descriptors
// with an association descriptor.
using AssociationInterfaceTest = UsbInterfaceTest<&kTestInterfaceAssociation>;

template <>
void AssociationInterfaceTest::InitTest() {
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterfaceAssociation.ep1_2, {}, true);
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterfaceAssociation.ep2_1, {}, true);
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterfaceAssociation.ep1_1, {}, true);
  SetUpInterface(&kTestInterfaceAssociation.association, sizeof(kTestInterfaceAssociation));
  EXPECT_EQ(dut_->usb_class(), kTestInterfaceAssociation.association.b_function_class);
  EXPECT_EQ(dut_->usb_subclass(), kTestInterfaceAssociation.association.b_function_sub_class);
  EXPECT_EQ(dut_->usb_protocol(), kTestInterfaceAssociation.association.b_function_protocol);
}

TEST_F(AssociationInterfaceTest, SetUpTest) { InitTest(); }

TEST_F(AssociationInterfaceTest, SetAltSettingTest) {
  InitTest();

  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterfaceAssociation.ep2_1, {}, false);
  usb_.ExpectControlOut(ZX_OK, USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_INTERFACE,
                        USB_REQ_SET_INTERFACE, 1, 1, ZX_TIME_INFINITE, std::vector<uint8_t>{});
  EXPECT_OK(dut_->SetAltSetting(1, 1));

  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterfaceAssociation.ep1_2, {}, false);
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterfaceAssociation.ep1_alt1, {}, true);
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterfaceAssociation.ep1_1, {}, false);
  usb_.ExpectControlOut(ZX_OK, USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_INTERFACE,
                        USB_REQ_SET_INTERFACE, 1, 0, ZX_TIME_INFINITE, std::vector<uint8_t>{});
  EXPECT_OK(dut_->SetAltSetting(0, 1));
}

// Alternative Interface Association creation using Device Descriptor class/subclass/protocol.
constexpr usb_interface_assoc_descriptor_t kDevDescAssoc = {
    .b_length = sizeof(usb_interface_assoc_descriptor_t),
    .b_descriptor_type = USB_DT_INTERFACE_ASSOCIATION,
    .b_first_interface = 0,
    .b_interface_count = 0,
    .b_function_class = 0,
    .b_function_sub_class = 0,
    .b_function_protocol = 0,
    .i_function = 0,
};

// AssociationInterfaceTest tests an UsbInterface's ability to process interface descriptors
// with an association descriptor.
using DevDescInterfaceAssociationCreationTest = UsbInterfaceTest<&kDevDescAssoc>;

template <>
void DevDescInterfaceAssociationCreationTest::InitTest() {
  SetUpInterface(&kDevDescAssoc, sizeof(kDevDescAssoc));
}

TEST_F(DevDescInterfaceAssociationCreationTest, SetUpTest) {
  constexpr usb_device_descriptor_t kDeviceDescriptor = {
      .b_length = sizeof(usb_device_descriptor_t),
      .b_descriptor_type = USB_DT_DEVICE,
      .b_device_class = 0x1,
      .b_device_sub_class = 0x2,
      .b_device_protocol = 0x3,
  };
  SetDeviceDesc(kDeviceDescriptor);

  InitTest();
  EXPECT_EQ(dut_->usb_class(), kDeviceDescriptor.b_device_class);
  EXPECT_EQ(dut_->usb_subclass(), kDeviceDescriptor.b_device_sub_class);
  EXPECT_EQ(dut_->usb_protocol(), kDeviceDescriptor.b_device_protocol);
}

}  // namespace usb_composite
