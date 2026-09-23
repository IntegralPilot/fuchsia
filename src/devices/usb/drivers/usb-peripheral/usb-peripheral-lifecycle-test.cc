// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>

#include <atomic>

#include "src/devices/usb/drivers/usb-peripheral/usb-peripheral-test-harness.h"

namespace usb_peripheral::test {
namespace {

class UsbPeripheralTestHelper {
 public:
  static size_t active_functions_count(const UsbPeripheral& peripheral) {
    return peripheral.active_functions_count();
  }
  static void DecrementActiveFunctions(UsbPeripheral& peripheral, uint64_t config_generation) {
    peripheral.DecrementActiveFunctions(config_generation);
  }
  static void FunctionCleared(UsbPeripheral& peripheral, size_t function_index,
                              uint64_t config_generation) {
    peripheral.FunctionCleared(function_index, config_generation);
  }
};

TEST_F(ManagedUsbPeripheralTest, AddsCorrectSerialNumberMetadata) {
  fdescriptor::wire::UsbSetup setup;
  setup.w_length = 256;
  setup.w_value = 0x3 | (USB_DT_STRING << 8);
  setup.bm_request_type = USB_DIR_IN | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_GET_DESCRIPTOR;

  fidl::Arena arena;
  std::vector<uint8_t> unused;
  auto result =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));

  ASSERT_TRUE(result->is_ok());

  auto& serial = result.value()->read;

  EXPECT_EQ(serial[0], (kSerialNumber.size() + 1) * 2);
  EXPECT_EQ(serial[1], USB_DT_STRING);
  for (size_t i = 0; i < kSerialNumber.size(); i++) {
    EXPECT_EQ(serial[2 + (i * 2)], kSerialNumber[i]);
  }
}

TEST_F(ManagedUsbPeripheralTest, WorksWithVendorSpecificCommandWhenConfigurationIsZero) {
  fdescriptor::wire::UsbSetup setup;
  setup.w_length = 256;
  setup.w_value = 0x3 | (USB_DT_STRING << 8);
  setup.bm_request_type = USB_DIR_IN | USB_RECIP_DEVICE | USB_TYPE_VENDOR;
  setup.b_request = USB_REQ_GET_DESCRIPTOR;

  fidl::Arena arena;
  std::vector<uint8_t> unused;
  auto result =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(result->is_error());
  ASSERT_EQ(ZX_ERR_BAD_STATE, result->error_value());
}

TEST_F(ManagedUsbPeripheralTest, BosDescriptorVersionHandling) {
  fdescriptor::wire::UsbSetup setup;
  setup.w_length = sizeof(usb_bos_descriptor_t);
  setup.w_value = USB_DT_BOS << 8;
  setup.bm_request_type = USB_DIR_IN | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_GET_DESCRIPTOR;

  uint16_t orig_bcd_usb = 0;
  this->dut().RunInDriverContext(
      [&](UsbPeripheral& driver) { orig_bcd_usb = le16toh(driver.device_desc().bcd_usb); });
  auto restore_bcd_usb = fit::defer([this, orig_bcd_usb]() {
    this->dut().RunInDriverContext(
        [orig_bcd_usb](UsbPeripheral& driver) { driver.SetBcdUsbForTesting(orig_bcd_usb); });
  });

  // 1. USB 2.0 (0x0200) without BOS capabilities must stall GET_DESCRIPTOR(BOS).
  this->dut().RunInDriverContext(
      [](UsbPeripheral& driver) { driver.SetBcdUsbForTesting(USB_2_0); });
  {
    fidl::Arena arena;
    std::vector<uint8_t> unused;
    auto result =
        dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
    ASSERT_TRUE(result.ok()) << result.FormatDescription();
    ASSERT_TRUE(result->is_error());
    EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, result->error_value());
  }

  // 2. USB 2.0.1 (0x0201) transition point supports BOS descriptors.
  this->dut().RunInDriverContext(
      [](UsbPeripheral& driver) { driver.SetBcdUsbForTesting(USB_2_0_1); });
  {
    fidl::Arena arena;
    std::vector<uint8_t> unused;
    auto result =
        dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
    ASSERT_TRUE(result.ok()) << result.FormatDescription();
    ASSERT_TRUE(result->is_ok());
  }

  // 3. USB 2.1 (0x0210) supports BOS descriptors.
  this->dut().RunInDriverContext(
      [](UsbPeripheral& driver) { driver.SetBcdUsbForTesting(USB_2_1); });
  {
    fidl::Arena arena;
    std::vector<uint8_t> unused;
    auto result =
        dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
    ASSERT_TRUE(result.ok()) << result.FormatDescription();
    ASSERT_TRUE(result->is_ok());
  }

  // 4. USB 3.1 (0x0310) SuperSpeed supports BOS descriptors.
  this->dut().RunInDriverContext(
      [](UsbPeripheral& driver) { driver.SetBcdUsbForTesting(USB_3_1); });
  {
    fidl::Arena arena;
    std::vector<uint8_t> unused;
    auto result =
        dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
    ASSERT_TRUE(result.ok()) << result.FormatDescription();
    ASSERT_TRUE(result->is_ok());
  }

  // 5. Non-zero descriptor index or w_index must stall even on USB 2.1+ devices.
  this->dut().RunInDriverContext(
      [](UsbPeripheral& driver) { driver.SetBcdUsbForTesting(USB_2_1); });
  {
    fidl::Arena arena;
    std::vector<uint8_t> unused;

    // Non-zero descriptor index ((wValue & 0xFF) != 0).
    setup.w_value = (USB_DT_BOS << 8) | 1;
    setup.w_index = 0;
    auto res_index =
        dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
    ASSERT_TRUE(res_index.ok()) << res_index.FormatDescription();
    ASSERT_TRUE(res_index->is_error());
    EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, res_index->error_value());

    // Non-zero w_index (w_index != 0).
    setup.w_value = USB_DT_BOS << 8;
    setup.w_index = 1;
    auto res_windex =
        dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
    ASSERT_TRUE(res_windex.ok()) << res_windex.FormatDescription();
    ASSERT_TRUE(res_windex->is_error());
    EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, res_windex->error_value());
  }
}

TEST_F(ManagedUsbPeripheralTest, GetBosDescriptor) {
  // Validates standard GET_DESCRIPTOR request for the Binary Device Object Store
  // (BOS, USB_DT_BOS) descriptor per USB 3.2 Specification Section 9.6.2.
  // Validates that the returned descriptor has valid header length, descriptor type USB_DT_BOS,
  // matching total length (wTotalLength), and correct device capability count.
  uint16_t orig_bcd_usb = 0;
  this->dut().RunInDriverContext(
      [&](UsbPeripheral& driver) { orig_bcd_usb = le16toh(driver.device_desc().bcd_usb); });
  auto restore_bcd_usb = fit::defer([this, orig_bcd_usb]() {
    this->dut().RunInDriverContext(
        [orig_bcd_usb](UsbPeripheral& driver) { driver.SetBcdUsbForTesting(orig_bcd_usb); });
  });

  this->dut().RunInDriverContext(
      [](UsbPeripheral& driver) { driver.SetBcdUsbForTesting(USB_3_1); });
  fdescriptor::wire::UsbSetup setup;
  setup.bm_request_type = USB_DIR_IN | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_GET_DESCRIPTOR;
  setup.w_value = (USB_DT_BOS << 8) | 0;
  setup.w_index = 0;
  setup.w_length = sizeof(usb_bos_descriptor_t);

  fidl::Arena arena;
  auto result = dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>());

  ASSERT_TRUE(result.ok()) << result.FormatDescription();
  ASSERT_TRUE(result->is_ok());

  auto& desc = result->value()->read;
  ASSERT_EQ(desc.size(), sizeof(usb_bos_descriptor_t));
  EXPECT_EQ(desc[0], sizeof(usb_bos_descriptor_t));
  EXPECT_EQ(desc[1], USB_DT_BOS);
  usb_bos_descriptor_t bos_desc;
  std::memcpy(&bos_desc, desc.data(), sizeof(bos_desc));
  EXPECT_EQ(le16toh(bos_desc.w_total_length), sizeof(usb_bos_descriptor_t));
  EXPECT_EQ(bos_desc.b_num_device_caps, 0);
}

TEST_F(ManagedUsbPeripheralTest, GetLanguageTableStringDescriptor) {
  // Validates standard GET_DESCRIPTOR request for String Descriptor Index 0 (Language Table)
  // per USB 2.0 Specification Section 9.6.7. Returns array of supported LANGIDs (0x0409 English
  // US). Validates descriptor length, string descriptor type, and default English language ID.
  fdescriptor::wire::UsbSetup setup;
  setup.bm_request_type = USB_DIR_IN | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_GET_DESCRIPTOR;
  setup.w_value = (USB_DT_STRING << 8) | 0;
  setup.w_index = 0;
  setup.w_length = 256;

  fidl::Arena arena;
  auto result = dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>());

  ASSERT_TRUE(result.ok()) << result.FormatDescription();
  ASSERT_TRUE(result->is_ok());

  auto& desc = result->value()->read;
  ASSERT_EQ(desc.size(), 4u);
  EXPECT_EQ(desc[0], 4);
  EXPECT_EQ(desc[1], USB_DT_STRING);
  // Default language is English (United States) - 0x0409 (little-endian: 0x09, 0x04).
  EXPECT_EQ(desc[2], 0x09);
  EXPECT_EQ(desc[3], 0x04);
}

TEST_F(ManagedUsbPeripheralTest, GetStatusEndpointZeroInWhenUnconfigured) {
  // Validates that GET_STATUS addressed to Endpoint 0 IN (wIndex = 0x80) is valid
  // and returns 2 bytes of zero status even before the device is configured, per
  // USB 2.0 Specification Section 9.3.4 and Table 9-3 (the default control pipe is
  // always accessible in both directions).
  fdescriptor::wire::UsbSetup setup;
  setup.bm_request_type = USB_DIR_IN | USB_RECIP_ENDPOINT | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_GET_STATUS;
  setup.w_value = 0;
  setup.w_index = 0x80;  // EP 0 IN
  setup.w_length = 2;

  fidl::Arena arena;
  auto result = dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>());

  ASSERT_TRUE(result.ok()) << result.FormatDescription();
  ASSERT_TRUE(result->is_ok());
  auto& resp = result->value()->read;
  ASSERT_EQ(resp.size(), 2u);
  EXPECT_EQ(resp[0], 0);
  EXPECT_EQ(resp[1], 0);
}

