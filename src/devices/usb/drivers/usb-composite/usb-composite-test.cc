// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-composite/usb-composite.h"

#include <vector>

#include <fbl/auto_lock.h>
#include <fbl/ref_counted.h>
#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/devices/usb/drivers/usb-composite/test-helper.h"
#include "src/devices/usb/drivers/usb-composite/usb-interface.h"

namespace usb_composite {

// UsbCompositeTest is templated on the configuration descriptor data used for the test,
// UsbCompositeTest<&kDescriptors> will set up a test fixture with a USB protocol client that
// implements GetDescriptors and GetDescriptorsLength for the provided kDescriptors.
template <auto* descriptors>
class UsbCompositeTest : public zxtest::Test {
 public:
  void SetUp() override {
    fake_parent_->AddProtocol(ZX_PROTOCOL_USB, usb_.GetProto()->ops, usb_.GetProto()->ctx);

    auto endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
    ZX_ASSERT(endpoints.is_ok());
    fake_parent_->AddFidlService(fuchsia_hardware_usb::UsbService::Name,
                                 std::move(endpoints->client));

    usb_.ExpectGetDeviceDescriptor(usb_device_descriptor_t{
        .b_length = sizeof(usb_device_descriptor_t),
        .b_descriptor_type = USB_DT_DEVICE,
        .bcd_usb = 0,
        .b_device_class = 12,
        .b_device_sub_class = 14,
        .b_device_protocol = 16,
        .b_max_packet_size0 = 5,
        .id_vendor = 9,
        .id_product = 8,
        .bcd_device = 7,
        .i_manufacturer = 6,
        .i_product = 5,
        .i_serial_number = 0x12,
        .b_num_configurations = 1,
    });
    usb_.ExpectGetConfiguration(5);
    usb_.ExpectGetConfigurationDescriptorLength(ZX_OK, 5, sizeof(*descriptors));
    usb_.ExpectGetConfigurationDescriptor(
        ZX_OK, 5,
        std::vector<uint8_t>(reinterpret_cast<const uint8_t*>(descriptors),
                             reinterpret_cast<const uint8_t*>(descriptors) + sizeof(*descriptors)));
    usb_.ExpectGetDeviceId(10);
    ExpectConfigureEndpoints();
    ASSERT_OK(usb_composite::UsbComposite::Create(nullptr, fake_parent_.get()));
    composite_ = fake_parent_->GetLatestChild();
    dut_ = composite_->GetDeviceContext<usb_composite::UsbComposite>();

    auto result = fdf::RunOnDispatcherSync(dispatcher_->async_dispatcher(), [&]() {
      composite_->InitOp();
      EXPECT_OK(composite_->WaitUntilInitReplyCalled());
      EXPECT_TRUE(composite_->InitReplyCalled());
    });
    EXPECT_TRUE(result.is_ok());
  }

  void TearDown() override {
    auto result = fdf::RunOnDispatcherSync(dispatcher_->async_dispatcher(), [&]() {
      device_async_remove(composite_);
      mock_ddk::ReleaseFlaggedDevices(fake_parent_.get());
    });
    EXPECT_TRUE(result.is_ok());
  }

  struct init_test_expected_values_t {
    std::string name;
    size_t length;
    const uint8_t* start;
  };
  void InitTest(std::vector<init_test_expected_values_t> expected) {
    EXPECT_EQ(composite_->child_count(), expected.size());
    size_t i = 0;
    for (const auto& child : composite_->children()) {
      EXPECT_STREQ(child->name(), expected[i].name);

      auto* intf = child->GetDeviceContext<UsbInterface>();
      ASSERT_EQ(intf->UsbGetDescriptorsLength(), expected[i].length);
      std::vector<uint8_t> desc(expected[i].length);
      size_t actual;
      intf->UsbGetDescriptors(desc.data(), desc.size(), &actual);
      EXPECT_EQ(actual, desc.size());
      EXPECT_BYTES_EQ(expected[i].start, desc.data(), desc.size());

      i++;
    }
  }

  void SetClaimed(uint8_t interface_id) {
    fbl::AutoLock _(&dut_->lock_);
    dut_->interface_statuses_[interface_id] = UsbComposite::InterfaceStatus::CLAIMED;
  }

 protected:
  MockUsb usb_;
  MockDevice* composite_;
  usb_composite::UsbComposite* dut_;