TEST_F(UsbPeripheralReadyTest, InspectMetrics) {
  // Initial state should be PeripheralReady.
  {
    inspect::Hierarchy hierarchy;
    this->dut().RunInDriverContext([&](UsbPeripheral& driver) {
      hierarchy = usb_inspect::ReadHierarchyFromInspector(driver.inspector());
    });

    auto* dci_metrics = hierarchy.GetByPath({"usb-peripheral", "dci_metrics"});
    ASSERT_NE(dci_metrics, nullptr);
    EXPECT_THAT(*dci_metrics,
                NodeMatches(AllOf(NameMatches("dci_metrics"),
                                  PropertyList(Contains(StringIs("state", "kPeripheralReady"))))));

    auto* function_node = hierarchy.GetByPath({"usb-peripheral", "function-000"});
    ASSERT_NE(function_node, nullptr);
    EXPECT_THAT(*function_node, NodeMatches(AllOf(NameMatches("function-000"),
                                                  PropertyList(Contains(UintIs("index", 0))))));

    auto* interface_node = hierarchy.GetByPath({"usb-peripheral", "function-000", "interface-000"});
    ASSERT_NE(interface_node, nullptr);
    EXPECT_THAT(*interface_node,
                NodeMatches(AllOf(
                    NameMatches("interface-000"),
                    PropertyList(::testing::UnorderedElementsAre(
                        UintIs("interface_number", ::testing::_), UintIs("alternate_setting", 0),
                        UintIs("num_endpoints", 2), UintIs("interface_class", 255),
                        UintIs("interface_subclass", 0), UintIs("interface_protocol", 0))))));

    auto& ep_children = interface_node->children();
    ASSERT_EQ(ep_children.size(), 2u);
    EXPECT_THAT(ep_children[0],
                NodeMatches(AllOf(PropertyList(Contains(UintIs("attributes", 2))),
                                  PropertyList(Contains(UintIs("max_packet_size", 64))))));
  }

  // Connect host.
  auto connected_res = this->dci()->SetConnected(true);
  ASSERT_TRUE(connected_res.ok());
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Check Inspect again.
  {
    inspect::Hierarchy hierarchy;
    this->dut().RunInDriverContext([&](UsbPeripheral& driver) {
      hierarchy = usb_inspect::ReadHierarchyFromInspector(driver.inspector());
    });

    auto* dci_metrics = hierarchy.GetByPath({"usb-peripheral", "dci_metrics"});
    ASSERT_NE(dci_metrics, nullptr);
    EXPECT_THAT(*dci_metrics,
                NodeMatches(AllOf(NameMatches("dci_metrics"),
                                  PropertyList(Contains(StringIs("state", "kHostConnected"))))));

    // Check dci_metrics.
    EXPECT_THAT(*dci_metrics,
                NodeMatches(AllOf(NameMatches("dci_metrics"),
                                  PropertyList(Contains(BoolIs("connected", true))))));
  }

  // Disconnect host.
  auto disconnected_res = this->dci()->SetConnected(false);
  ASSERT_TRUE(disconnected_res.ok());
  ExpectStateEventual(UsbPeripheral::DeviceState::kPeripheralReady);

  // Check Inspect again.
  {
    inspect::Hierarchy hierarchy;
    this->dut().RunInDriverContext([&](UsbPeripheral& driver) {
      hierarchy = usb_inspect::ReadHierarchyFromInspector(driver.inspector());
    });

    auto* dci_metrics = hierarchy.GetByPath({"usb-peripheral", "dci_metrics"});
    ASSERT_NE(dci_metrics, nullptr);
    EXPECT_THAT(*dci_metrics,
                NodeMatches(AllOf(NameMatches("dci_metrics"),
                                  PropertyList(Contains(StringIs("state", "kPeripheralReady"))))));

    EXPECT_THAT(*dci_metrics,
                NodeMatches(AllOf(NameMatches("dci_metrics"),
                                  PropertyList(Contains(BoolIs("connected", false))))));
  }
}

TEST_F(UsbPeripheralReadyTest, HostConnectionToggle) {
  for (int i = 0; i < 10; ++i) {
    // Connect host.
    auto connected_res = this->dci()->SetConnected(true);
    ASSERT_TRUE(connected_res.ok());
    ExpectState(UsbPeripheral::DeviceState::kHostConnected);

    // Disconnect host.
    auto disconnected_res = this->dci()->SetConnected(false);
    ASSERT_TRUE(disconnected_res.ok());
    ExpectStateEventual(UsbPeripheral::DeviceState::kPeripheralReady);
  }
}

TEST_F(UsbPeripheralReadyTest, HostDisconnectResetsConfiguration) {
  // Connect host.
  auto connected_res = this->dci()->SetConnected(true);
  ASSERT_TRUE(connected_res.ok());
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Set configuration to 1.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_SET_CONFIGURATION,
        .w_value = 1,
        .w_index = 0,
        .w_length = 0,
    };
    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
  }

  // Get configuration should be 1.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_GET_CONFIGURATION,
        .w_value = 0,
        .w_index = 0,
        .w_length = 1,
    };
    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
    ASSERT_EQ(res->value()->read.size(), 1u);
    EXPECT_EQ(res->value()->read[0], 1);
  }

  // Disconnect host.
  auto disconnected_res = this->dci()->SetConnected(false);
  ASSERT_TRUE(disconnected_res.ok());
  ExpectStateEventual(UsbPeripheral::DeviceState::kPeripheralReady);

  // Get configuration should be 0.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_GET_CONFIGURATION,
        .w_value = 0,
        .w_index = 0,
        .w_length = 1,
    };
    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
    ASSERT_EQ(res->value()->read.size(), 1u);
    EXPECT_EQ(res->value()->read[0], 0);
  }
}

TEST_F(UsbPeripheralReadyTest, DisconnectHostWhenAlreadyPeripheralReady) {
  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);
  FakeUsbFunction& fake = *function_clients_.fakes[0];
  EXPECT_FALSE(fake.set_configured_called());
  EXPECT_FALSE(fake.configured());

  {
    auto disconnected_res = this->dci()->SetConnected(false);
    ASSERT_TRUE(disconnected_res.ok());
  }

  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);

  fake.WaitUntilCalled();
  EXPECT_TRUE(fake.set_configured_called());
  EXPECT_FALSE(fake.configured());

  fake.clear_set_configured_called();
  {
    auto disconnected_res = this->dci()->SetConnected(false);
    ASSERT_TRUE(disconnected_res.ok());
  }
  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);

  // Not called again. Run in the fake's dispatcher so we know it would've
  // processed any call made as part of processing SetConnected.
  libsync::Completion comp;
  async::PostTask(fake.dispatcher()->async_dispatcher(), [&]() {
    EXPECT_FALSE(fake.set_configured_called());
    EXPECT_FALSE(fake.configured());
    comp.Signal();
  });
  ASSERT_OK(comp.Wait(zx::sec(5)));
}

TEST_F(UsbPeripheralReadyTest, GetConfigurationDescriptor) {
  // Validates standard GET_DESCRIPTOR request for the Configuration Descriptor (USB_DT_CONFIG)
  // and its subordinate descriptors (Interface and Endpoint) per USB 2.0 Specification
  // Section 9.4.3 and Section 9.6.3.
  // Validates configuration header fields, total returned length (wTotalLength), number of
  // interfaces, and subordinate interface descriptor type and index.
  fdescriptor::wire::UsbSetup setup;
  setup.bm_request_type = USB_DIR_IN | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_GET_DESCRIPTOR;
  setup.w_value = (USB_DT_CONFIG << 8) | 0;
  setup.w_index = 0;
  setup.w_length = 256;

  fidl::Arena arena;
  auto result = dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>());

  ASSERT_TRUE(result.ok()) << result.FormatDescription();
  ASSERT_TRUE(result->is_ok());

  auto& desc = result->value()->read;
  ASSERT_GE(desc.size(),
            sizeof(usb_configuration_descriptor_t) + sizeof(usb_interface_descriptor_t));
  ASSERT_EQ(desc[0], sizeof(usb_configuration_descriptor_t));
  ASSERT_EQ(desc[1], USB_DT_CONFIG);
  usb_configuration_descriptor_t config_desc;
  std::memcpy(&config_desc, desc.data(), sizeof(config_desc));
  EXPECT_EQ(le16toh(config_desc.w_total_length), desc.size());
  EXPECT_GE(config_desc.b_num_interfaces, 1);
  size_t config_len = config_desc.b_length;
  ASSERT_LE(config_len + sizeof(usb_interface_descriptor_t), desc.size());
  usb_interface_descriptor_t intf_desc;
  std::memcpy(&intf_desc, desc.data() + config_len, sizeof(intf_desc));
  EXPECT_EQ(intf_desc.b_length, sizeof(usb_interface_descriptor_t));
  EXPECT_EQ(intf_desc.b_descriptor_type, USB_DT_INTERFACE);
  EXPECT_EQ(intf_desc.b_interface_number, 0);
}

TEST_F(UsbPeripheralReadyTest, GetDeviceDescriptor) {
  // Validates standard GET_DESCRIPTOR request for the Device Descriptor (USB_DT_DEVICE)
  // per USB 2.0 Specification Section 9.4.3 & Section 9.6.1.
  // Validates descriptor header length, USB specification release number (bcdUSB), EP0
  // max packet size (64 bytes), and configuration count.
  fdescriptor::wire::UsbSetup setup;
  setup.bm_request_type = USB_DIR_IN | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_GET_DESCRIPTOR;
  setup.w_value = (USB_DT_DEVICE << 8) | 0;
  setup.w_index = 0;
  setup.w_length = sizeof(usb_device_descriptor_t);

  fidl::Arena arena;
  auto result = dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>());

  ASSERT_TRUE(result.ok()) << result.FormatDescription();
  ASSERT_TRUE(result->is_ok());

  auto& desc = result->value()->read;
  ASSERT_EQ(desc.size(), sizeof(usb_device_descriptor_t));
  EXPECT_EQ(desc[0], sizeof(usb_device_descriptor_t));
  EXPECT_EQ(desc[1], USB_DT_DEVICE);
  usb_device_descriptor_t dev_desc;
  std::memcpy(&dev_desc, desc.data(), sizeof(dev_desc));
  EXPECT_EQ(le16toh(dev_desc.bcd_usb), USB_2_0);
  EXPECT_EQ(dev_desc.b_max_packet_size0, 64);
  EXPECT_GE(dev_desc.b_num_configurations, 1);
}

TEST_F(UsbPeripheralReadyTest, GetDeviceQualifierDescriptor) {
  // Validates standard GET_DESCRIPTOR request for the Device Qualifier Descriptor
  // (USB_DT_DEVICE_QUALIFIER) per USB 2.0 Specification Section 9.4.3 & Section 9.6.2.
  // Validates qualifier header length, bcdUSB version, reserved byte setting, and
  // available alternate-speed configuration count.
  fdescriptor::wire::UsbSetup setup;
  setup.bm_request_type = USB_DIR_IN | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_GET_DESCRIPTOR;
  setup.w_value = (USB_DT_DEVICE_QUALIFIER << 8) | 0;
  setup.w_index = 0;
  setup.w_length = sizeof(usb_device_qualifier_descriptor_t);

  fidl::Arena arena;
  auto result = dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>());

  ASSERT_TRUE(result.ok()) << result.FormatDescription();
  ASSERT_TRUE(result->is_ok());

  auto& desc = result->value()->read;
  ASSERT_EQ(desc.size(), sizeof(usb_device_qualifier_descriptor_t));
  EXPECT_EQ(desc[0], sizeof(usb_device_qualifier_descriptor_t));
  EXPECT_EQ(desc[1], USB_DT_DEVICE_QUALIFIER);
  usb_device_qualifier_descriptor_t qualifier;
  std::memcpy(&qualifier, desc.data(), sizeof(qualifier));
  EXPECT_EQ(le16toh(qualifier.bcd_usb), USB_2_0);
  EXPECT_EQ(qualifier.b_reserved, 0);
  EXPECT_GE(qualifier.b_num_configurations, 1);
}

TEST_F(UnmanagedUsbPeripheralTest, ClearFunctionsWhenNoneAdded) {
  StartDriverWithConfig(usb_peripheral_config::Config{});

  auto client = this->Client();

  zx::result endpoints = fidl::CreateEndpoints<fperipheral::Events>();
  ASSERT_OK(endpoints);

  FakeEvents fake_events;
  fake_events.Bind(std::move(endpoints->server));

  auto set_listener_res = client->SetStateChangeListener(std::move(endpoints->client));
  ASSERT_TRUE(set_listener_res.ok()) << set_listener_res.FormatDescription();

  // Clear functions - should work immediately.
  auto clear_res = client->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

  fake_events.WaitUntilCleared(this->dut().runtime());
  fake_events.Unbind();

  // Verify Inspect state is kNoConfiguration.
  ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);
}

TEST_F(UnmanagedUsbPeripheralTest, GetConfiguration) {
  StartDriverWithConfig(usb_peripheral_config::Config{});

  auto client = this->Client();

  // Test GetConfiguration before configuration is set returns ZX_ERR_BAD_STATE.
  {
    auto res = client->GetConfiguration();
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_error());
    EXPECT_EQ(res->error_value(), ZX_ERR_BAD_STATE);
  }

  // Set device descriptor and configuration.
  {
    fperipheral::wire::DeviceDescriptor dev_desc = {
        .bcd_usb = 0x0200,
        .b_device_class = 0,
        .b_device_sub_class = 0,
        .b_device_protocol = 0,
        .b_max_packet_size0 = 64,
        .id_vendor = 0x18d1,
        .id_product = 0xa001,
        .bcd_device = 0x0100,
        .manufacturer = fidl::StringView::FromExternal("Google"),
        .product = fidl::StringView::FromExternal("Fuchsia"),
        .serial = fidl::StringView::FromExternal("12345678"),
        .b_num_configurations = 1,
    };

    fidl::Arena arena;
    fidl::VectorView<fperipheral::wire::FunctionDescriptor> functions(arena, 1);
    functions[0] = fperipheral::wire::FunctionDescriptor{
        .interface_class = 0xff,
        .interface_subclass = 0x00,
        .interface_protocol = 0x00,
    };
    fidl::VectorView<fidl::VectorView<fperipheral::wire::FunctionDescriptor>> configs(arena, 1);
    configs[0] = functions;

    auto set_desc_res = client->SetConfiguration(dev_desc, configs);
    ASSERT_TRUE(set_desc_res.ok()) << set_desc_res.FormatDescription();
    ASSERT_TRUE(set_desc_res->is_ok());
  }

  // Verify GetConfiguration returns the configured descriptors and strings.
  {
    auto res = client->GetConfiguration();
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
    const auto& dev_desc = res->value()->device_desc;
    EXPECT_EQ(dev_desc.id_vendor, 0x18d1);
    EXPECT_EQ(dev_desc.id_product, 0xa001);
    EXPECT_EQ(std::string_view(dev_desc.manufacturer.data(), dev_desc.manufacturer.size()),
              "Google");
    EXPECT_EQ(std::string_view(dev_desc.product.data(), dev_desc.product.size()), "Fuchsia");
    EXPECT_EQ(std::string_view(dev_desc.serial.data(), dev_desc.serial.size()), "12345678");
  }
}

TEST_F(UnmanagedUsbPeripheralTest, ClearFunctionsDoubleCallIsNoOp) {
  StartDriverWithConfig(usb_peripheral_config::Config{});

  auto client = this->Client();

  zx::result endpoints = fidl::CreateEndpoints<fperipheral::Events>();
  ASSERT_OK(endpoints);

  FakeEvents fake_events;
  fake_events.Bind(std::move(endpoints->server));

  auto set_listener_res = client->SetStateChangeListener(std::move(endpoints->client));
  ASSERT_TRUE(set_listener_res.ok()) << set_listener_res.FormatDescription();

  // Clear functions once.
  auto clear_res = client->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();
  fake_events.WaitUntilCleared(this->dut().runtime());

  // Clear functions twice - should succeed and be a no-op since no functions are bound.
  auto clear_res2 = client->ClearFunctions();
  ASSERT_TRUE(clear_res2.ok()) << clear_res2.FormatDescription();

  fake_events.Unbind();
  ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);
}

TEST_F(UnmanagedUsbPeripheralTest, KbootFunctionsOverrideFunctions) {
  usb_peripheral_config::Config config;
  config.functions() = {"ums"};
  config.kboot_functions() = "cdc,adb";
  StartDriverWithConfig(config);

  fdescriptor::wire::UsbSetup setup;
  setup.w_length = sizeof(usb_device_descriptor_t);
  setup.bm_request_type = USB_DIR_IN | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_GET_DESCRIPTOR;
  setup.w_value = USB_DT_DEVICE << 8;
  setup.w_index = 0;
  setup.w_length = sizeof(usb_device_descriptor_t);

  fidl::Arena arena;
  std::vector<uint8_t> unused;
  fidl::WireUnownedResult result =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));

  ASSERT_TRUE(result.ok());
  ASSERT_FALSE(result->is_error());
  ASSERT_EQ(sizeof(usb_device_descriptor_t), result->value()->read.size());

  usb_device_descriptor_t desc;
  std::memcpy(&desc, result->value()->read.data(), sizeof(usb_device_descriptor_t));

  // Determined by config.kboot_functions() above.
  ASSERT_EQ(GOOGLE_USB_CDC_AND_ADB_PID, desc.id_product);
}

TEST_F(UnmanagedUsbPeripheralTest, ClearFunctionsWhenAlreadyUnbound) {
  usb_peripheral_config::Config config;
  config.functions() = {"test", "cdc"};
  StartDriverWithConfig(config);

  auto client = Client();

  zx::result endpoints = fidl::CreateEndpoints<fperipheral::Events>();
  ASSERT_OK(endpoints);

  FakeEvents fake_events;
  fake_events.Bind(std::move(endpoints->server));

  auto set_listener_res = client->SetStateChangeListener(std::move(endpoints->client));
  ASSERT_TRUE(set_listener_res.ok()) << set_listener_res.FormatDescription();

  // Trigger framework-level removal for BOTH function-000 and function-001.
  SimulateFunctionUnbind({"function-000", "function-001"});

  // Process unbind handlers.
  this->dut().runtime().RunUntilIdle();

  // The peripheral driver should detect the spontaneous unbind and drop to kWaitForFunctionBind.
  ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);
  ExpectControllerStarted(false);

  // Verify that the framework has actually removed the nodes.
  WaitUntilChildNodeCount(0);

  // Call ClearFunctions. Since they are already unbound, this will return immediately.
  auto clear_res = client->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();
  ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);
  ExpectControllerStarted(false);

  // Verify that the node count remains zero.
  ExpectChildNodeCount(0);

  fake_events.WaitUntilCleared(this->dut().runtime());
  fake_events.Unbind();

  ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);
}

TEST_F(UnmanagedUsbPeripheralTest, ClearFunctionsDuringBind) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto client = Client();

  // We are now in kWaitForFunctionBind (as child devices were added).
  ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);

  // Call ClearFunctions before any function driver has registered.
  auto clear_res = client->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

  // Process the unbinds.
  this->dut().runtime().RunUntilIdle();

  // Should reach kNoConfiguration.
  ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, FunctionInterfaceClosedButNodeBound) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);

  // Simulate a function driver closing its interface.
  {
    function_clients.value().clients[0] = {};
  }

  // The peripheral driver should detect this and drop back to kWaitForFunctionBind.
  WaitUntilState(UsbPeripheral::DeviceState::kWaitForFunctionBind);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, FunctionInterfaceClosedInHostConnectedState) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);

  // Connect host using the DCI interface.
  auto connected_res = this->dci()->SetConnected(true);
  ASSERT_TRUE(connected_res.ok()) << connected_res.FormatDescription();
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Simulate a function driver closing its interface.
  {
    function_clients.value().clients[0] = {};
  }

  // The peripheral driver should detect this, take the peripheral offline,
  // and drop back to kWaitForFunctionBind (even if host was connected).
  WaitUntilState(UsbPeripheral::DeviceState::kWaitForFunctionBind);

  // Check that the controller was stopped.
  ExpectControllerStarted(false);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, FunctionUnbindDuringTeardown) {
  usb_peripheral_config::Config config;
  config.functions() = {"test", "cdc"};
  StartDriverWithConfig(config);

  auto client = Client();

  auto function_clients = TransitionToPeripheralReady(2);
  ASSERT_OK(function_clients);

  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);

  // Trigger an unsolicited unbind for one function.
  SimulateFunctionUnbind({"function-000"});

  // Start ClearFunctions.
  auto clear_res = client->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

  WaitUntilState(UsbPeripheral::DeviceState::kNoConfiguration);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, FaultyFunctionInterfaceReset) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);

  // Faulty function driver closes its interface but doesn't unbind the node.
  {
    function_clients.value().clients[0] = {};
  }

  // The peripheral driver should detect this and drop back to kWaitForFunctionBind.
  WaitUntilState(UsbPeripheral::DeviceState::kWaitForFunctionBind);

  // Restore again by re-configuring.
  {
    auto peripheral_client = ConnectPeripheral();
    ASSERT_OK(peripheral_client);

    auto clear_res = peripheral_client.value()->ClearFunctions();
    ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

    fperipheral::wire::DeviceDescriptor device_desc = CreateTestDeviceDescriptor();

    fidl::Arena arena;
    auto configs = CreateTestFunctionDescriptors(arena);

    auto set_config_res = peripheral_client.value()->SetConfiguration(device_desc, configs);
    ASSERT_TRUE(set_config_res.ok()) << set_config_res.FormatDescription();
    ASSERT_TRUE(set_config_res->is_ok()) << zx_status_get_string(set_config_res->error_value());

    auto function_clients = TransitionToPeripheralReady();
    ASSERT_OK(function_clients);
  }
}

TEST_F(UnmanagedUsbPeripheralTest, PartialFunctionRegistration) {
  usb_peripheral_config::Config config;
  config.functions() = {"test", "cdc"};
  StartDriverWithConfig(config);

  // We should be in kWaitForFunctionBind because only 0/2 functions are registered.
  ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);

  // Bind one function.
  zx::result function_client = ConnectFunction();
  ASSERT_OK(function_client);

  // Still kWaitForFunctionBind (1/2 registered).
  ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);

  // Unbind that one function by closing the client.
  {
    function_client.value() = {};
  }

  // We can't wait for FunctionsCleared here because one function is still registered (virtually).
  // But we can wait for the state to be stable.
  this->dut().runtime().RunUntilIdle();
  ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);
}