 private:
  void ExpectConfigureEndpoints();

  std::shared_ptr<MockDevice> fake_parent_ = MockDevice::FakeRootParent();
  fdf::UnownedSynchronizedDispatcher dispatcher_ =
      fdf_testing::DriverRuntime::GetInstance()->StartBackgroundDispatcher();
};

constexpr struct interface_config_t {
  usb_configuration_descriptor_t config;
  usb_interface_descriptor_t interface1;
  usb_endpoint_descriptor_t ep1_1;
  usb_ss_ep_comp_descriptor_t ss_companion1;
  usb_endpoint_descriptor_t ep1_2;
  usb_ss_ep_comp_descriptor_t ss_companion2;
  usb_interface_descriptor_t alt1_interface1;
  usb_endpoint_descriptor_t ep1_alt1_1;
  usb_endpoint_descriptor_t ep1_alt1_2;
  usb_interface_descriptor_t alt1_interface2;
  usb_interface_descriptor_t interface2;
  usb_endpoint_descriptor_t ep2_1;
  usb_interface_descriptor_t alt2_interface1;
  usb_endpoint_descriptor_t ep2_alt1_1;
  usb_interface_descriptor_t interface3;
} kTestInterface = {
    .config =
        {
            .b_length = sizeof(usb_configuration_descriptor_t),
            .b_descriptor_type = USB_DT_CONFIG,
            .w_total_length = sizeof(interface_config_t),
            .b_num_interfaces = 3,
            .b_configuration_value = 0,
            .i_configuration = 0,
            .bm_attributes = 2,
            .b_max_power = 4,
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
    .ss_companion1 =
        {
            .b_length = sizeof(usb_ss_ep_comp_descriptor_t),
            .b_descriptor_type = USB_DT_SS_EP_COMPANION,
            .b_max_burst = 3,
            .bm_attributes = 0,
            .w_bytes_per_interval = 0,
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
    .ss_companion2 =
        {
            .b_length = sizeof(usb_ss_ep_comp_descriptor_t),
            .b_descriptor_type = USB_DT_SS_EP_COMPANION,
            .b_max_burst = 3,
            .bm_attributes = 0,
            .w_bytes_per_interval = 0,
        },
    .alt1_interface1 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 0,
            .b_alternate_setting = 1,
            .b_num_endpoints = 2,
            .b_interface_class = 8,
            .b_interface_sub_class = 6,
            .b_interface_protocol = 80,
            .i_interface = 0,
        },
    .ep1_alt1_1 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 0x85,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .ep1_alt1_2 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 3,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .alt1_interface2 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 0,
            .b_alternate_setting = 2,
            .b_num_endpoints = 0,
            .b_interface_class = 8,
            .b_interface_sub_class = 6,
            .b_interface_protocol = 80,
            .i_interface = 0,
        },
    .interface2 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 1,
            .b_alternate_setting = 0,
            .b_num_endpoints = 1,
            .b_interface_class = 5,
            .b_interface_sub_class = 7,
            .b_interface_protocol = 8,
            .i_interface = 0,
        },
    .ep2_1 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 0x82,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .alt2_interface1 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 1,
            .b_alternate_setting = 1,
            .b_num_endpoints = 1,
            .b_interface_class = 5,
            .b_interface_sub_class = 7,
            .b_interface_protocol = 8,
            .i_interface = 0,
        },
    .ep2_alt1_1 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 0x1,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .interface3 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 2,
            .b_alternate_setting = 0,
            .b_num_endpoints = 0,
            .b_interface_class = 12,
            .b_interface_sub_class = 3,
            .b_interface_protocol = 6,
            .i_interface = 0,
        },
};

// NoAssociationCompositeTest tests UsbComposite's ability to process interface descriptors
// that have no association descriptors.
using NoAssociationCompositeTest = UsbCompositeTest<&kTestInterface>;

template <>
void NoAssociationCompositeTest::ExpectConfigureEndpoints() {
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep1_2, {}, true);
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep1_1, {}, true);
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep2_1, {}, true);
}