TEST_F(UnmanagedUsbPeripheralTest, PrepareStopTransitionFromNoConfiguration) {
  // Driver started without config reaches kNoConfiguration.
  StartDriverWithConfig({});
  // TearDown will call StopDriver which will in turn call PrepareStop and verify controller is
  // stopped.
}

TEST_F(UnmanagedUsbPeripheralTest, PrepareStopTransitionFromWaitForFunctionBind) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);
  this->dut().runtime().RunUntilIdle();
  ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);

  // TearDown will call StopDriver which will in turn call PrepareStop and verify controller is
  // stopped.
}

TEST_F(UnmanagedUsbPeripheralTest, ClearFunctionsFromNoConfiguration) {
  StartDriverWithConfig(usb_peripheral_config::Config{});
  auto client = Client();
  auto clear_res = client->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

  this->dut().runtime().RunUntilIdle();
  ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);
}

TEST_F(UnmanagedUsbPeripheralTest, ClearFunctionsFromWaitForFunctionBind) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);
  this->dut().runtime().RunUntilIdle();
  ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);

  auto client = Client();
  auto clear_res = client->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

  this->dut().runtime().RunUntilIdle();
  ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);

  ExpectControllerStarted(false);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, ClearFunctionsFromPeripheralReady) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);
  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);
  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);

  auto client = Client();
  auto clear_res = client->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

  this->dut().runtime().RunUntilIdle();
  ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);

  ExpectControllerStarted(false);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, ClearFunctionsFromHostConnected) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);
  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);
  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);

  auto connected_res = this->dci()->SetConnected(true);
  ASSERT_TRUE(connected_res.ok()) << connected_res.FormatDescription();
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  auto client = Client();
  auto clear_res = client->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

  this->dut().runtime().RunUntilIdle();
  ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);

  ExpectControllerStarted(false);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, PrepareStopTransitionFromPeripheralReady) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);
  this->dut().runtime().RunUntilIdle();
  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);
  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);

  // TearDown will call StopDriver which will in turn call PrepareStop and verify controller is
  // stopped.
}

TEST_F(UnmanagedUsbPeripheralReadyTest, PrepareStopTransitionFromHostConnected) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);
  this->dut().runtime().RunUntilIdle();
  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);
  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);

  auto connected_res = this->dci()->SetConnected(true);
  ASSERT_TRUE(connected_res.ok()) << connected_res.FormatDescription();
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // TearDown will call StopDriver which will in turn call PrepareStop and verify controller is
  // stopped.
}

TEST_F(UnmanagedUsbPeripheralTest, PartialFunctionNodeUnbind) {
  usb_peripheral_config::Config config;
  config.functions() = {"test", "cdc"};
  StartDriverWithConfig(config);

  // We should be in kWaitForFunctionBind because only 0/2 functions are registered.
  ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);

  // Bind one function (by connecting its interface). State remains kWaitForFunctionBind.
  {
    zx::result function_client = ConnectFunction();
    ASSERT_OK(function_client);
  }
  ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);

  // Simulate one function provider (the child node) unbinding entirely.
  // This is different from just closing the UsbFunction interface.
  SimulateFunctionUnbind({"function-001"});

  // State should stay kWaitForFunctionBind.
  this->dut().runtime().RunUntilIdle();
  ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, ClearFunctionsAfterPartialFunctionUnbind) {
  usb_peripheral_config::Config config;
  config.functions() = {"test", "cdc"};
  StartDriverWithConfig(config);

  auto function_clients = TransitionToPeripheralReady(2);
  ASSERT_OK(function_clients);

  auto client = Client();

  // Simulate one function provider (the child node) unbinding.
  SimulateFunctionUnbind({"function-000"});

  // State should drop to kWaitForFunctionBind.
  WaitUntilState(UsbPeripheral::DeviceState::kWaitForFunctionBind);

  // Call ClearFunctions.
  auto clear_res = client->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

  WaitUntilState(UsbPeripheral::DeviceState::kNoConfiguration);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, FunctionNodeUnbindInHostConnectedState) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);

  // Connect host.
  auto connected_res = this->dci()->SetConnected(true);
  ASSERT_TRUE(connected_res.ok()) << connected_res.FormatDescription();
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Simulate function node unbind.
  SimulateFunctionUnbind({"function-000"});

  // Peripheral should go offline and wait for functions.
  WaitUntilState(UsbPeripheral::DeviceState::kWaitForFunctionBind);

  // Controller should be stopped.
  ExpectControllerStarted(false);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, CoordinatedCompositeTeardown) {
  // 1. Setup a multi-function composite device profile matching real watch configurations
  usb_peripheral_config::Config config;
  config.functions() = {"cdc", "test"};
  StartDriverWithConfig(config);

  auto function_clients = TransitionToPeripheralReady(2);
  ASSERT_OK(function_clients);

  // Connect host to transition to Host Connected state
  auto connected_res = this->dci()->SetConnected(true);
  ASSERT_TRUE(connected_res.ok()) << connected_res.FormatDescription();
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // 2. Simulate unsolicited unbind on the composite device (both function-000 and function-001)
  SimulateFunctionUnbind({"function-000", "function-001"});

  // 3. STATE AND TOPOLOGY VERIFICATION:
  // The parent must command RequestRemoval() on both nodes to sweep the layout cleanly.
  WaitUntilState(UsbPeripheral::DeviceState::kWaitForFunctionBind);
  ExpectControllerStarted(false);
  // Let mock framework process the node unbind events asynchronously on the loop.
  dut().runtime().RunUntilIdle();
}

TEST_F(UnmanagedUsbPeripheralReadyTest, FaultyFunctionNodeUnbindReset) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);

  auto fake_events = std::make_shared<FakeEvents>();
  this->RegisterFakeEvents(fake_events);

  // Simulate repeated node unbinds.
  for (int i = 0; i < 10; i++) {
    SimulateFunctionUnbind({"function-000"});

    WaitUntilState(UsbPeripheral::DeviceState::kWaitForFunctionBind);
    ExpectControllerStarted(false);
    WaitUntilChildNodeCount(0);

    // Restore again by re-configuring.
    {
      auto peripheral_client = ConnectPeripheral();
      ASSERT_OK(peripheral_client);

      auto clear_res = peripheral_client.value()->ClearFunctions();
      ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

      fperipheral::wire::DeviceDescriptor device_desc = CreateTestDeviceDescriptor();

      fidl::Arena arena;
      auto configs = CreateTestFunctionDescriptors(arena);

      auto set_config_res = peripheral_client.value()->SetConfiguration(device_desc, configs);
      ASSERT_TRUE(set_config_res.ok()) << set_config_res.FormatDescription();
      ASSERT_TRUE(set_config_res->is_ok()) << zx_status_get_string(set_config_res->error_value());

      auto function_clients = TransitionToPeripheralReady();
      ASSERT_OK(function_clients);
    }
  }

  fake_events->WaitUntilCleared(this->dut().runtime());
}

TEST_F(UnmanagedUsbPeripheralReadyTest, CheckAndStartControllerGuard) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);

  // Verify initial state.
  this->dut().RunInDriverContext([&](UsbPeripheral& peripheral) {
    EXPECT_EQ(peripheral.SnapshotState(), UsbPeripheral::DeviceState::kPeripheralReady);
  });

  // Call CheckAndStartController again.
  // This is to check for a very rare race condition where the function register calls occur in
  // parallel and the peripheral has already started the controller.
  // It's hard to simulate this race condition, so we just call CheckAndStartController again
  // to be safe.
  // This race condition would go away once we move to a single dispatcher.
  this->dut().RunInDriverContext([&](UsbPeripheral& peripheral) {
    // This should do nothing and return ZX_OK because state is not kWaitForFunctionBind.
    EXPECT_OK(peripheral.CheckAndStartController());
  });

  // Verify state is still kPeripheralReady.
  this->dut().RunInDriverContext([&](UsbPeripheral& peripheral) {
    EXPECT_EQ(peripheral.SnapshotState(), UsbPeripheral::DeviceState::kPeripheralReady);
  });
}

TEST_F(UnmanagedUsbPeripheralTest, UnconfiguredRequestTests) {
  StartDriverWithConfig(usb_peripheral_config::Config{});

  // 1. Test Control request with USB_RECIP_INTERFACE before configuration.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_INTERFACE,
        .b_request = USB_REQ_GET_STATUS,
        .w_value = 0,
        .w_index = 0,
        .w_length = 2,
    };

    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_error());
    EXPECT_EQ(res->error_value(), ZX_ERR_BAD_STATE);
  }

  // 1b. Test GET_STATUS for USB_RECIP_DEVICE before configuration.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_GET_STATUS,
        .w_value = 0,
        .w_index = 0,
        .w_length = 2,
    };

    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
    ASSERT_EQ(res->value()->read.size(), 2u);
    EXPECT_EQ(res->value()->read[0] & (1 << USB_DEVICE_SELF_POWERED), 1 << USB_DEVICE_SELF_POWERED);
    EXPECT_EQ(res->value()->read[1], 0);
  }

  // 1c. Test GET_STATUS for USB_RECIP_ENDPOINT (EP0 w_index=0) before configuration.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT,
        .b_request = USB_REQ_GET_STATUS,
        .w_value = 0,
        .w_index = 0,
        .w_length = 2,
    };

    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
    ASSERT_EQ(res->value()->read.size(), 2u);
    EXPECT_EQ(res->value()->read[0], 0);
    EXPECT_EQ(res->value()->read[1], 0);
  }

  // 1d. Test GET_STATUS for USB_RECIP_ENDPOINT (non-zero w_index=1) before configuration.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT,
        .b_request = USB_REQ_GET_STATUS,
        .w_value = 0,
        .w_index = 1,
        .w_length = 2,
    };

    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_error());
    EXPECT_EQ(res->error_value(), ZX_ERR_BAD_STATE);
  }

  // 1e. Test SET_FEATURE(USB_ENDPOINT_HALT) on non-zero w_index=1 before configuration.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT,
        .b_request = USB_REQ_SET_FEATURE,
        .w_value = USB_ENDPOINT_HALT,
        .w_index = 1,
        .w_length = 0,
    };

    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_error());
    EXPECT_EQ(res->error_value(), ZX_ERR_BAD_STATE);
  }

  // 1f. Test CLEAR_FEATURE(USB_ENDPOINT_HALT) on non-zero w_index=1 before configuration.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT,
        .b_request = USB_REQ_CLEAR_FEATURE,
        .w_value = USB_ENDPOINT_HALT,
        .w_index = 1,
        .w_length = 0,
    };

    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_error());
    EXPECT_EQ(res->error_value(), ZX_ERR_BAD_STATE);
  }

  // 2. Test SetInterface (via CommonControl) before configuration.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_INTERFACE,
        .b_request = USB_REQ_SET_INTERFACE,
        .w_value = 1,  // alt setting
        .w_index = 0,  // interface
        .w_length = 0,
    };

    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_error());
    EXPECT_EQ(res->error_value(), ZX_ERR_BAD_STATE);
  }

  // 3. Test SetConfiguration with invalid index (1) when 0 configs exist.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_SET_CONFIGURATION,
        .w_value = 1,  // config 1
        .w_index = 0,
        .w_length = 0,
    };

    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_error());
    EXPECT_EQ(res->error_value(), ZX_ERR_INVALID_ARGS);
  }

  // 4. Test SetConfiguration with 0 (unconfigure) when 0 configs exist.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_SET_CONFIGURATION,
        .w_value = 0,  // config 0
        .w_index = 0,
        .w_length = 0,
    };

    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
  }

  // 5. Test GET_DESCRIPTOR(USB_DT_BOS) behavior across USB versions on unconfigured device.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_GET_DESCRIPTOR,
        .w_value = USB_DT_BOS << 8,
        .w_index = 0,
        .w_length = sizeof(usb_bos_descriptor_t),
    };

    // Stalls for USB 2.0.
    this->dut().RunInDriverContext(
        [](UsbPeripheral& driver) { driver.SetBcdUsbForTesting(USB_2_0); });
    auto res_2_0 = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_2_0.ok()) << res_2_0.FormatDescription();
    ASSERT_TRUE(res_2_0->is_error());
    EXPECT_EQ(res_2_0->error_value(), ZX_ERR_NOT_SUPPORTED);

    // Succeeds for USB 2.1.
    this->dut().RunInDriverContext(
        [](UsbPeripheral& driver) { driver.SetBcdUsbForTesting(USB_2_1); });
    auto res_2_1 = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_2_1.ok()) << res_2_1.FormatDescription();
    ASSERT_TRUE(res_2_1->is_ok());

    // Succeeds for USB 3.1.
    this->dut().RunInDriverContext(
        [](UsbPeripheral& driver) { driver.SetBcdUsbForTesting(USB_3_1); });
    auto res_3_1 = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_3_1.ok()) << res_3_1.FormatDescription();
    ASSERT_TRUE(res_3_1->is_ok());

    // Non-zero descriptor index ((wValue & 0xFF) != 0) must stall.
    setup.w_value = (USB_DT_BOS << 8) | 1;
    setup.w_index = 0;
    auto res_inv_idx = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_inv_idx.ok()) << res_inv_idx.FormatDescription();
    ASSERT_TRUE(res_inv_idx->is_error());
    EXPECT_EQ(res_inv_idx->error_value(), ZX_ERR_NOT_SUPPORTED);

    // Non-zero w_index must stall.
    setup.w_value = USB_DT_BOS << 8;
    setup.w_index = 1;
    auto res_inv_widx = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_inv_widx.ok()) << res_inv_widx.FormatDescription();
    ASSERT_TRUE(res_inv_widx->is_error());
    EXPECT_EQ(res_inv_widx->error_value(), ZX_ERR_NOT_SUPPORTED);
  }
}

TEST_F(UnmanagedUsbPeripheralReadyTest, InvalidConfigurationTest) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);

  // Test SetConfiguration with invalid index (2) when 1 config exists.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_SET_CONFIGURATION,
        .w_value = 2,  // config 2 (invalid)
        .w_index = 0,
        .w_length = 0,
    };

    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_error());
    EXPECT_EQ(res->error_value(), ZX_ERR_INVALID_ARGS);
  }
}

TEST_F(UnmanagedUsbPeripheralReadyTest, ConfiguredGetStatusTests) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);

  // Transition to configured state (configuration 1).
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_SET_CONFIGURATION,
        .w_value = 1,
        .w_index = 0,
        .w_length = 0,
    };
    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
  }

  // 1. Test GET_STATUS for USB_RECIP_DEVICE in configured state.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_GET_STATUS,
        .w_value = 0,
        .w_index = 0,
        .w_length = 2,
    };
    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
    ASSERT_EQ(res->value()->read.size(), 2u);
    EXPECT_EQ(res->value()->read[0] & (1 << USB_DEVICE_SELF_POWERED), 1 << USB_DEVICE_SELF_POWERED);
    EXPECT_EQ(res->value()->read[1], 0);
  }

  // 2. Test GET_STATUS for USB_RECIP_INTERFACE (valid interface 0) in configured state.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_INTERFACE,
        .b_request = USB_REQ_GET_STATUS,
        .w_value = 0,
        .w_index = 0,  // interface 0
        .w_length = 2,
    };
    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
    ASSERT_EQ(res->value()->read.size(), 2u);
    EXPECT_EQ(res->value()->read[0], 0);
    EXPECT_EQ(res->value()->read[1], 0);
  }

  // 3. Test GET_STATUS for USB_RECIP_INTERFACE (invalid interface 5) in configured state.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_INTERFACE,
        .b_request = USB_REQ_GET_STATUS,
        .w_value = 0,
        .w_index = 5,  // out of range
        .w_length = 2,
    };
    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_error());
    EXPECT_EQ(res->error_value(), ZX_ERR_OUT_OF_RANGE);
  }

  // 4. Test GET_STATUS for USB_RECIP_ENDPOINT (EP0 control endpoint) in configured state.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT,
        .b_request = USB_REQ_GET_STATUS,
        .w_value = 0,
        .w_index = 0,  // EP0
        .w_length = 2,
    };
    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
    ASSERT_EQ(res->value()->read.size(), 2u);
    EXPECT_EQ(res->value()->read[0], 0);
    EXPECT_EQ(res->value()->read[1], 0);
  }

  // 5. Test GET_STATUS for USB_RECIP_ENDPOINT (valid EP 0x81) initially not halted.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT,
        .b_request = USB_REQ_GET_STATUS,
        .w_value = 0,
        .w_index = 0x81,
        .w_length = 2,
    };
    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
    ASSERT_EQ(res->value()->read.size(), 2u);
    EXPECT_EQ(res->value()->read[0], 0);
    EXPECT_EQ(res->value()->read[1], 0);
  }

  // 6. Test SET_FEATURE(USB_ENDPOINT_HALT) on EP 0x81 and verify GET_STATUS reflects stall.
  {
    fdescriptor::wire::UsbSetup set_halt = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT,
        .b_request = USB_REQ_SET_FEATURE,
        .w_value = USB_ENDPOINT_HALT,
        .w_index = 0x81,
        .w_length = 0,
    };
    auto res_set = dci()->Control(set_halt, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_set.ok()) << res_set.FormatDescription();
    ASSERT_TRUE(res_set->is_ok());

    fdescriptor::wire::UsbSetup get_status = {
        .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT,
        .b_request = USB_REQ_GET_STATUS,
        .w_value = 0,
        .w_index = 0x81,
        .w_length = 2,
    };
    auto res_get = dci()->Control(get_status, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_get.ok()) << res_get.FormatDescription();
    ASSERT_TRUE(res_get->is_ok());
    ASSERT_EQ(res_get->value()->read.size(), 2u);
    EXPECT_EQ(res_get->value()->read[0], 1);  // halted
    EXPECT_EQ(res_get->value()->read[1], 0);

    // Clear feature back to not halted.
    fdescriptor::wire::UsbSetup clear_halt = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT,
        .b_request = USB_REQ_CLEAR_FEATURE,
        .w_value = USB_ENDPOINT_HALT,
        .w_index = 0x81,
        .w_length = 0,
    };
    auto res_clear = dci()->Control(clear_halt, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_clear.ok()) << res_clear.FormatDescription();
    ASSERT_TRUE(res_clear->is_ok());

    auto res_get2 = dci()->Control(get_status, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_get2.ok()) << res_get2.FormatDescription();
    ASSERT_TRUE(res_get2->is_ok());
    ASSERT_EQ(res_get2->value()->read.size(), 2u);
    EXPECT_EQ(res_get2->value()->read[0], 0);  // cleared
    EXPECT_EQ(res_get2->value()->read[1], 0);
  }
}

TEST_F(UnmanagedUsbPeripheralReadyTest, SetConfigurationClearsEndpointHalt) {
  // Validates that calling SET_CONFIGURATION resets the halt status of all endpoints
  // to not halted, as mandated by USB 2.0 Specification Section 9.4.7:
  // "The SetConfiguration() request... causes the halt status of each endpoint to be
  // reset to not halted".
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);

  // Transition to configured state (configuration 1).
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_SET_CONFIGURATION,
        .w_value = 1,
        .w_index = 0,
        .w_length = 0,
    };
    fidl::Arena arena;
    auto res = dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
  }

  // Set endpoint halt on EP 0x81.
  {
    fdescriptor::wire::UsbSetup set_halt = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT,
        .b_request = USB_REQ_SET_FEATURE,
        .w_value = USB_ENDPOINT_HALT,
        .w_index = 0x81,
        .w_length = 0,
    };
    fidl::Arena arena;
    auto res_set = dci().buffer(arena)->Control(set_halt, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_set.ok()) << res_set.FormatDescription();
    ASSERT_TRUE(res_set->is_ok());
  }

  // Verify endpoint 0x81 is halted via GET_STATUS.
  fdescriptor::wire::UsbSetup get_status = {
      .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT,
      .b_request = USB_REQ_GET_STATUS,
      .w_value = 0,
      .w_index = 0x81,
      .w_length = 2,
  };
  {
    fidl::Arena arena;
    auto res_get = dci().buffer(arena)->Control(get_status, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_get.ok()) << res_get.FormatDescription();
    ASSERT_TRUE(res_get->is_ok());
    ASSERT_EQ(res_get->value()->read.size(), 2u);
    EXPECT_EQ(res_get->value()->read[0], 1);  // halted
    EXPECT_EQ(res_get->value()->read[1], 0);
  }

  // Re-issue SET_CONFIGURATION (configuration 1).
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_SET_CONFIGURATION,
        .w_value = 1,
        .w_index = 0,
        .w_length = 0,
    };
    fidl::Arena arena;
    auto res = dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
  }

  // Verify endpoint halt was cleared by SET_CONFIGURATION per USB 2.0 § 9.4.7.
  {
    fidl::Arena arena;
    auto res_get = dci().buffer(arena)->Control(get_status, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_get.ok()) << res_get.FormatDescription();
    ASSERT_TRUE(res_get->is_ok());
    ASSERT_EQ(res_get->value()->read.size(), 2u);
    EXPECT_EQ(res_get->value()->read[0], 0);  // halt must be cleared
    EXPECT_EQ(res_get->value()->read[1], 0);
  }
}

TEST_F(UnmanagedUsbPeripheralReadyTest, GetInterfaceReturnsCurrentAlternateSetting) {
  // Validates standard GET_INTERFACE (bRequest = 0x0A) request for an active interface
  // per USB 2.0 Specification Section 9.4.4. A device must return a 1-byte value specifying
  // the current alternate setting for the specified interface.
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);

  // Transition to configured state.
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_SET_CONFIGURATION,
        .w_value = 1,
        .w_index = 0,
        .w_length = 0,
    };
    fidl::Arena arena;
    auto res = dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
  }

  fdescriptor::wire::UsbSetup setup = {
      .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_INTERFACE,
      .b_request = USB_REQ_GET_INTERFACE,
      .w_value = 0,
      .w_index = 0,  // Interface 0
      .w_length = 1,
  };
  fidl::Arena arena;
  auto result = dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>());
  ASSERT_TRUE(result.ok()) << result.FormatDescription();
  ASSERT_TRUE(result->is_ok());
  auto& resp = result->value()->read;
  ASSERT_EQ(resp.size(), 1u);
  EXPECT_EQ(resp[0], 0);  // Alternate setting 0
}