TEST_F(NoAssociationCompositeTest, InitTest) {
  InitTest(std::vector<init_test_expected_values_t>{
      init_test_expected_values_t{
          .name = "ifc-000",
          .length = reinterpret_cast<uintptr_t>(&kTestInterface.interface2) -
                    reinterpret_cast<uintptr_t>(&kTestInterface.interface1),
          .start = reinterpret_cast<const uint8_t*>(&kTestInterface.interface1),
      },
      init_test_expected_values_t{
          .name = "ifc-001",
          .length = reinterpret_cast<uintptr_t>(&kTestInterface.interface3) -
                    reinterpret_cast<uintptr_t>(&kTestInterface.interface2),
          .start = reinterpret_cast<const uint8_t*>(&kTestInterface.interface2),
      },
      init_test_expected_values_t{
          .name = "ifc-002",
          .length = sizeof(kTestInterface.interface3),
          .start = reinterpret_cast<const uint8_t*>(&kTestInterface.interface3),
      },
  });
}

TEST_F(NoAssociationCompositeTest, ClaimInterfaceTest) {
  // GetInterfaceById failed
  EXPECT_EQ(dut_->ClaimInterface(3), ZX_ERR_INVALID_ARGS);

  // Success
  MockDevice* intf = nullptr;
  for (const auto& child : composite_->children()) {
    if (std::strcmp(child->name(), "ifc-002")) {
      intf = child.get();
      break;
    }
  }
  ASSERT_NOT_NULL(intf);
  EXPECT_OK(dut_->ClaimInterface(2));
  intf->AsyncRemoveCalled();

  // Already claimed.
  SetClaimed(1);
  EXPECT_EQ(dut_->ClaimInterface(1), ZX_ERR_ALREADY_BOUND);
}

TEST_F(NoAssociationCompositeTest, SetInterfaceTest) {
  // Success
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep2_alt1_1, {}, true);
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep2_1, {}, false);
  usb_.ExpectControlOut(ZX_OK, USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_INTERFACE,
                        USB_REQ_SET_INTERFACE, 1, 1, ZX_TIME_INFINITE, std::vector<uint8_t>{});
  EXPECT_OK(dut_->SetInterface(1, 1));

  // interface_id not found
  EXPECT_EQ(dut_->SetInterface(3, 1), ZX_ERR_INVALID_ARGS);
}

TEST_F(NoAssociationCompositeTest, GetAdditionalDescriptorListTest) {
  uint8_t desc_list[sizeof(kTestInterface.interface3)];
  size_t actual;
  EXPECT_OK(dut_->GetAdditionalDescriptorList(1, desc_list, sizeof(desc_list), &actual));
  EXPECT_EQ(actual, sizeof(desc_list));
  EXPECT_BYTES_EQ(desc_list, &kTestInterface.interface3, actual);
}