TEST_F(UnmanagedUsbPeripheralReadyTest, SetInterfaceClearsEndpointHalt) {
  // Validates standard SET_INTERFACE (bRequest = 0x0B) request for an interface
  // per USB 2.0 Specification Section 9.4.5:
  // "A SetInterface request for an interface resets the halt status of each endpoint
  // associated with that interface to not halted."
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients = TransitionToPeripheralReady();
  ASSERT_OK(function_clients);

  // Transition to configured state (configuration 1).
  {
    fdescriptor::wire::UsbSetup setup = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
        .b_request = USB_REQ_SET_CONFIGURATION,
        .w_value = 1,
        .w_index = 0,
        .w_length = 0,
    };
    auto res = dci()->Control(setup, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res->is_ok());
  }

  // Set endpoint halt on EP 0x81 (allocated to interface 0).
  {
    fdescriptor::wire::UsbSetup set_halt = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT,
        .b_request = USB_REQ_SET_FEATURE,
        .w_value = USB_ENDPOINT_HALT,
        .w_index = 0x81,
        .w_length = 0,
    };
    auto res_set = dci()->Control(set_halt, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_set.ok()) << res_set.FormatDescription();
    ASSERT_TRUE(res_set->is_ok());
  }

  // Verify endpoint 0x81 is halted via GET_STATUS.
  fdescriptor::wire::UsbSetup get_status = {
      .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT,
      .b_request = USB_REQ_GET_STATUS,
      .w_value = 0,
      .w_index = 0x81,
      .w_length = 2,
  };
  {
    auto res_get = dci()->Control(get_status, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_get.ok()) << res_get.FormatDescription();
    ASSERT_TRUE(res_get->is_ok());
    ASSERT_EQ(res_get->value()->read.size(), 2u);
    EXPECT_EQ(res_get->value()->read[0], 1);  // halted
    EXPECT_EQ(res_get->value()->read[1], 0);
  }

  // Issue SET_INTERFACE for Interface 0, Alternate Setting 0.
  {
    fdescriptor::wire::UsbSetup set_iface = {
        .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_INTERFACE,
        .b_request = USB_REQ_SET_INTERFACE,
        .w_value = 0,  // Alternate setting 0
        .w_index = 0,  // Interface 0
        .w_length = 0,
    };
    auto res_iface = dci()->Control(set_iface, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_iface.ok()) << res_iface.FormatDescription();
    ASSERT_TRUE(res_iface->is_ok());
  }

  // Verify endpoint halt was cleared by SET_INTERFACE per USB 2.0 § 9.4.5.
  {
    auto res_get = dci()->Control(get_status, fidl::VectorView<uint8_t>());
    ASSERT_TRUE(res_get.ok()) << res_get.FormatDescription();
    ASSERT_TRUE(res_get->is_ok());
    ASSERT_EQ(res_get->value()->read.size(), 2u);
    EXPECT_EQ(res_get->value()->read[0], 0);  // halt must be cleared
    EXPECT_EQ(res_get->value()->read[1], 0);
  }

  // Verify that mock DCI EndpointClearStall was invoked for hardware halt clearing.
  this->dut().RunInEnvironmentTypeContext([](UsbPeripheralTestEnvironment& env) {
    const auto& clear_stalls = env.dci().clear_stalls();
    EXPECT_NE(std::find(clear_stalls.begin(), clear_stalls.end(), 0x81), clear_stalls.end());
  });
}

TEST_F(UnmanagedUsbPeripheralReadyTest, StartControllerFailsFromDci) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  // Tell DCI mock to fail StartController.
  this->dut().RunInEnvironmentTypeContext(
      [](UsbPeripheralTestEnvironment& env) { env.dci().fail_start_.store(true); });

  auto res = TransitionToPeripheralReady();
  ASSERT_TRUE(res.is_error());
  EXPECT_STATUS(res.error_value(), ZX_ERR_IO_NOT_PRESENT);

  // Verify state stays at kWaitForFunctionBind since configuration failed to start the controller.
  ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);
  ExpectControllerStarted(false);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, StopControllerFailsFromDci) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  // Successfully configure and transition to peripheral ready first.
  auto res = TransitionToPeripheralReady();
  ASSERT_OK(res);

  // Tell DCI mock to fail StopController.
  this->dut().RunInEnvironmentTypeContext(
      [](UsbPeripheralTestEnvironment& env) { env.dci().fail_stop_.store(true); });

  // Call ClearFunctions. Even though StopController fails, it should still succeed
  // and teardown the functions to kNoConfiguration state.
  auto client = this->Client();

  zx::result endpoints = fidl::CreateEndpoints<fperipheral::Events>();
  ASSERT_OK(endpoints);

  FakeEvents fake_events;
  fake_events.Bind(std::move(endpoints->server));

  auto set_listener_res = client->SetStateChangeListener(std::move(endpoints->client));
  ASSERT_TRUE(set_listener_res.ok()) << set_listener_res.FormatDescription();

  auto clear_res = client->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

  fake_events.WaitUntilCleared(this->dut().runtime());
  fake_events.Unbind();

  // Verify we successfully cleaned up and reached kNoConfiguration.
  ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);
  // DCI stop failed, so from fake DCI's point of view, it is still started.
  ExpectControllerStarted(true);

  // Reset failure flag and manually set controller_started to false to allow clean teardown in
  // TearDown()
  this->dut().RunInEnvironmentTypeContext([](UsbPeripheralTestEnvironment& env) {
    env.dci().fail_stop_.store(false);
    env.dci().set_controller_started(false);
  });
}

TEST_F(UsbPeripheralReadyTest, ActiveFunctionsCountTracking) {
  // Initially in kPeripheralReady with 1 function from SetUp().
  size_t active_count = dut().RunInDriverContext<size_t>([](UsbPeripheral& peripheral) {
    return UsbPeripheralTestHelper::active_functions_count(peripheral);
  });
  EXPECT_EQ(1u, active_count);

  // Simulate function node unbind.
  SimulateFunctionUnbind({"function-000"});
  WaitUntilChildNodeCount(0);
  WaitUntilState(UsbPeripheral::DeviceState::kWaitForFunctionBind);

  active_count = dut().RunInDriverContext<size_t>([](UsbPeripheral& peripheral) {
    return UsbPeripheralTestHelper::active_functions_count(peripheral);
  });
  EXPECT_EQ(0u, active_count);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, LateFunctionClearedCallbackIgnoredByGenerationFence) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  // Generation 0 Setup (initial configuration)
  auto function_clients_res = TransitionToPeripheralReady();
  ASSERT_OK(function_clients_res);

  // Verify that active functions count is 1 for Generation 0.
  size_t active_count = dut().RunInDriverContext<size_t>([](UsbPeripheral& peripheral) {
    return UsbPeripheralTestHelper::active_functions_count(peripheral);
  });
  EXPECT_EQ(1u, active_count);

  auto peripheral_client = ConnectPeripheral();
  ASSERT_OK(peripheral_client);

  // Clear functions to trigger teardown.
  auto clear_res = peripheral_client.value()->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();
  dut().runtime().RunUntilIdle();

  // Generation 1 Setup: Re-configure the device to increment config_generation to 1.
  fperipheral::wire::DeviceDescriptor device_desc = CreateTestDeviceDescriptor();
  fidl::Arena arena;
  auto configs = CreateTestFunctionDescriptors(arena);
  auto set_config_res = peripheral_client.value()->SetConfiguration(device_desc, configs);
  ASSERT_TRUE(set_config_res.ok());

  auto function_clients_gen1 = TransitionToPeripheralReady();
  ASSERT_OK(function_clients_gen1);

  // Verify that active functions count is 1 for Generation 1.
  active_count = dut().RunInDriverContext<size_t>([](UsbPeripheral& peripheral) {
    return UsbPeripheralTestHelper::active_functions_count(peripheral);
  });
  EXPECT_EQ(1u, active_count);

  // Direct injection of stale unbind callbacks stamped with Generation 0.
  dut().RunInDriverContext([](UsbPeripheral& peripheral) {
    // Generation 0 is stale (the current generation is 1).
    UsbPeripheralTestHelper::DecrementActiveFunctions(peripheral, 0);
    UsbPeripheralTestHelper::FunctionCleared(peripheral, 0, 0);
  });

  dut().runtime().RunUntilIdle();

  // Assertion: The driver must securely ignore the stale callback.
  active_count = dut().RunInDriverContext<size_t>([](UsbPeripheral& peripheral) {
    return UsbPeripheralTestHelper::active_functions_count(peripheral);
  });
  EXPECT_EQ(1u, active_count);

  // Ensure we didn't accidentally tear down or regress state.
  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, HostDisconnectPowerCutRaceTrap) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients_res = TransitionToPeripheralReady();
  ASSERT_OK(function_clients_res);
  auto function_clients =
      std::make_shared<FunctionClients>(std::move(function_clients_res.value()));

  // The mock function driver in TransitionToPeripheralReady allocates OUT endpoint 1 and IN
  // endpoint 0x81.
  uint8_t ep_addr_out = 1;
  uint8_t ep_addr_in = 0x81;

  // Establish active connection.
  auto connected_res = this->dci()->SetConnected(true);
  ASSERT_TRUE(connected_res.ok());
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  std::mutex completer_lock;
  std::optional<FakeUsbFunction::SetConfiguredCompleterAsync> deferred_completer;
  auto completer_cleanup = fit::defer([&]() {
    std::optional<FakeUsbFunction::SetConfiguredCompleterAsync> completer_to_run;
    {
      std::lock_guard lock(completer_lock);
      completer_to_run = std::move(deferred_completer);
      deferred_completer.reset();
    }
    if (completer_to_run.has_value()) {
      completer_to_run->ReplySuccess();
    }
  });
  auto callback_cleanup =
      fit::defer([&]() { function_clients->fakes[0]->set_on_set_configured_async(nullptr); });

  // Intercept SetConfigured(false) on fake function driver and trigger endpoint disable.
  function_clients->fakes[0]->set_on_set_configured_async(
      [&](bool configured, FakeUsbFunction::SetConfiguredCompleterAsync completer) {
        if (!configured) {
          auto res_out = function_clients->clients[0]->DisableEndpoint(ep_addr_out);
          EXPECT_TRUE(res_out.ok());
          if (res_out.ok()) {
            EXPECT_TRUE(res_out->is_ok())
                << "DisableEndpoint OUT failed: " << zx_status_get_string(res_out->error_value());
          }
          auto res_in = function_clients->clients[0]->DisableEndpoint(ep_addr_in);
          EXPECT_TRUE(res_in.ok());
          if (res_in.ok()) {
            EXPECT_TRUE(res_in->is_ok())
                << "DisableEndpoint IN failed: " << zx_status_get_string(res_in->error_value());
          }
          std::lock_guard lock(completer_lock);
          deferred_completer.emplace(std::move(completer));
        } else {
          completer.ReplySuccess();
        }
      });

  // Trigger host disconnect.
  auto disconnected_res = this->dci()->SetConnected(false);
  ASSERT_TRUE(disconnected_res.ok());

  // Wait for the unconfiguration callback to be invoked and disable endpoints.
  this->dut().runtime().RunUntil([&]() {
    std::lock_guard lock(completer_lock);
    return deferred_completer.has_value();
  });

  // Allow any pending dispatcher tasks to settle.
  this->dut().runtime().RunUntilIdle();

  // Verify that endpoints were disabled on DCI.
  std::vector<uint8_t> disabled_endpoints;
  this->dut().RunInEnvironmentTypeContext([&](UsbPeripheralTestEnvironment& env) {
    disabled_endpoints = env.dci().disabled_endpoints();
  });
  EXPECT_EQ(disabled_endpoints.size(), 2u);

  // Assertion: The peripheral state MUST remain kHostConnected while unconfigure is in-flight,
  // preventing power cuts before endpoints are disabled.
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Complete the unconfigure operation.
  completer_cleanup.call();

  // Let the async promise join chain finish.
  this->dut().runtime().RunUntilIdle();

  // Verify the driver successfully transitioned back to kPeripheralReady after async teardown.
  WaitUntilState(UsbPeripheral::DeviceState::kPeripheralReady);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, AsynchronousUnconfigureTeardownCompleter) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients_res = TransitionToPeripheralReady();
  ASSERT_OK(function_clients_res);
  auto fake_function = function_clients_res.value().fakes[0];

  // Simulate active VBUS cable plug to transition state to kHostConnected.
  ASSERT_OK(dci()->SetConnected(true).status());
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Defer SetConfigured(false) completion to simulate slow network flushes.
  std::mutex completer_lock;
  std::optional<FakeUsbFunction::SetConfiguredCompleterAsync> deferred_completer;
  auto callback_cleanup =
      fit::defer([&]() { fake_function->set_on_set_configured_async(nullptr); });
  fake_function->set_on_set_configured_async(
      [&](bool configured, FakeUsbFunction::SetConfiguredCompleterAsync completer) {
        if (!configured) {
          std::lock_guard lock(completer_lock);
          deferred_completer.emplace(std::move(completer));
        } else {
          completer.ReplySuccess();
        }
      });

  auto peripheral_client = ConnectPeripheral();
  ASSERT_OK(peripheral_client);

  std::atomic<bool> clear_finished = false;
  // Clear functions asynchronously on a background thread. This is necessary because
  // ClearFunctions() is a synchronous FIDL call that blocks waiting for SetConfigured(false)
  // to complete, which is held open by deferred_completer. If called on the main test thread,
  // the test would deadlock before it could assert the in-flight state or reply to the completer.
  auto clear_promise = std::async(std::launch::async, [&]() {
    auto clear_res = peripheral_client.value()->ClearFunctions();
    EXPECT_TRUE(clear_res.ok()) << clear_res.FormatDescription();
    clear_finished.store(true);
  });
  // completer_cleanup is declared after clear_promise so that upon test failure or exception,
  // its destructor runs before ~future() blocks waiting on ClearFunctions().
  auto completer_cleanup = fit::defer([&]() {
    std::optional<FakeUsbFunction::SetConfiguredCompleterAsync> completer_to_run;
    {
      std::lock_guard lock(completer_lock);
      completer_to_run = std::move(deferred_completer);
      deferred_completer.reset();
    }
    if (completer_to_run.has_value()) {
      completer_to_run->ReplySuccess();
    }
  });

  // Allow the dispatcher to process the initial ClearFunctions task.
  dut().runtime().RunUntilIdle();

  // Block until the background thread fully finishes the SetConfigured(false) callback.
  dut().runtime().RunUntil([&]() {
    std::lock_guard lock(completer_lock);
    return deferred_completer.has_value();
  });

  // Assert that ClearFunctions remains blocked and the hardware controller is kept live
  // while the SetConfigured(false) completer is outstanding.
  EXPECT_FALSE(clear_finished.load());
  dut().RunInEnvironmentTypeContext(
      [](UsbPeripheralTestEnvironment& env) { EXPECT_TRUE(env.dci().controller_started()); });

  // Signal logical teardown completion.
  completer_cleanup.call();

  // Process the completion reply and execute hardware shutdown.
  dut().runtime().RunUntilIdle();
  dut().runtime().RunUntil([&]() {
    return dut().RunInEnvironmentTypeContext<bool>(
        [](UsbPeripheralTestEnvironment& env) { return !env.dci().controller_started(); });
  });

  // Assert that ClearFunctions completed and the hardware controller stopped.
  clear_promise.get();
  EXPECT_TRUE(clear_finished);
  dut().RunInEnvironmentTypeContext(
      [](UsbPeripheralTestEnvironment& env) { EXPECT_FALSE(env.dci().controller_started()); });
}

TEST_F(UnmanagedUsbPeripheralReadyTest, HostReconnectDuringAsyncUnconfigure) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients_res = TransitionToPeripheralReady();
  ASSERT_OK(function_clients_res);
  auto fake_function = function_clients_res.value().fakes[0];

  // Simulate active VBUS cable plug to transition state to kHostConnected.
  ASSERT_OK(dci()->SetConnected(true).status());
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Defer SetConfigured(false) completion to simulate delayed mock teardown.
  std::mutex completer_lock;
  std::optional<FakeUsbFunction::SetConfiguredCompleterAsync> deferred_completer;
  auto completer_cleanup = fit::defer([&]() {
    std::optional<FakeUsbFunction::SetConfiguredCompleterAsync> completer_to_run;
    {
      std::lock_guard lock(completer_lock);
      completer_to_run = std::move(deferred_completer);
      deferred_completer.reset();
    }
    if (completer_to_run.has_value()) {
      completer_to_run->ReplySuccess();
    }
  });
  auto callback_cleanup =
      fit::defer([&]() { fake_function->set_on_set_configured_async(nullptr); });
  fake_function->set_on_set_configured_async(
      [&](bool configured, FakeUsbFunction::SetConfiguredCompleterAsync completer) {
        if (!configured) {
          std::lock_guard lock(completer_lock);
          deferred_completer.emplace(std::move(completer));
        } else {
          completer.ReplySuccess();
        }
      });

  // Trigger a disconnect via DCI to begin the async teardown.
  ASSERT_OK(dci()->SetConnected(false).status());

  // Allow the dispatcher to process the SetConnected(false) task and wait for
  // SetConfigured(false) to arrive at the fake function.
  dut().runtime().RunUntil([&]() {
    std::lock_guard lock(completer_lock);
    return deferred_completer.has_value();
  });

  // Assertion: The state should still be kHostConnected because the unconfigure
  // promise is currently blocked waiting for the completer.
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Before resolving the completer, simulate a rapid cable bounce by reconnecting.
  ASSERT_OK(dci()->SetConnected(true).status());
  dut().runtime().RunUntilIdle();

  // Now resolve the captured completer to allow the async promise join task to finish.
  completer_cleanup.call();

  // Run the dispatcher until idle to flush the promise join chain.
  dut().runtime().RunUntilIdle();

  // Final Assertion: Verify that the state machine safely caught the reconnection.
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);
}

TEST_F(UnmanagedUsbPeripheralReadyTest,
       RapidDisconnectReconnectDisconnectMaintainsHostConnectedUntilAllPromisesComplete) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients_res = TransitionToPeripheralReady();
  ASSERT_OK(function_clients_res);

  auto fake_function = function_clients_res.value().fakes[0];
  ASSERT_NE(nullptr, fake_function);

  // Connect host to transition to kHostConnected.
  ASSERT_OK(dci()->SetConnected(true).status());
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Track all in-flight SetConfigured(false) completers in a vector to ensure no
  // completer is dropped without replying if multiple disconnect events arrive.
  std::mutex completers_lock;
  std::vector<FakeUsbFunction::SetConfiguredCompleterAsync> deferred_completers;
  auto completers_cleanup = fit::defer([&]() {
    std::vector<FakeUsbFunction::SetConfiguredCompleterAsync> completers_to_run;
    {
      std::lock_guard lock(completers_lock);
      completers_to_run = std::move(deferred_completers);
      deferred_completers.clear();
    }
    for (auto& completer : completers_to_run) {
      completer.ReplySuccess();
    }
  });
  auto callback_cleanup =
      fit::defer([&]() { fake_function->set_on_set_configured_async(nullptr); });
  fake_function->set_on_set_configured_async(
      [&](bool configured, FakeUsbFunction::SetConfiguredCompleterAsync completer) {
        if (!configured) {
          std::lock_guard lock(completers_lock);
          deferred_completers.push_back(std::move(completer));
        } else {
          completer.ReplySuccess();
        }
      });

  // Disconnect 1: Starts async unconfigure task 1.
  ASSERT_OK(dci()->SetConnected(false).status());
  dut().runtime().RunUntil([&]() {
    std::lock_guard lock(completers_lock);
    return !deferred_completers.empty();
  });
  {
    std::lock_guard lock(completers_lock);
    ASSERT_EQ(deferred_completers.size(), 1u);
  }
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Rapid Reconnect: host connects again.
  ASSERT_OK(dci()->SetConnected(true).status());
  dut().runtime().RunUntilIdle();
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Disconnect 2: Rapid bounce back to disconnected. Starts async unconfigure task 2.
  // Note: UsbFunction is already in unconfigured state from Disconnect 1, so it completes
  // immediately without invoking fake_function a second time.
  ASSERT_OK(dci()->SetConnected(false).status());
  dut().runtime().RunUntilIdle();

  // Crucial invariant: Disconnect 1's unconfigure promise is STILL in flight on the
  // function driver. Even though Disconnect 2 occurred and connected_ is false,
  // the driver state MUST remain kHostConnected until all promises finish!
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Now resolve Disconnect 1's completer.
  completers_cleanup.call();
  dut().runtime().RunUntilIdle();

  // All in-flight unconfigures are now finished. The driver safely transitions to kPeripheralReady.
  ExpectStateEventual(UsbPeripheral::DeviceState::kPeripheralReady);
}