constexpr struct assoc_config_t {
  usb_configuration_descriptor_t config;
  usb_interface_assoc_descriptor_t association;
  usb_interface_descriptor_t interface1;
  usb_endpoint_descriptor_t ep1_1;
  usb_ss_ep_comp_descriptor_t ss_companion1;
  usb_endpoint_descriptor_t ep1_2;
  usb_ss_ep_comp_descriptor_t ss_companion2;
  usb_interface_descriptor_t alt1_interface1;
  usb_endpoint_descriptor_t ep1_alt1_1;
  usb_endpoint_descriptor_t ep1_alt1_2;
  usb_interface_descriptor_t alt1_interface2;
  usb_interface_descriptor_t interface2;
  usb_endpoint_descriptor_t ep2_1;
  usb_interface_descriptor_t alt2_interface1;
  usb_endpoint_descriptor_t ep2_alt1_1;
  usb_interface_descriptor_t interface3;
} kTestAssociation = {
    .config =
        {
            .b_length = sizeof(usb_configuration_descriptor_t),
            .b_descriptor_type = USB_DT_CONFIG,
            .w_total_length = sizeof(assoc_config_t),
            .b_num_interfaces = 3,
            .b_configuration_value = 0,
            .i_configuration = 0,
            .bm_attributes = 2,
            .b_max_power = 4,
        },
    .association =
        {
            .b_length = sizeof(usb_interface_assoc_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE_ASSOCIATION,
            .b_first_interface = 0,
            .b_interface_count = 2,
            .b_function_class = 5,
            .b_function_sub_class = 3,
            .b_function_protocol = 2,
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
    .ss_companion1 =
        {
            .b_length = sizeof(usb_ss_ep_comp_descriptor_t),
            .b_descriptor_type = USB_DT_SS_EP_COMPANION,
            .b_max_burst = 3,
            .bm_attributes = 0,
            .w_bytes_per_interval = 0,
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
    .ss_companion2 =
        {
            .b_length = sizeof(usb_ss_ep_comp_descriptor_t),
            .b_descriptor_type = USB_DT_SS_EP_COMPANION,
            .b_max_burst = 3,
            .bm_attributes = 0,
            .w_bytes_per_interval = 0,
        },
    .alt1_interface1 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 0,
            .b_alternate_setting = 1,
            .b_num_endpoints = 2,
            .b_interface_class = 8,
            .b_interface_sub_class = 6,
            .b_interface_protocol = 80,
            .i_interface = 0,
        },
    .ep1_alt1_1 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 0x85,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .ep1_alt1_2 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 3,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .alt1_interface2 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 0,
            .b_alternate_setting = 2,
            .b_num_endpoints = 0,
            .b_interface_class = 8,
            .b_interface_sub_class = 6,
            .b_interface_protocol = 80,
            .i_interface = 0,
        },
    .interface2 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 1,
            .b_alternate_setting = 0,
            .b_num_endpoints = 1,
            .b_interface_class = 5,
            .b_interface_sub_class = 7,
            .b_interface_protocol = 8,
            .i_interface = 0,
        },
    .ep2_1 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 0x82,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .alt2_interface1 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 1,
            .b_alternate_setting = 1,
            .b_num_endpoints = 1,
            .b_interface_class = 5,
            .b_interface_sub_class = 7,
            .b_interface_protocol = 8,
            .i_interface = 0,
        },
    .ep2_alt1_1 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 0x1,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .interface3 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 2,
            .b_alternate_setting = 0,
            .b_num_endpoints = 0,
            .b_interface_class = 12,
            .b_interface_sub_class = 3,
            .b_interface_protocol = 6,
            .i_interface = 0,
        },
};

// AssociationCompositeTest tests UsbComposite's ability to process interface descriptors
// that have association descriptors.
using AssociationCompositeTest = UsbCompositeTest<&kTestAssociation>;

template <>
void AssociationCompositeTest::ExpectConfigureEndpoints() {
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep1_2, {}, true);
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep1_1, {}, true);
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep2_1, {}, true);
}

TEST_F(AssociationCompositeTest, InitTest) {
  InitTest(std::vector<init_test_expected_values_t>{
      init_test_expected_values_t{
          .name = "asc-000",
          .length = reinterpret_cast<uintptr_t>(&kTestAssociation.interface3) -
                    reinterpret_cast<uintptr_t>(&kTestAssociation.association),
          .start = reinterpret_cast<const uint8_t*>(&kTestAssociation.association),
      },
      init_test_expected_values_t{
          .name = "ifc-002",
          .length = sizeof(kTestAssociation.interface3),
          .start = reinterpret_cast<const uint8_t*>(&kTestAssociation.interface3),
      },
  });
}

constexpr struct two_assoc_config_t {
  usb_configuration_descriptor_t config;
  usb_interface_assoc_descriptor_t association1;
  usb_interface_descriptor_t interface1;
  usb_endpoint_descriptor_t ep1_1;
  usb_ss_ep_comp_descriptor_t ss_companion1;
  usb_endpoint_descriptor_t ep1_2;
  usb_ss_ep_comp_descriptor_t ss_companion2;
  usb_interface_descriptor_t alt1_interface1;
  usb_endpoint_descriptor_t ep1_alt1_1;
  usb_endpoint_descriptor_t ep1_alt1_2;
  usb_interface_descriptor_t alt1_interface2;
  usb_interface_descriptor_t interface2;
  usb_endpoint_descriptor_t ep2_1;
  usb_interface_descriptor_t alt2_interface1;
  usb_endpoint_descriptor_t ep2_alt1_1;
  usb_interface_assoc_descriptor_t association2;
  usb_interface_descriptor_t interface3;
} kTestTwoAssociation = {
    .config =
        {
            .b_length = sizeof(usb_configuration_descriptor_t),
            .b_descriptor_type = USB_DT_CONFIG,
            .w_total_length = sizeof(two_assoc_config_t),
            .b_num_interfaces = 3,
            .b_configuration_value = 0,
            .i_configuration = 0,
            .bm_attributes = 2,
            .b_max_power = 4,
        },
    .association1 =
        {
            .b_length = sizeof(usb_interface_assoc_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE_ASSOCIATION,
            .b_first_interface = 0,
            .b_interface_count = 2,
            .b_function_class = 5,
            .b_function_sub_class = 3,
            .b_function_protocol = 2,
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
    .ss_companion1 =
        {
            .b_length = sizeof(usb_ss_ep_comp_descriptor_t),
            .b_descriptor_type = USB_DT_SS_EP_COMPANION,
            .b_max_burst = 3,
            .bm_attributes = 0,
            .w_bytes_per_interval = 0,
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
    .ss_companion2 =
        {
            .b_length = sizeof(usb_ss_ep_comp_descriptor_t),
            .b_descriptor_type = USB_DT_SS_EP_COMPANION,
            .b_max_burst = 3,
            .bm_attributes = 0,
            .w_bytes_per_interval = 0,
        },
    .alt1_interface1 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 0,
            .b_alternate_setting = 1,
            .b_num_endpoints = 2,
            .b_interface_class = 8,
            .b_interface_sub_class = 6,
            .b_interface_protocol = 80,
            .i_interface = 0,
        },
    .ep1_alt1_1 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 0x85,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .ep1_alt1_2 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 3,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .alt1_interface2 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 0,
            .b_alternate_setting = 2,
            .b_num_endpoints = 0,
            .b_interface_class = 8,
            .b_interface_sub_class = 6,
            .b_interface_protocol = 80,
            .i_interface = 0,
        },
    .interface2 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 1,
            .b_alternate_setting = 0,
            .b_num_endpoints = 1,
            .b_interface_class = 5,
            .b_interface_sub_class = 7,
            .b_interface_protocol = 8,
            .i_interface = 0,
        },
    .ep2_1 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 0x82,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .alt2_interface1 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 1,
            .b_alternate_setting = 1,
            .b_num_endpoints = 1,
            .b_interface_class = 5,
            .b_interface_sub_class = 7,
            .b_interface_protocol = 8,
            .i_interface = 0,
        },
    .ep2_alt1_1 =
        {
            .b_length = sizeof(usb_endpoint_descriptor_t),
            .b_descriptor_type = USB_DT_ENDPOINT,
            .b_endpoint_address = 0x1,
            .bm_attributes = 2,
            .w_max_packet_size = 1024,
            .b_interval = 0,
        },
    .association2 =
        {
            .b_length = sizeof(usb_interface_assoc_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE_ASSOCIATION,
            .b_first_interface = 2,
            .b_interface_count = 1,
            .b_function_class = 5,
            .b_function_sub_class = 3,
            .b_function_protocol = 2,
            .i_function = 1,
        },
    .interface3 =
        {
            .b_length = sizeof(usb_interface_descriptor_t),
            .b_descriptor_type = USB_DT_INTERFACE,
            .b_interface_number = 2,
            .b_alternate_setting = 0,
            .b_num_endpoints = 0,
            .b_interface_class = 12,
            .b_interface_sub_class = 3,
            .b_interface_protocol = 6,
            .i_interface = 0,
        },
};

// TwoAssociationCompositeTest tests UsbComposite's ability to process interface descriptors
// that have consecutive association descriptors.
using TwoAssociationCompositeTest = UsbCompositeTest<&kTestTwoAssociation>;

template <>
void TwoAssociationCompositeTest::ExpectConfigureEndpoints() {
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep1_2, {}, true);
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep1_1, {}, true);
  usb_.ExpectEnableEndpoint(ZX_OK, kTestInterface.ep2_1, {}, true);
}

TEST_F(TwoAssociationCompositeTest, InitTest) {
  InitTest(std::vector<init_test_expected_values_t>{
      init_test_expected_values_t{
          .name = "asc-000",
          .length = reinterpret_cast<uintptr_t>(&kTestTwoAssociation.association2) -
                    reinterpret_cast<uintptr_t>(&kTestTwoAssociation.association1),
          .start = reinterpret_cast<const uint8_t*>(&kTestTwoAssociation.association1),
      },
      init_test_expected_values_t{
          .name = "asc-002",
          .length =
              sizeof(kTestTwoAssociation.association2) + sizeof(kTestTwoAssociation.interface3),
          .start = reinterpret_cast<const uint8_t*>(&kTestTwoAssociation.association2),
      },
  });
}

}  // namespace usb_composite