TEST_F(UnmanagedUsbPeripheralReadyTest, HostReconnectSetConfigurationDeferredDuringDisconnect) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients_res = TransitionToPeripheralReady();
  ASSERT_OK(function_clients_res);
  auto fake_function = function_clients_res.value().fakes[0];
  ASSERT_NE(nullptr, fake_function);

  // Connect host to transition to kHostConnected.
  ASSERT_OK(dci()->SetConnected(true).status());
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Configure first to set up active configuration 1.
  fdescriptor::wire::UsbSetup set_config = {
      .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
      .b_request = USB_REQ_SET_CONFIGURATION,
      .w_value = 1,
      .w_index = 0,
      .w_length = 0,
  };
  auto set_res = dci()->Control(set_config, fidl::VectorView<uint8_t>());
  ASSERT_TRUE(set_res.ok()) << set_res.FormatDescription();
  ASSERT_TRUE(set_res->is_ok());

  // Intercept SetConfigured(false) to hold disconnect unconfiguration in-flight.
  std::mutex completer_lock;
  std::optional<FakeUsbFunction::SetConfiguredCompleterAsync> deferred_unconfigure;
  auto callback_cleanup =
      fit::defer([&]() { fake_function->set_on_set_configured_async(nullptr); });
  fake_function->set_on_set_configured_async(
      [&](bool configured, FakeUsbFunction::SetConfiguredCompleterAsync completer) {
        if (!configured) {
          std::lock_guard lock(completer_lock);
          deferred_unconfigure.emplace(std::move(completer));
        } else {
          completer.ReplySuccess();
        }
      });

  // Host disconnects -> begins asynchronous unconfiguration.
  ASSERT_OK(dci()->SetConnected(false).status());
  dut().runtime().RunUntil([&]() {
    std::lock_guard lock(completer_lock);
    return deferred_unconfigure.has_value();
  });

  // Host rapidly reconnects.
  ASSERT_OK(dci()->SetConnected(true).status());
  dut().runtime().RunUntilIdle();

  // Host issues SET_CONFIGURATION(1) asynchronously via Control request.
  // Because disconnect unconfigure is in-flight, this should be deferred until unconfigure
  // completes.
  std::atomic<bool> configure_finished = false;
  auto configure_future = std::async(std::launch::async, [&]() {
    auto res = dci()->Control(set_config, fidl::VectorView<uint8_t>());
    EXPECT_TRUE(res.ok()) << res.FormatDescription();
    EXPECT_TRUE(res->is_ok());
    configure_finished.store(true);
  });
  // completer_cleanup is declared after configure_future so that upon test failure or exception,
  // its destructor runs before ~future() blocks waiting on Control().
  auto completer_cleanup = fit::defer([&]() {
    std::optional<FakeUsbFunction::SetConfiguredCompleterAsync> completer_to_run;
    {
      std::lock_guard lock(completer_lock);
      completer_to_run = std::move(deferred_unconfigure);
      deferred_unconfigure.reset();
    }
    if (completer_to_run.has_value()) {
      completer_to_run->ReplySuccess();
    }
  });

  // Wait until the driver receives and registers the deferred SET_CONFIGURATION.
  dut().runtime().RunUntil([&]() {
    bool pending = false;
    dut().RunInDriverContext(
        [&](UsbPeripheral& peripheral) { pending = peripheral.HasPendingSetConfiguration(); });
    return pending;
  });

  // While deferred_unconfigure is outstanding, SET_CONFIGURATION must NOT complete yet.
  EXPECT_FALSE(configure_finished.load());

  // Resolve the disconnect unconfigure completer.
  completer_cleanup.call();
  dut().runtime().RunUntilIdle();

  // Now the deferred SET_CONFIGURATION completes and configures the functions cleanly.
  configure_future.get();
  EXPECT_TRUE(configure_finished.load());

  // Verify configuration is 1.
  fdescriptor::wire::UsbSetup get_config = {
      .bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
      .b_request = USB_REQ_GET_CONFIGURATION,
      .w_value = 0,
      .w_index = 0,
      .w_length = 1,
  };
  auto get_res = dci()->Control(get_config, fidl::VectorView<uint8_t>());
  ASSERT_TRUE(get_res.ok()) << get_res.FormatDescription();
  ASSERT_TRUE(get_res->is_ok());
  ASSERT_EQ(get_res->value()->read.size(), 1u);
  EXPECT_EQ(get_res->value()->read[0], 1);
}

TEST_F(UnmanagedUsbPeripheralReadyTest,
       HostDisconnectWhileSetConfigurationDeferredCancelsCompleter) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients_res = TransitionToPeripheralReady();
  ASSERT_OK(function_clients_res);
  auto fake_function = function_clients_res.value().fakes[0];
  ASSERT_NE(nullptr, fake_function);

  // Connect host to transition to kHostConnected.
  ASSERT_OK(dci()->SetConnected(true).status());
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Configure first to set up active configuration 1.
  fdescriptor::wire::UsbSetup set_config = {
      .bm_request_type = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE,
      .b_request = USB_REQ_SET_CONFIGURATION,
      .w_value = 1,
      .w_index = 0,
      .w_length = 0,
  };
  auto set_res = dci()->Control(set_config, fidl::VectorView<uint8_t>());
  ASSERT_TRUE(set_res.ok()) << set_res.FormatDescription();
  ASSERT_TRUE(set_res->is_ok());

  // Intercept SetConfigured(false) to hold disconnect unconfiguration in-flight.
  std::mutex completers_lock;
  std::vector<FakeUsbFunction::SetConfiguredCompleterAsync> deferred_unconfigures;
  auto callback_cleanup =
      fit::defer([&]() { fake_function->set_on_set_configured_async(nullptr); });
  fake_function->set_on_set_configured_async(
      [&](bool configured, FakeUsbFunction::SetConfiguredCompleterAsync completer) {
        if (!configured) {
          std::lock_guard lock(completers_lock);
          deferred_unconfigures.push_back(std::move(completer));
        } else {
          completer.ReplySuccess();
        }
      });

  // Host disconnects -> begins asynchronous unconfiguration.
  ASSERT_OK(dci()->SetConnected(false).status());
  dut().runtime().RunUntil([&]() {
    std::lock_guard lock(completers_lock);
    return !deferred_unconfigures.empty();
  });

  // Host rapidly reconnects.
  ASSERT_OK(dci()->SetConnected(true).status());
  dut().runtime().RunUntilIdle();

  // Host issues SET_CONFIGURATION(1) asynchronously via Control request.
  // Because disconnect unconfigure is in-flight, this will be deferred into
  // pending_set_configuration_.
  std::atomic<bool> configure_finished = false;
  zx_status_t configure_error = ZX_OK;
  auto configure_future = std::async(std::launch::async, [&]() {
    auto res = dci()->Control(set_config, fidl::VectorView<uint8_t>());
    EXPECT_TRUE(res.ok()) << res.FormatDescription();
    if (res.ok() && res->is_error()) {
      configure_error = res->error_value();
    }
    configure_finished.store(true);
  });
  auto completer_cleanup = fit::defer([&]() {
    std::vector<FakeUsbFunction::SetConfiguredCompleterAsync> completers_to_run;
    {
      std::lock_guard lock(completers_lock);
      completers_to_run = std::move(deferred_unconfigures);
      deferred_unconfigures.clear();
    }
    for (auto& comp : completers_to_run) {
      comp.ReplySuccess();
    }
  });

  // Wait until the driver receives and registers the deferred SET_CONFIGURATION.
  dut().runtime().RunUntil([&]() {
    bool pending = false;
    dut().RunInDriverContext(
        [&](UsbPeripheral& peripheral) { pending = peripheral.HasPendingSetConfiguration(); });
    return pending;
  });

  EXPECT_FALSE(configure_finished.load());

  // Host disconnects again before the first unconfigure finishes.
  ASSERT_OK(dci()->SetConnected(false).status());
  dut().runtime().RunUntilIdle();

  // The deferred SET_CONFIGURATION must be canceled with ZX_ERR_CANCELED without hanging.
  configure_future.get();
  EXPECT_TRUE(configure_finished.load());
  EXPECT_EQ(configure_error, ZX_ERR_CANCELED);

  // Complete all in-flight unconfigures.
  completer_cleanup.call();
  ExpectStateEventual(UsbPeripheral::DeviceState::kPeripheralReady);
}

TEST_F(UnmanagedUsbPeripheralReadyTest,
       RapidClearFunctionsWhileTeardownInFlightDoesNotCompleteEarly) {
  usb_peripheral_config::Config config;
  config.functions() = {"test"};
  StartDriverWithConfig(config);

  auto function_clients_res = TransitionToPeripheralReady();
  ASSERT_OK(function_clients_res);
  auto fake_function = function_clients_res.value().fakes[0];
  ASSERT_NE(nullptr, fake_function);

  std::mutex completer_lock;
  std::optional<FakeUsbFunction::SetConfiguredCompleterAsync> deferred_completer;
  auto callback_cleanup =
      fit::defer([&]() { fake_function->set_on_set_configured_async(nullptr); });
  fake_function->set_on_set_configured_async(
      [&](bool configured, FakeUsbFunction::SetConfiguredCompleterAsync completer) {
        if (!configured) {
          std::lock_guard lock(completer_lock);
          deferred_completer.emplace(std::move(completer));
        } else {
          completer.ReplySuccess();
        }
      });

  auto peripheral_client = ConnectPeripheral();
  ASSERT_OK(peripheral_client);

  std::atomic<bool> first_clear_finished = false;
  auto first_clear_promise = std::async(std::launch::async, [&]() {
    auto res = peripheral_client.value()->ClearFunctions();
    EXPECT_TRUE(res.ok()) << res.FormatDescription();
    first_clear_finished.store(true);
  });

  dut().runtime().RunUntilIdle();
  dut().runtime().RunUntil([&]() {
    std::lock_guard lock(completer_lock);
    return deferred_completer.has_value();
  });

  // Second ClearFunctions arrives while first teardown is awaiting SetConfigured(false).
  std::atomic<bool> second_clear_finished = false;
  auto second_clear_promise = std::async(std::launch::async, [&]() {
    auto res = peripheral_client.value()->ClearFunctions();
    EXPECT_TRUE(res.ok()) << res.FormatDescription();
    second_clear_finished.store(true);
  });
  // completer_cleanup is declared after both clear promises so that upon test failure or exception,
  // its destructor runs before ~future() blocks waiting on ClearFunctions().
  auto completer_cleanup = fit::defer([&]() {
    std::optional<FakeUsbFunction::SetConfiguredCompleterAsync> completer_to_run;
    {
      std::lock_guard lock(completer_lock);
      completer_to_run = std::move(deferred_completer);
      deferred_completer.reset();
    }
    if (completer_to_run.has_value()) {
      completer_to_run->ReplySuccess();
    }
  });

  dut().runtime().RunUntilIdle();
  // Neither ClearFunctions should have completed while teardown promises are in flight.
  EXPECT_FALSE(first_clear_finished.load());
  EXPECT_FALSE(second_clear_finished.load());
  ExpectState(UsbPeripheral::DeviceState::kStopping);

  // Now resolve the unconfigure completer.
  completer_cleanup.call();
  dut().runtime().RunUntilIdle();

  first_clear_promise.get();
  second_clear_promise.get();
  EXPECT_TRUE(first_clear_finished.load());
  EXPECT_TRUE(second_clear_finished.load());
  ExpectStateEventual(UsbPeripheral::DeviceState::kNoConfiguration);
}

}  // namespace
}  // namespace usb_peripheral::test
