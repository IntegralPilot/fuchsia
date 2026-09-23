// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>

#include "src/devices/usb/drivers/usb-peripheral/usb-peripheral-test-harness.h"

namespace usb_peripheral::test {
namespace {

TEST_F(UsbPeripheralFunctionTest, ConfigureAndRouteFidlCalls) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  fidl::WireResult alloc_res = function_client->AllocResources(1, {}, {});
  ASSERT_TRUE(alloc_res.ok()) << alloc_res.status_string();
  ASSERT_TRUE(alloc_res->is_ok()) << zx_status_get_string(alloc_res->error_value());
  uint8_t interface_num = alloc_res->value()->interface_nums[0];

  // Valid descriptors for ValidateFunction (we pass UMS's descriptors)
  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = interface_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 0,
      .b_interface_class = 8,
      .b_interface_sub_class = 6,
      .b_interface_protocol = 80,
      .i_interface = 0,
  };

  std::vector<uint8_t> descriptors(sizeof(intf_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));

  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(fake_function_endpoint));

  ASSERT_TRUE(configure_res.ok()) << configure_res.status_string();
  ASSERT_TRUE(configure_res->is_ok());

  // Controller starts when all functions are registered.
  ExpectControllerStarted(true);

  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);

  ASSERT_OK(dci()->SetConnected(true).status());

  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  fidl::Arena arena;
  std::vector<uint8_t> unused;

  // Test SetConfigured via standard endpoint request
  fdescriptor::wire::UsbSetup setup;
  setup.bm_request_type = USB_DIR_OUT | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_SET_CONFIGURATION;
  setup.w_value = 1;  // Configuration 1
  setup.w_index = interface_num;
  setup.w_length = 0;

  fidl::WireUnownedResult config_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  EXPECT_TRUE(config_res.ok()) << config_res.FormatDescription();
  ASSERT_OK(config_res.value());

  fake_function->WaitUntilCalled();
  EXPECT_TRUE(fake_function->set_configured_called());
  EXPECT_TRUE(fake_function->configured());

  // Test Control via provided endpoint request.
  setup.bm_request_type = USB_DIR_IN;
  setup.b_request = 0xAA;
  setup.w_value = 0x01;
  setup.w_index = 0x02;
  setup.w_length = 3;
  fidl::WireUnownedResult control_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));

  EXPECT_TRUE(control_res.ok()) << control_res.FormatDescription();
  ASSERT_OK(control_res.value());

  fake_function->WaitUntilCalled();
  EXPECT_TRUE(fake_function->control_called());
  EXPECT_EQ(0xAA, fake_function->control_req());

  // Test SetInterface via standard endpoint request
  setup.bm_request_type = USB_DIR_OUT | USB_RECIP_INTERFACE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_SET_INTERFACE;
  setup.w_value = 1;  // Alt setting 1
  setup.w_index = interface_num;
  setup.w_length = 0;

  fidl::WireUnownedResult intf_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(intf_res->is_ok()) << intf_res.FormatDescription();
  ASSERT_OK(intf_res.value());
  fake_function->WaitUntilCalled();

  EXPECT_TRUE(fake_function->set_interface_called());
  EXPECT_EQ(interface_num, fake_function->interface());
  EXPECT_EQ(1, fake_function->alt_setting());
}

// Test that a repeated SetConfiguration request for the same configuration ID
// forces the function driver to transition through an unconfigured (false)
// state. In compliance with the USB 2.0 specification (section 9.1.1.5), this
// unconfigured transition ensures that all endpoints and interface state are
// reset to default values.
TEST_F(UsbPeripheralFunctionTest, RepeatedSetConfigurationResetsFunction) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  fidl::WireResult alloc_res = function_client->AllocResources(1, {}, {});
  ASSERT_TRUE(alloc_res.ok()) << alloc_res.status_string();
  ASSERT_TRUE(alloc_res->is_ok()) << zx_status_get_string(alloc_res->error_value());
  uint8_t interface_num = alloc_res->value()->interface_nums[0];

  // Valid descriptors for ValidateFunction (we pass UMS's descriptors)
  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = interface_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 0,
      .b_interface_class = 8,
      .b_interface_sub_class = 6,
      .b_interface_protocol = 80,
      .i_interface = 0,
  };

  std::vector<uint8_t> descriptors(sizeof(intf_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));

  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(fake_function_endpoint));

  ASSERT_TRUE(configure_res.ok()) << configure_res.status_string();
  ASSERT_TRUE(configure_res->is_ok());

  // Controller starts when all functions are registered.
  ExpectControllerStarted(true);

  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);

  ASSERT_OK(dci()->SetConnected(true).status());

  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  fidl::Arena arena;
  std::vector<uint8_t> unused;

  // Test SetConfigured via standard endpoint request
  fdescriptor::wire::UsbSetup setup;
  setup.bm_request_type = USB_DIR_OUT | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_SET_CONFIGURATION;
  setup.w_value = 1;  // Configuration 1
  setup.w_index = interface_num;
  setup.w_length = 0;

  fidl::WireUnownedResult config_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  EXPECT_TRUE(config_res.ok()) << config_res.FormatDescription();
  ASSERT_OK(config_res.value());

  EXPECT_TRUE(fake_function->set_configured_called());
  EXPECT_TRUE(fake_function->configured());
  ASSERT_EQ(fake_function->configured_history().size(), 1u);
  EXPECT_TRUE(fake_function->configured_history()[0]);

  // Test repeated SetConfiguration request causes unconfigure/reconfigure transition.
  fake_function->clear_set_configured_called();
  fidl::WireUnownedResult config_res2 =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  EXPECT_TRUE(config_res2.ok()) << config_res2.FormatDescription();
  ASSERT_OK(config_res2.value());

  EXPECT_TRUE(fake_function->set_configured_called());
  EXPECT_TRUE(fake_function->configured());
  ASSERT_EQ(fake_function->configured_history().size(), 3u);
  EXPECT_FALSE(fake_function->configured_history()[1]);
  EXPECT_TRUE(fake_function->configured_history()[2]);
}

TEST_F(UsbPeripheralFunctionTest, ClearFunctionsWaitsForTeardown) {
  zx::result peripheral_client_result = ConnectPeripheral();
  ASSERT_OK(peripheral_client_result);
  auto peripheral_client = std::move(peripheral_client_result.value());

  zx::result endpoints = fidl::CreateEndpoints<fperipheral::Events>();
  ASSERT_OK(endpoints);

  FakeEvents fake_events;
  fake_events.Bind(std::move(endpoints->server));

  auto set_listener_res = peripheral_client->SetStateChangeListener(std::move(endpoints->client));
  ASSERT_TRUE(set_listener_res.ok()) << set_listener_res.FormatDescription();

  // Add a function so there is something to clear.
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  fidl::WireResult alloc_res = function_client->AllocResources(1, {}, {});
  ASSERT_TRUE(alloc_res.ok()) << alloc_res.FormatDescription();
  ASSERT_OK(alloc_res.value());
  uint8_t interface_num = alloc_res->value()->interface_nums[0];

  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = interface_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 0,
      .b_interface_class = 8,
      .b_interface_sub_class = 6,
      .b_interface_protocol = 80,
      .i_interface = 0,
  };
  std::vector<uint8_t> descriptors(sizeof(intf_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));

  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(fake_function_endpoint));
  ASSERT_TRUE(configure_res.ok()) << configure_res.FormatDescription();
  ASSERT_OK(configure_res.value());

  // Clear functions and wait for event.
  auto clear_res = peripheral_client->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

  // Wait for async teardown to complete.
  this->dut().runtime().RunUntilIdle();

  // DCI should be stopped.
  ExpectControllerStarted(false);

  // 2. State should be back to kNoConfiguration.
  ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);

  fake_events.WaitUntilCleared(this->dut().runtime());
  fake_events.Unbind();
}

TEST_F(UsbPeripheralFunctionTest, ConfigureFailsIfInterfaceNotAllocated) {
  ExpectControllerStarted(false);

  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  // We use an interface number that hasn't been allocated (0, by default in descriptors).
  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = 0,
      .b_alternate_setting = 0,
      .b_num_endpoints = 0,
      .b_interface_class = 8,
      .b_interface_sub_class = 6,
      .b_interface_protocol = 80,
      .i_interface = 0,
  };

  std::vector<uint8_t> descriptors(sizeof(intf_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));

  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(fake_function_endpoint));

  ASSERT_TRUE(configure_res.ok()) << configure_res.FormatDescription();
  EXPECT_STATUS(configure_res.value(), ZX_ERR_INVALID_ARGS);
}

TEST_F(UsbPeripheralFunctionTest, ConfigureFailsIfAlreadyBound) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  fidl::WireResult alloc_res = function_client->AllocResources(1, {}, {});
  ASSERT_TRUE(alloc_res.ok()) << alloc_res.status_string();
  ASSERT_TRUE(alloc_res->is_ok()) << zx_status_get_string(alloc_res->error_value());
  uint8_t interface_num = alloc_res->value()->interface_nums[0];

  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = interface_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 0,
      .b_interface_class = 8,
      .b_interface_sub_class = 6,
      .b_interface_protocol = 80,
      .i_interface = 0,
  };

  std::vector<uint8_t> descriptors(sizeof(intf_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));

  // First call should succeed
  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(fake_function_endpoint));
  ASSERT_TRUE(configure_res.ok()) << configure_res.FormatDescription();
  ASSERT_OK(configure_res.value());

  // Second call with a new endpoint should fail with ZX_ERR_ALREADY_BOUND.
  // The driver is already in kPeripheralReady because the first function was configured.
  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);
  zx::result endpoints = fidl::CreateEndpoints<ffunction::UsbFunctionInterface>();
  ASSERT_OK(endpoints);
  auto second_fake = std::make_shared<FakeUsbFunction>();
  second_fake->Bind(dut().runtime().StartBackgroundDispatcher(), std::move(endpoints->server));
  auto second_fake_endpoint = std::move(endpoints->client);
  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);

  fidl::WireResult second_configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(second_fake_endpoint));

  ASSERT_TRUE(second_configure_res.ok()) << second_configure_res.FormatDescription();
  EXPECT_STATUS(second_configure_res.value(), ZX_ERR_ALREADY_BOUND);
}

TEST_F(UsbPeripheralFunctionTest, ConnectToEndpointFailsIfEpNotAllocated) {
  ExpectControllerStarted(false);

  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  auto ep_endpoints = fidl::Endpoints<fendpoint::Endpoint>::Create();
  // Try to connect to endpoint 1 (which hasn't been allocated).
  fidl::WireResult connect_res =
      function_client->ConnectToEndpoint(1, std::move(ep_endpoints.server));
  ASSERT_TRUE(connect_res.ok()) << connect_res.FormatDescription();
  EXPECT_STATUS(connect_res.value(), ZX_ERR_NOT_FOUND);
}

TEST_F(UsbPeripheralFunctionTest, ConnectToEndpointSuccessAndSequencing) {
  ExpectControllerStarted(false);

  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  fidl::Arena arena;
  auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 1);
  endpoints[0].direction = fdescriptor::wire::EndpointDirection::kIn;
  endpoints[0].ep_info = BulkEpInfo(arena);
  endpoints[0].max_packet_size = 512;
  auto ep_endpoints = fidl::Endpoints<fendpoint::Endpoint>::Create();
  endpoints[0].endpoint = std::move(ep_endpoints.server);

  fidl::WireResult alloc_res =
      function_client->AllocResources(1, endpoints, fidl::VectorView<fidl::StringView>());
  ASSERT_TRUE(alloc_res.ok()) << alloc_res.status_string();
  ASSERT_TRUE(alloc_res->is_ok());

  uint8_t ep_addr = alloc_res->value()->endpoint_addrs[0];
  uint8_t interface_num = alloc_res->value()->interface_nums[0];

  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = interface_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 1,
      .b_interface_class = 8,
      .b_interface_sub_class = 6,
      .b_interface_protocol = 80,
  };
  usb_endpoint_descriptor_t ep_desc = {
      .b_length = sizeof(usb_endpoint_descriptor_t),
      .b_descriptor_type = USB_DT_ENDPOINT,
      .b_endpoint_address = ep_addr,
      .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
      .w_max_packet_size = 512,
  };
  std::vector<uint8_t> descriptors(sizeof(intf_desc) + sizeof(ep_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));
  memcpy(descriptors.data() + sizeof(intf_desc), &ep_desc, sizeof(ep_desc));

  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(fake_function_endpoint));
  ASSERT_TRUE(configure_res.ok()) << configure_res.status_string();
  ASSERT_TRUE(configure_res->is_ok());

  auto ep_endpoints2 = fidl::Endpoints<fendpoint::Endpoint>::Create();
  // Try to connect to the allocated endpoint.
  fidl::WireResult connect_res =
      function_client->ConnectToEndpoint(ep_addr, std::move(ep_endpoints2.server));
  ASSERT_TRUE(connect_res.ok()) << connect_res.FormatDescription();
  ASSERT_OK(connect_res.value());

  // Verify [FUNC-1.2]: SetConfigured must not have been called yet.
  EXPECT_FALSE(fake_function->set_configured_called());
}

TEST_F(UsbPeripheralFunctionTest, ConnectToEndpointServerEndCloseOnTeardown) {
  ExpectControllerStarted(false);

  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::Arena arena;
  zx::result<uint8_t> ep_addr_result = ConfigureDefaultFunction(function_client, arena);
  ASSERT_OK(ep_addr_result);
  uint8_t ep_addr = ep_addr_result.value();

  auto ep_endpoints = fidl::Endpoints<fendpoint::Endpoint>::Create();
  fidl::WireResult connect_res =
      function_client->ConnectToEndpoint(ep_addr, std::move(ep_endpoints.server));
  ASSERT_TRUE(connect_res.ok()) << connect_res.FormatDescription();
  ASSERT_OK(connect_res.value());

  // Verify the client channel is still connected (peer not closed).
  zx_signals_t observed = 0;
  EXPECT_STATUS(ep_endpoints.client.channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                       zx::time::infinite_past(), &observed),
                ZX_ERR_TIMED_OUT);

  // Stop the controller (which triggers teardown).
  zx::result peripheral_client_result = ConnectPeripheral();
  ASSERT_OK(peripheral_client_result);
  fidl::WireSyncClient<fperipheral::Device> peripheral_client =
      std::move(peripheral_client_result.value());

  FakeEvents fake_events;
  auto event_endpoints = fidl::CreateEndpoints<fperipheral::Events>();
  ASSERT_OK(event_endpoints);
  fake_events.Bind(std::move(event_endpoints->server));

  auto set_listener_res =
      peripheral_client->SetStateChangeListener(std::move(event_endpoints->client));
  ASSERT_TRUE(set_listener_res.ok()) << set_listener_res.FormatDescription();

  fidl::WireResult clear_res = peripheral_client->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.status_string();

  fake_events.WaitUntilCleared(dut().runtime());

  // Now the endpoint channel peer MUST be closed.
  EXPECT_OK(ep_endpoints.client.channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(),
                                                   &observed));
  EXPECT_TRUE(observed & ZX_CHANNEL_PEER_CLOSED);
}

TEST_F(UsbPeripheralFunctionTest, ConfigureFailsIfDescriptorsMalformed) {
  ExpectControllerStarted(false);

  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  fidl::WireResult alloc_res = function_client->AllocResources(1, {}, {});
  ASSERT_TRUE(alloc_res.ok()) << alloc_res.status_string();
  ASSERT_TRUE(alloc_res->is_ok());

  // Pass malformed descriptors (length byte = 5, but sizeof interface is 9).
  std::vector<uint8_t> malformed_descriptors(sizeof(usb_interface_descriptor_t));
  auto* intf = reinterpret_cast<usb_interface_descriptor_t*>(malformed_descriptors.data());
  intf->b_length = 5;  // Invalid! Must be 9.
  intf->b_descriptor_type = USB_DT_INTERFACE;

  fidl::WireResult configure_res =
      function_client->Configure(fidl::VectorView<uint8_t>::FromExternal(
                                     malformed_descriptors.data(), malformed_descriptors.size()),
                                 std::move(fake_function_endpoint));
  ASSERT_TRUE(configure_res.ok()) << configure_res.FormatDescription();
  EXPECT_STATUS(configure_res.value(), ZX_ERR_INVALID_ARGS);
}

TEST_F(UsbPeripheralFunctionTest, DeconfigureClosesEndpointChannels) {
  ExpectControllerStarted(false);

  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::Arena arena;
  zx::result<uint8_t> ep_addr_result = ConfigureDefaultFunction(function_client, arena);
  ASSERT_OK(ep_addr_result);
  uint8_t ep_addr = ep_addr_result.value();

  auto ep_endpoints = fidl::Endpoints<fendpoint::Endpoint>::Create();
  fidl::WireResult connect_res =
      function_client->ConnectToEndpoint(ep_addr, std::move(ep_endpoints.server));
  ASSERT_TRUE(connect_res.ok()) << connect_res.FormatDescription();
  ASSERT_OK(connect_res.value());

  // Verify the client channel is still connected (peer not closed).
  zx_signals_t observed = 0;
  EXPECT_STATUS(ep_endpoints.client.channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                       zx::time::infinite_past(), &observed),
                ZX_ERR_TIMED_OUT);

  // Deconfigure
  fidl::WireResult deconfig_res = function_client->Deconfigure();
  ASSERT_TRUE(deconfig_res.ok()) << deconfig_res.FormatDescription();
  ASSERT_OK(deconfig_res.value());

  // Verify the client channel is closed (because deconfigure stops the controller, disabling
  // endpoints).
  EXPECT_OK(ep_endpoints.client.channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(),
                                                   &observed));
  EXPECT_TRUE(observed & ZX_CHANNEL_PEER_CLOSED);
}

TEST_F(UsbPeripheralFunctionTest, ConfigureEndpointFailsIfDescriptorMissing) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::Arena arena;
  zx::result<uint8_t> configure_result = ConfigureDefaultFunction(function_client, arena);
  ASSERT_OK(configure_result);
  uint8_t ep_addr = configure_result.value();

  // Create configuration WITHOUT descriptor.
  auto config_builder = ffunction::wire::EndpointConfiguration::Builder(arena);
  // Do not call config_builder.descriptor(...)

  auto res = function_client->ConfigureEndpoint(ep_addr, config_builder.Build());
  ASSERT_TRUE(res.ok()) << res.FormatDescription();
  EXPECT_STATUS(res.value(), ZX_ERR_INVALID_ARGS);
}

TEST_F(UsbPeripheralFunctionTest, DeconfigureTrivialSuccessIfNotConfigured) {
  ExpectControllerStarted(false);

  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::WireResult deconfig_res = function_client->Deconfigure();
  ASSERT_TRUE(deconfig_res.ok()) << deconfig_res.FormatDescription();
  ASSERT_TRUE(deconfig_res->is_ok()) << zx_status_get_string(deconfig_res->error_value());
}

TEST_F(UsbPeripheralFunctionTest, AllocResourcesFailsIfInvalidDirection) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::Arena arena;
  auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 1);
  endpoints[0].direction = static_cast<fdescriptor::wire::EndpointDirection>(99);
  auto ep_endpoints = fidl::Endpoints<fendpoint::Endpoint>::Create();
  endpoints[0].endpoint = std::move(ep_endpoints.server);

  fidl::WireResult alloc_res = function_client->AllocResources(1, endpoints, {});
  if (alloc_res.ok()) {
    ASSERT_TRUE(alloc_res->is_error());
    EXPECT_STATUS(alloc_res->error_value(), ZX_ERR_INVALID_ARGS);
  } else {
    // If FIDL serialization failed, it's also acceptable validation.
    EXPECT_STATUS(alloc_res.status(), ZX_ERR_INVALID_ARGS);
  }
}

TEST_F(UsbPeripheralFunctionTest, DeconfigureAllowsReconfigure) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  fidl::WireResult alloc_res = function_client->AllocResources(1, {}, {});
  ASSERT_TRUE(alloc_res.ok()) << alloc_res.status_string();
  ASSERT_TRUE(alloc_res->is_ok()) << zx_status_get_string(alloc_res->error_value());
  uint8_t interface_num = alloc_res->value()->interface_nums[0];

  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = interface_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 0,
      .b_interface_class = 8,
      .b_interface_sub_class = 6,
      .b_interface_protocol = 80,
      .i_interface = 0,
  };

  std::vector<uint8_t> descriptors(sizeof(intf_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));

  // First Configure
  {
    fidl::WireResult result = function_client->Configure(
        fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
        std::move(fake_function_endpoint));
    ASSERT_TRUE(result.ok()) << result.FormatDescription();
    ASSERT_OK(result.value());
  }
  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);

  // Deconfigure
  {
    fidl::WireResult result = function_client->Deconfigure();
    ASSERT_TRUE(result.ok()) << result.FormatDescription();
    ASSERT_OK(result.value());
  }
  fake_function->WaitUntilUnbound();
  ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);

  ExpectControllerStarted(false);

  // Now Configure should succeed again with a new endpoint
  fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [new_fake_function, new_fake_function_endpoint] = std::move(fake_function_result.value());

  {
    fidl::WireResult result = function_client->Configure(
        fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
        std::move(new_fake_function_endpoint));
    ASSERT_TRUE(result.ok()) << result.FormatDescription();
    ASSERT_OK(result.value());
  }
  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);

  ExpectControllerStarted(true);

  // Verify new fake function is called.
  {
    ASSERT_OK(dci()->SetConnected(true).status());
    fdescriptor::wire::UsbSetup setup;
    setup.bm_request_type = USB_DIR_OUT | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
    setup.b_request = USB_REQ_SET_CONFIGURATION;
    setup.w_value = 1;  // Configuration 1
    setup.w_index = interface_num;
    setup.w_length = 0;
    fidl::Arena arena;
    std::vector<uint8_t> unused;
    fidl::WireResult config_res =
        dci()->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
    EXPECT_TRUE(config_res.ok()) << config_res.FormatDescription();
    ASSERT_OK(config_res.value());
  }

  new_fake_function->WaitUntilCalled();
  EXPECT_TRUE(new_fake_function->set_configured_called());
  EXPECT_TRUE(new_fake_function->configured());
}

TEST_F(UsbPeripheralFunctionTest, ControllerStoppedOnFunctionClose) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  fidl::WireResult alloc_res = function_client->AllocResources(1, {}, {});
  ASSERT_TRUE(alloc_res.ok()) << alloc_res.status_string();
  ASSERT_TRUE(alloc_res->is_ok()) << zx_status_get_string(alloc_res->error_value());
  uint8_t interface_num = alloc_res->value()->interface_nums[0];

  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = interface_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 0,
      .b_interface_class = 8,
      .b_interface_sub_class = 6,
      .b_interface_protocol = 80,
      .i_interface = 0,
  };

  std::vector<uint8_t> descriptors(sizeof(intf_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));

  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(fake_function_endpoint));
  ASSERT_TRUE(configure_res.ok()) << configure_res.FormatDescription();
  ASSERT_OK(configure_res.value());

  // Controller starts when all functions are registered.
  ExpectControllerStarted(true);

  // Set the stop completion before unbinding.
  libsync::Completion stop_completion;
  dut().RunInEnvironmentTypeContext(
      [&](UsbPeripheralTestEnvironment& env) { env.dci().set_stop_completion(&stop_completion); });

  // Close the fake function endpoint.
  fake_function->Unbind();

  // Wait for the controller to stop.
  ASSERT_OK(stop_completion.Wait(zx::sec(5)));

  dut().RunInEnvironmentTypeContext([](UsbPeripheralTestEnvironment& env) {
    env.dci().set_stop_completion(nullptr);
    EXPECT_FALSE(env.dci().controller_started());
  });
}

TEST_F(UsbPeripheralFunctionTest, AllocResources) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::Endpoints<fendpoint::Endpoint> ep_endpoints1 =
      fidl::Endpoints<fendpoint::Endpoint>::Create();
  fidl::Endpoints<fendpoint::Endpoint> ep_endpoints2 =
      fidl::Endpoints<fendpoint::Endpoint>::Create();

  zx_info_handle_basic_t info1, info2;
  ASSERT_OK(ep_endpoints1.server.channel().get_info(ZX_INFO_HANDLE_BASIC, &info1, sizeof(info1),
                                                    nullptr, nullptr));
  ASSERT_OK(ep_endpoints2.server.channel().get_info(ZX_INFO_HANDLE_BASIC, &info2, sizeof(info2),
                                                    nullptr, nullptr));

  fidl::Arena arena;
  auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 2);
  endpoints[0].direction = fdescriptor::wire::EndpointDirection::kIn;
  endpoints[0].ep_info = BulkEpInfo(arena);
  endpoints[0].max_packet_size = 512;
  endpoints[0].endpoint = std::move(ep_endpoints1.server);
  endpoints[1].direction = fdescriptor::wire::EndpointDirection::kOut;
  endpoints[1].ep_info = BulkEpInfo(arena);
  endpoints[1].max_packet_size = 512;
  endpoints[1].endpoint = std::move(ep_endpoints2.server);

  auto strings = fidl::VectorView<fidl::StringView>(arena, 2);
  strings[0] = fidl::StringView(arena, "string1");
  strings[1] = fidl::StringView(arena, "string2");

  fidl::WireResult res = function_client->AllocResources(1, endpoints, strings);

  ASSERT_TRUE(res.ok()) << res.FormatDescription();
  ASSERT_TRUE(res->is_ok()) << zx_status_get_string(res->error_value());

  auto* response = res->value();
  ASSERT_EQ(response->interface_nums.size(), 1u);
  ASSERT_EQ(response->endpoint_addrs.size(), 2u);
  ASSERT_EQ(response->string_indices.size(), 2u);

  uint8_t ep1_addr = response->endpoint_addrs[0];
  uint8_t ep2_addr = response->endpoint_addrs[1];

  // Verify endpoints connected to DCI.
  dut().RunInEnvironmentTypeContext([&](UsbPeripheralTestEnvironment& env) {
    auto dci_ep1 = env.dci().TakeEndpoint(ep1_addr);
    auto dci_ep2 = env.dci().TakeEndpoint(ep2_addr);

    ASSERT_TRUE(dci_ep1.is_valid());
    ASSERT_TRUE(dci_ep2.is_valid());

    zx_info_handle_basic_t dci_info1, dci_info2;
    ASSERT_OK(dci_ep1.channel().get_info(ZX_INFO_HANDLE_BASIC, &dci_info1, sizeof(dci_info1),
                                         nullptr, nullptr));
    ASSERT_OK(dci_ep2.channel().get_info(ZX_INFO_HANDLE_BASIC, &dci_info2, sizeof(dci_info2),
                                         nullptr, nullptr));

    EXPECT_EQ(info1.koid, dci_info1.koid);
    EXPECT_EQ(info2.koid, dci_info2.koid);
  });

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  struct {
    usb_interface_descriptor_t intf;
    usb_endpoint_descriptor_t ep1;
    usb_endpoint_descriptor_t ep2;
  } __PACKED combined_descriptors = {
      .intf =
          {
              .b_length = sizeof(usb_interface_descriptor_t),
              .b_descriptor_type = USB_DT_INTERFACE,
              .b_interface_number = response->interface_nums[0],
              .b_num_endpoints = 2,
              .b_interface_class = 8,
              .b_interface_sub_class = 6,
              .b_interface_protocol = 80,
              .i_interface = response->string_indices[0],
          },
      .ep1 =
          {
              .b_length = sizeof(usb_endpoint_descriptor_t),
              .b_descriptor_type = USB_DT_ENDPOINT,
              .b_endpoint_address = ep1_addr,
              .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
              .w_max_packet_size = 512,
          },
      .ep2 =
          {
              .b_length = sizeof(usb_endpoint_descriptor_t),
              .b_descriptor_type = USB_DT_ENDPOINT,
              .b_endpoint_address = ep2_addr,
              .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
              .w_max_packet_size = 512,
          },
  };

  std::vector<uint8_t> descriptors_vec(sizeof(combined_descriptors));
  memcpy(descriptors_vec.data(), &combined_descriptors, sizeof(combined_descriptors));

  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors_vec.data(), descriptors_vec.size()),
      std::move(fake_function_endpoint));

  ASSERT_TRUE(configure_res.ok()) << configure_res.status_string();
  ASSERT_TRUE(configure_res->is_ok());
}

TEST_F(UsbPeripheralFunctionTest, ResourceCleanupOnClose) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  auto function_client = std::move(function_client_result.value());

  fidl::Arena arena;
  auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 1);
  endpoints[0].direction = fdescriptor::wire::EndpointDirection::kIn;
  endpoints[0].ep_info = BulkEpInfo(arena);
  endpoints[0].max_packet_size = 512;
  auto ep_endpoints = fidl::Endpoints<fendpoint::Endpoint>::Create();
  endpoints[0].endpoint = std::move(ep_endpoints.server);

  auto strings = fidl::VectorView<fidl::StringView>(arena, 1);
  strings[0] = fidl::StringView(arena, "cleanup_test_string");

  fidl::WireResult res = function_client->AllocResources(1, endpoints, strings);
  ASSERT_TRUE(res.ok()) << res.FormatDescription();
  ASSERT_TRUE(res->is_ok()) << zx_status_get_string(res->error_value());

  // Verify resources are allocated.
  UsbPeripheral::ResourceAllocations allocations;
  dut().RunInDriverContext(
      [&](UsbPeripheral& peripheral) { allocations = peripheral.GetResourceAllocations(0); });
  ASSERT_EQ(allocations.interface_nums.size(), 1u);
  ASSERT_EQ(allocations.endpoint_addrs.size(), 1u);
  ASSERT_EQ(allocations.string_indices.size(), 1u);

  // Close the FIDL connection.
  function_client = {};

  // Verify resources are cleared.
  dut().runtime().RunUntil([&]() {
    dut().RunInDriverContext(
        [&](UsbPeripheral& peripheral) { allocations = peripheral.GetResourceAllocations(0); });
    return allocations.interface_nums.empty() && allocations.endpoint_addrs.empty() &&
           allocations.string_indices.empty();
  });
}

TEST_F(UsbPeripheralFunctionTest, AllocResourcesRollback) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  auto function_client = std::move(function_client_result.value());

  fidl::Arena arena;

  // 1. Initial success allocation to have a baseline of "used" resources.
  {
    auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 1);
    endpoints[0].direction = fdescriptor::wire::EndpointDirection::kIn;
    endpoints[0].ep_info = BulkEpInfo(arena);
    endpoints[0].max_packet_size = 512;
    auto ep_endpoints = fidl::Endpoints<fendpoint::Endpoint>::Create();
    endpoints[0].endpoint = std::move(ep_endpoints.server);

    auto strings = fidl::VectorView<fidl::StringView>(arena, 1);
    strings[0] = fidl::StringView(arena, "initial_string");

    fidl::WireResult res = function_client->AllocResources(1, endpoints, strings);
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_TRUE(res.value().is_ok()) << zx_status_get_string(res.value().error_value());
  }

  UsbPeripheral::ResourceAllocations initial;
  dut().RunInDriverContext(
      [&](UsbPeripheral& peripheral) { initial = peripheral.GetResourceAllocations(0); });
  ASSERT_EQ(initial.interface_nums.size(), 1u);
  ASSERT_EQ(initial.endpoint_addrs.size(), 1u);
  ASSERT_EQ(initial.string_indices.size(), 1u);

  // 2. Perform a request that should succeed for strings and endpoints, but
  //    fails for interfaces. We already have 1 interface. Requesting
  //    UsbPeripheral::MAX_INTERFACES more should fail.
  {
    auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 1);
    endpoints[0].direction = fdescriptor::wire::EndpointDirection::kOut;
    endpoints[0].ep_info = BulkEpInfo(arena);
    endpoints[0].max_packet_size = 512;
    auto ep_endpoints = fidl::Endpoints<fendpoint::Endpoint>::Create();
    endpoints[0].endpoint = std::move(ep_endpoints.server);

    auto strings = fidl::VectorView<fidl::StringView>(arena, 1);
    strings[0] = fidl::StringView(arena, "should_rollback");

    fidl::WireResult res =
        function_client->AllocResources(UsbPeripheral::kMaxInterfaces, endpoints, strings);
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    EXPECT_STATUS(res.value().error_value(), ZX_ERR_NO_RESOURCES);
  }

  // Verify only initial resources remain.
  UsbPeripheral::ResourceAllocations allocations;
  dut().RunInDriverContext(
      [&](UsbPeripheral& peripheral) { allocations = peripheral.GetResourceAllocations(0); });
  EXPECT_EQ(allocations.interface_nums, initial.interface_nums);
  EXPECT_EQ(allocations.endpoint_addrs, initial.endpoint_addrs);
  EXPECT_EQ(allocations.string_indices, initial.string_indices);

  // 3. Perform a request that should succeed for interfaces and endpoints, but
  //    fails for strings. Global strings (3) + Initial function strings taken.
  //    Requesting enough to exceed UsbPeripheral::MAX_STRINGS should fail.
  {
    auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 1);
    endpoints[0].direction = fdescriptor::wire::EndpointDirection::kOut;
    endpoints[0].ep_info = BulkEpInfo(arena);
    endpoints[0].max_packet_size = 512;
    auto ep_endpoints = fidl::Endpoints<fendpoint::Endpoint>::Create();
    endpoints[0].endpoint = std::move(ep_endpoints.server);

    std::vector<fidl::StringView> strings_vec(UsbPeripheral::kMaxStrings,
                                              fidl::StringView(arena, "too_many"));

    fidl::WireResult res = function_client->AllocResources(
        1, endpoints,
        fidl::VectorView<fidl::StringView>::FromExternal(strings_vec.data(), strings_vec.size()));
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    EXPECT_STATUS(res.value().error_value(), ZX_ERR_NO_RESOURCES);
  }

  // Verify only initial resources remain.
  dut().RunInDriverContext(
      [&](UsbPeripheral& peripheral) { allocations = peripheral.GetResourceAllocations(0); });
  EXPECT_EQ(allocations.interface_nums, initial.interface_nums);
  EXPECT_EQ(allocations.endpoint_addrs, initial.endpoint_addrs);
  EXPECT_EQ(allocations.string_indices, initial.string_indices);

  // 4. Perform a request that should succeed for strings and interfaces, but
  //    fails for endpoints. Initial function IN endpoint (1) taken. Total IN
  //    endpoints available: UsbPeripheral::IN_EP_END -
  //    UsbPeripheral::IN_EP_START + 1.
  {
    size_t total_in_eps = UsbPeripheral::kInEpEnd - UsbPeripheral::kInEpStart + 1;
    auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, total_in_eps);
    for (size_t i = 0; i < total_in_eps; i++) {
      endpoints[i].direction = fdescriptor::wire::EndpointDirection::kIn;
      endpoints[i].ep_info = BulkEpInfo(arena);
      endpoints[i].max_packet_size = 512;
      auto ep_endpoints = fidl::Endpoints<fendpoint::Endpoint>::Create();
      endpoints[i].endpoint = std::move(ep_endpoints.server);
    }

    auto strings = fidl::VectorView<fidl::StringView>(arena, 1);
    strings[0] = fidl::StringView(arena, "should_rollback");

    fidl::WireResult res = function_client->AllocResources(1, endpoints, strings);
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    EXPECT_STATUS(res.value().error_value(), ZX_ERR_NO_RESOURCES);
  }

  // Verify only initial resources remain.
  dut().RunInDriverContext(
      [&](UsbPeripheral& peripheral) { allocations = peripheral.GetResourceAllocations(0); });
  EXPECT_EQ(allocations.interface_nums, initial.interface_nums);
  EXPECT_EQ(allocations.endpoint_addrs, initial.endpoint_addrs);
  EXPECT_EQ(allocations.string_indices, initial.string_indices);
}

TEST_F(UsbPeripheralFunctionTest, EndpointSetStall) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::Arena arena;
  zx::result<uint8_t> configure_result = ConfigureDefaultFunction(function_client, arena);
  ASSERT_OK(configure_result);
  uint8_t ep_addr = configure_result.value();

  // Test setting a stall on an allocated endpoint.
  auto res = function_client->EndpointSetStall(ep_addr);
  ASSERT_TRUE(res.ok()) << res.FormatDescription();
  ASSERT_OK(res.value());

  dut().RunInEnvironmentTypeContext([ep_addr](UsbPeripheralTestEnvironment& env) {
    auto stalls = env.dci().set_stalls();
    EXPECT_EQ(stalls.size(), 1u);
    EXPECT_EQ(stalls[0], ep_addr);
  });

  // Test double call to SetStall is forwarded and succeeds.
  auto res_double = function_client->EndpointSetStall(ep_addr);
  ASSERT_TRUE(res_double.ok()) << res_double.FormatDescription();
  ASSERT_OK(res_double.value());

  dut().RunInEnvironmentTypeContext([ep_addr](UsbPeripheralTestEnvironment& env) {
    auto stalls = env.dci().set_stalls();
    EXPECT_EQ(stalls.size(), 2u);
    EXPECT_EQ(stalls[1], ep_addr);
  });

  // Test an unknown/failing endpoint stall by toggling `fail_stall_` in our mock.
  dut().RunInEnvironmentTypeContext(
      [](UsbPeripheralTestEnvironment& env) { env.dci().fail_stall_.store(true); });

  auto res2 = function_client->EndpointSetStall(ep_addr);
  ASSERT_TRUE(res2.ok()) << res2.FormatDescription();
  EXPECT_STATUS(res2.value(), ZX_ERR_IO_NOT_PRESENT);

  // Test setting a stall on an unallocated endpoint.
  uint8_t unallocated_ep_addr = (ep_addr == 0x81) ? 0x82 : 0x81;
  auto res3 = function_client->EndpointSetStall(unallocated_ep_addr);
  ASSERT_TRUE(res3.ok()) << res3.FormatDescription();
  EXPECT_STATUS(res3.value(), ZX_ERR_NOT_FOUND);
}

TEST_F(UsbPeripheralFunctionTest, EndpointClearStall) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::Arena arena;
  zx::result<uint8_t> configure_result = ConfigureDefaultFunction(function_client, arena);
  ASSERT_OK(configure_result);
  uint8_t ep_addr = configure_result.value();

  // Test clearing a stall on an allocated endpoint.
  auto res = function_client->EndpointClearStall(ep_addr);
  ASSERT_TRUE(res.ok()) << res.FormatDescription();
  ASSERT_OK(res.value());

  dut().RunInEnvironmentTypeContext([ep_addr](UsbPeripheralTestEnvironment& env) {
    auto stalls = env.dci().clear_stalls();
    EXPECT_EQ(stalls.size(), 1u);
    EXPECT_EQ(stalls[0], ep_addr);
  });

  // Test double call to ClearStall is forwarded and succeeds.
  auto res_double = function_client->EndpointClearStall(ep_addr);
  ASSERT_TRUE(res_double.ok()) << res_double.FormatDescription();
  ASSERT_OK(res_double.value());

  dut().RunInEnvironmentTypeContext([ep_addr](UsbPeripheralTestEnvironment& env) {
    auto stalls = env.dci().clear_stalls();
    EXPECT_EQ(stalls.size(), 2u);
    EXPECT_EQ(stalls[1], ep_addr);
  });

  // Test an unknown/failing endpoint stall by toggling `fail_stall_` in our mock.
  dut().RunInEnvironmentTypeContext(
      [](UsbPeripheralTestEnvironment& env) { env.dci().fail_stall_.store(true); });

  auto res2 = function_client->EndpointClearStall(ep_addr);
  ASSERT_TRUE(res2.ok()) << res2.FormatDescription();
  EXPECT_STATUS(res2.value(), ZX_ERR_IO_NOT_PRESENT);

  // Test clearing a stall on an unallocated endpoint.
  uint8_t unallocated_ep_addr = (ep_addr == 0x81) ? 0x82 : 0x81;
  auto res3 = function_client->EndpointClearStall(unallocated_ep_addr);
  ASSERT_TRUE(res3.ok()) << res3.FormatDescription();
  EXPECT_STATUS(res3.value(), ZX_ERR_NOT_FOUND);
}

TEST_P(UsbPeripheralFunctionConfigureEndpointTest, ConfigureEndpoint) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::Arena arena;
  zx::result<uint8_t> configure_result = ConfigureDefaultFunction(function_client, arena);
  ASSERT_OK(configure_result);
  uint8_t ep_addr = configure_result.value();

  ffunction::wire::EndpointDescriptor desc = {
      .bm_attributes = 1,
      .w_max_packet_size = 2,
      .b_interval = 3,
  };

  auto config_builder = ffunction::wire::EndpointConfiguration::Builder(arena);
  config_builder.descriptor(desc);

  bool with_ss_companion = GetParam();
  ffunction::wire::SuperSpeedEndpointCompanionDescriptor ss_desc;
  if (with_ss_companion) {
    ss_desc = {
        .b_max_burst = 5,
        .bm_attributes = 4,
        .w_bytes_per_interval = 6,
    };
    config_builder.super_speed_companion(ss_desc);
  }

  ffunction::wire::EndpointConfiguration config = config_builder.Build();

  auto res = function_client->ConfigureEndpoint(ep_addr, config);
  ASSERT_TRUE(res.ok()) << res.FormatDescription();
  ASSERT_OK(res.value());

  dut().RunInEnvironmentTypeContext(
      [ep_addr, &desc, with_ss_companion, &ss_desc](UsbPeripheralTestEnvironment& env) {
        auto configured_eps = env.dci().configured_endpoints();
        auto configured_ss = env.dci().configured_endpoints_ss_companion();
        EXPECT_EQ(configured_eps.size(), 1u);
        EXPECT_EQ(configured_eps[0].b_endpoint_address, ep_addr);
        EXPECT_EQ(configured_eps[0].w_max_packet_size, desc.w_max_packet_size);
        EXPECT_EQ(configured_eps[0].bm_attributes, desc.bm_attributes);
        EXPECT_EQ(configured_eps[0].b_interval, desc.b_interval);
        if (with_ss_companion) {
          EXPECT_EQ(configured_ss[0].b_max_burst, ss_desc.b_max_burst);
          EXPECT_EQ(configured_ss[0].bm_attributes, ss_desc.bm_attributes);
          EXPECT_EQ(configured_ss[0].w_bytes_per_interval, ss_desc.w_bytes_per_interval);
        } else {
          EXPECT_EQ(configured_ss[0].b_max_burst, 0);
          EXPECT_EQ(configured_ss[0].bm_attributes, 0);
          EXPECT_EQ(configured_ss[0].w_bytes_per_interval, 0u);
        }
      });

  // Test unknown endpoint configuration.
  uint8_t unallocated_ep_addr = (ep_addr == 0x81) ? 0x82 : 0x81;
  auto res2 = function_client->ConfigureEndpoint(unallocated_ep_addr, config);
  ASSERT_TRUE(res2.ok()) << res2.FormatDescription();
  EXPECT_STATUS(res2.value(), ZX_ERR_NOT_FOUND);

  // Test failing configuration from DCI.
  dut().RunInEnvironmentTypeContext(
      [](UsbPeripheralTestEnvironment& env) { env.dci().fail_configure_.store(true); });
  auto res3 = function_client->ConfigureEndpoint(ep_addr, config);
  ASSERT_TRUE(res3.ok()) << res3.FormatDescription();
  EXPECT_STATUS(res3.value(), ZX_ERR_IO_NOT_PRESENT);
}

INSTANTIATE_TEST_SUITE_P(UsbPeripheralFunctionConfigureEndpointTest,
                         UsbPeripheralFunctionConfigureEndpointTest, testing::Bool());

TEST_F(UsbPeripheralFunctionTest, DisableEndpoint) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::Arena arena;
  zx::result<uint8_t> configure_result = ConfigureDefaultFunction(function_client, arena);
  ASSERT_OK(configure_result);
  uint8_t ep_addr = configure_result.value();

  // Test disabling an allocated endpoint.
  auto res = function_client->DisableEndpoint(ep_addr);
  ASSERT_TRUE(res.ok()) << res.FormatDescription();
  ASSERT_TRUE(res->is_ok()) << zx_status_get_string(res->error_value());

  dut().RunInEnvironmentTypeContext([ep_addr](UsbPeripheralTestEnvironment& env) {
    auto disabled_eps = env.dci().disabled_endpoints();
    EXPECT_EQ(disabled_eps.size(), 1u);
    EXPECT_EQ(disabled_eps[0], ep_addr);
  });

  // Test double call to DisableEndpoint is forwarded and succeeds.
  auto res_double = function_client->DisableEndpoint(ep_addr);
  ASSERT_TRUE(res_double.ok()) << res_double.FormatDescription();
  ASSERT_TRUE(res_double->is_ok()) << zx_status_get_string(res_double->error_value());

  dut().RunInEnvironmentTypeContext([ep_addr](UsbPeripheralTestEnvironment& env) {
    auto disabled_eps = env.dci().disabled_endpoints();
    EXPECT_EQ(disabled_eps.size(), 2u);
    EXPECT_EQ(disabled_eps[1], ep_addr);
  });

  // Test unknown endpoint disable
  uint8_t unallocated_ep_addr = (ep_addr == 0x81) ? 0x82 : 0x81;
  auto res2 = function_client->DisableEndpoint(unallocated_ep_addr);
  ASSERT_TRUE(res2.ok()) << res2.FormatDescription();
  EXPECT_STATUS(res2.value(), ZX_ERR_NOT_FOUND);

  // Test failing disable from DCI when controller is unpowered/stopped.
  // ZX_ERR_IO_NOT_PRESENT is treated as success because the endpoint is already
  // effectively disabled in hardware when the DCI controller is disconnected or unpowered.
  auto fail_cleanup = fit::defer([&]() {
    dut().RunInEnvironmentTypeContext(
        [](UsbPeripheralTestEnvironment& env) { env.dci().fail_disable_status_.store(ZX_OK); });
  });
  dut().RunInEnvironmentTypeContext([](UsbPeripheralTestEnvironment& env) {
    env.dci().fail_disable_status_.store(ZX_ERR_IO_NOT_PRESENT);
  });
  auto res3 = function_client->DisableEndpoint(ep_addr);
  ASSERT_TRUE(res3.ok()) << res3.FormatDescription();
  EXPECT_OK(res3.value());

  // Test failing disable from DCI with an unexpected error status.
  dut().RunInEnvironmentTypeContext([](UsbPeripheralTestEnvironment& env) {
    env.dci().fail_disable_status_.store(ZX_ERR_INTERNAL);
  });
  auto res4 = function_client->DisableEndpoint(ep_addr);
  ASSERT_TRUE(res4.ok()) << res4.FormatDescription();
  EXPECT_STATUS(res4.value(), ZX_ERR_INTERNAL);
}

TEST_F(UsbPeripheralFunctionTest, StateTransitionErrors) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::Arena arena;
  auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 1);
  endpoints[0].direction = fdescriptor::wire::EndpointDirection::kIn;
  endpoints[0].ep_info = BulkEpInfo(arena);
  endpoints[0].max_packet_size = 512;
  auto ep_endpoints = fidl::Endpoints<fendpoint::Endpoint>::Create();
  endpoints[0].endpoint = std::move(ep_endpoints.server);

  fidl::WireResult alloc_res =
      function_client->AllocResources(1, endpoints, fidl::VectorView<fidl::StringView>());
  ASSERT_TRUE(alloc_res.ok());
  ASSERT_OK(alloc_res.value());

  uint8_t ep_addr = alloc_res->value()->endpoint_addrs[0];

  // 1. Test endpoint methods before Configure -> should return ZX_ERR_BAD_STATE.
  {
    auto res = function_client->EndpointSetStall(ep_addr);
    ASSERT_TRUE(res.ok());
    EXPECT_STATUS(res.value(), ZX_ERR_BAD_STATE);
  }
  {
    auto res = function_client->EndpointClearStall(ep_addr);
    ASSERT_TRUE(res.ok());
    EXPECT_STATUS(res.value(), ZX_ERR_BAD_STATE);
  }
  {
    ffunction::wire::EndpointDescriptor desc = {
        .bm_attributes = 1,
        .w_max_packet_size = 2,
        .b_interval = 3,
    };
    auto config_builder = ffunction::wire::EndpointConfiguration::Builder(arena);
    config_builder.descriptor(desc);
    auto res = function_client->ConfigureEndpoint(ep_addr, config_builder.Build());
    ASSERT_TRUE(res.ok());
    EXPECT_STATUS(res.value(), ZX_ERR_BAD_STATE);
  }
  {
    auto res = function_client->DisableEndpoint(ep_addr);
    ASSERT_TRUE(res.ok());
    EXPECT_STATUS(res.value(), ZX_ERR_BAD_STATE);
  }

  // 2. Configure the function.
  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = alloc_res->value()->interface_nums[0],
      .b_alternate_setting = 0,
      .b_num_endpoints = 1,
      .b_interface_class = 8,
      .b_interface_sub_class = 6,
      .b_interface_protocol = 80,
  };
  usb_endpoint_descriptor_t ep_desc = {
      .b_length = sizeof(usb_endpoint_descriptor_t),
      .b_descriptor_type = USB_DT_ENDPOINT,
      .b_endpoint_address = ep_addr,
      .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
      .w_max_packet_size = 512,
  };
  std::vector<uint8_t> descriptors(sizeof(intf_desc) + sizeof(ep_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));
  memcpy(descriptors.data() + sizeof(intf_desc), &ep_desc, sizeof(ep_desc));

  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(fake_function_endpoint));
  ASSERT_TRUE(configure_res.ok());
  ASSERT_OK(configure_res.value());

  // 3. Test AllocResources after Configure -> should return ZX_ERR_BAD_STATE.
  {
    auto endpoints2 = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 0);
    fidl::WireResult alloc_res2 =
        function_client->AllocResources(1, endpoints2, fidl::VectorView<fidl::StringView>());
    ASSERT_TRUE(alloc_res2.ok());
    EXPECT_STATUS(alloc_res2.value(), ZX_ERR_BAD_STATE);
  }
}

TEST_F(UsbPeripheralFunctionTest, ConfigureEndpointDuringSetConfigured) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  fidl::Arena arena;
  auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 1);
  endpoints[0].direction = fdescriptor::wire::EndpointDirection::kIn;
  endpoints[0].ep_info = BulkEpInfo(arena);
  endpoints[0].max_packet_size = 512;
  auto ep_endpoints = fidl::Endpoints<fendpoint::Endpoint>::Create();
  endpoints[0].endpoint = std::move(ep_endpoints.server);

  fidl::WireResult alloc_res =
      function_client->AllocResources(1, endpoints, fidl::VectorView<fidl::StringView>());
  ASSERT_TRUE(alloc_res.ok());
  ASSERT_OK(alloc_res.value());

  uint8_t ep_addr = alloc_res->value()->endpoint_addrs[0];
  uint8_t interface_num = alloc_res->value()->interface_nums[0];

  std::vector<uint8_t> descriptors;
  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = interface_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 0,
  };
  descriptors.resize(sizeof(intf_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));

  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(fake_function_endpoint));
  ASSERT_TRUE(configure_res.ok()) << configure_res.FormatDescription();
  ASSERT_OK(configure_res.value());

  ExpectControllerStarted(true);
  ASSERT_OK(dci()->SetConnected(true).status());

  libsync::Completion configure_endpoint_completed;

  // Set the callback that simulates the condition of calling back into the
  // function before responding to set configured.
  auto callback_cleanup = fit::defer([&]() { fake_function->set_on_set_configured(nullptr); });
  fake_function->set_on_set_configured([&]() {
    fidl::Arena arena;
    ffunction::wire::EndpointDescriptor desc;
    desc.bm_attributes = 2;
    desc.w_max_packet_size = 512;
    desc.b_interval = 0;
    auto config_builder = ffunction::wire::EndpointConfiguration::Builder(arena);
    config_builder.descriptor(desc);

    auto res = function_client->ConfigureEndpoint(ep_addr, config_builder.Build());
    ASSERT_TRUE(res.ok()) << res.FormatDescription();
    ASSERT_OK(res.value());

    configure_endpoint_completed.Signal();
  });

  // Trigger SetConfigured by sending standard endpoint request SetConfiguration = 1
  fdescriptor::wire::UsbSetup setup;
  setup.bm_request_type = USB_DIR_OUT | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_SET_CONFIGURATION;
  setup.w_value = 1;
  setup.w_index = 0;
  setup.w_length = 0;

  std::vector<uint8_t> unused;
  fidl::WireUnownedResult config_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(config_res.ok()) << config_res.FormatDescription();
  ASSERT_OK(config_res.value());

  fake_function->WaitUntilCalled();
  EXPECT_TRUE(fake_function->set_configured_called());
  EXPECT_TRUE(fake_function->configured());

  ASSERT_OK(configure_endpoint_completed.Wait(zx::sec(5)));

  dut().RunInEnvironmentTypeContext([ep_addr](UsbPeripheralTestEnvironment& env) {
    auto configured_eps = env.dci().configured_endpoints();
    EXPECT_EQ(configured_eps.size(), 1u);
    EXPECT_EQ(configured_eps[0].b_endpoint_address, ep_addr);
  });
}

// ============================================================================
// NEW CONTRACT TESTS (Missing Matrix Items)
// ============================================================================

// [FUNC-1.4] ConnectToEndpoint validates ep not bound (fails ALREADY_BOUND)
TEST_F(UsbPeripheralFunctionTest, ConnectToEndpointFailsIfAlreadyBound) {
  ExpectControllerStarted(false);

  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::Arena arena;
  zx::result<uint8_t> ep_addr_result = ConfigureDefaultFunction(function_client, arena);
  ASSERT_OK(ep_addr_result);
  uint8_t ep_addr = ep_addr_result.value();

  // First connection.
  auto ep_endpoints1 = fidl::Endpoints<fendpoint::Endpoint>::Create();
  fidl::WireResult connect_res1 =
      function_client->ConnectToEndpoint(ep_addr, std::move(ep_endpoints1.server));
  ASSERT_TRUE(connect_res1.ok()) << connect_res1.FormatDescription();
  ASSERT_OK(connect_res1.value());

  // Second connection to same endpoint should fail.
  // Tell fake DCI to return ALREADY_BOUND for next connections.
  dut().RunInEnvironmentTypeContext(
      [](UsbPeripheralTestEnvironment& env) { env.dci().fail_already_bound_.store(true); });

  auto ep_endpoints2 = fidl::Endpoints<fendpoint::Endpoint>::Create();
  fidl::WireResult connect_res2 =
      function_client->ConnectToEndpoint(ep_addr, std::move(ep_endpoints2.server));
  ASSERT_TRUE(connect_res2.ok()) << connect_res2.FormatDescription();
  EXPECT_STATUS(connect_res2.value(), ZX_ERR_ALREADY_BOUND);
}

// [FUNC-2.2] Configure validates first descriptor is Interface/IAD (fails INVALID_ARGS)
TEST_F(UsbPeripheralFunctionTest, ConfigureFailsIfFirstDescriptorNotInterfaceOrIad) {
  ExpectControllerStarted(false);

  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  fidl::WireResult alloc_res = function_client->AllocResources(1, {}, {});
  ASSERT_TRUE(alloc_res.ok()) << alloc_res.status_string();
  ASSERT_TRUE(alloc_res->is_ok());

  // Pass endpoint descriptor first instead of Interface.
  usb_endpoint_descriptor_t ep_desc = {
      .b_length = sizeof(usb_endpoint_descriptor_t),
      .b_descriptor_type = USB_DT_ENDPOINT,
      .b_endpoint_address = 0x81,
      .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
      .w_max_packet_size = 512,
  };
  std::vector<uint8_t> descriptors(sizeof(ep_desc));
  memcpy(descriptors.data(), &ep_desc, sizeof(ep_desc));

  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(fake_function_endpoint));
  ASSERT_TRUE(configure_res.ok()) << configure_res.FormatDescription();
  EXPECT_STATUS(configure_res.value(), ZX_ERR_INVALID_ARGS);
}

class UsbPeripheralAllocationTest : public UsbPeripheralHarness<false> {
 public:
  usb_peripheral_config::Config GetDriverConfig() override {
    usb_peripheral_config::Config config;
    config.functions() = {"test"};
    return config;
  }
};

TEST_F(UsbPeripheralAllocationTest, BestFit) {
  dut().RunInEnvironmentTypeContext([](UsbPeripheralTestEnvironment& env) {
    std::vector<FakeDevice::EndpointCaps> caps = {
        {0x81, 64, {FakeDevice::EpType::kBulk, FakeDevice::EpType::kInterrupt}},
        {0x82, 512, {FakeDevice::EpType::kBulk, FakeDevice::EpType::kInterrupt}},
        {0x01, 512, {FakeDevice::EpType::kBulk, FakeDevice::EpType::kInterrupt}},
    };
    env.dci().SetHardwareInfo(std::move(caps), false);
  });

  StartDriverWithConfig(GetDriverConfig());

  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  auto function_client = std::move(function_client_result.value());

  fidl::Arena arena;
  auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 3);

  auto ep_endpoints1 = fidl::Endpoints<fendpoint::Endpoint>::Create();
  endpoints[0].direction = fdescriptor::wire::EndpointDirection::kIn;
  endpoints[0].ep_info = InterruptEpInfo(arena);
  endpoints[0].max_packet_size = 16;
  endpoints[0].endpoint = std::move(ep_endpoints1.server);

  auto ep_endpoints2 = fidl::Endpoints<fendpoint::Endpoint>::Create();
  endpoints[1].direction = fdescriptor::wire::EndpointDirection::kIn;
  endpoints[1].ep_info = BulkEpInfo(arena);
  endpoints[1].max_packet_size = 512;
  endpoints[1].endpoint = std::move(ep_endpoints2.server);

  auto ep_endpoints3 = fidl::Endpoints<fendpoint::Endpoint>::Create();
  endpoints[2].direction = fdescriptor::wire::EndpointDirection::kOut;
  endpoints[2].ep_info = BulkEpInfo(arena);
  endpoints[2].max_packet_size = 512;
  endpoints[2].endpoint = std::move(ep_endpoints3.server);

  fidl::WireResult res = function_client->AllocResources(1, endpoints, {});
  ASSERT_TRUE(res.ok()) << res.FormatDescription();
  ASSERT_TRUE(res->is_ok()) << zx_status_get_string(res->error_value());

  auto* response = res->value();
  ASSERT_EQ(response->endpoint_addrs.size(), 3u);
  EXPECT_EQ(response->endpoint_addrs[0], 0x81);
  EXPECT_EQ(response->endpoint_addrs[1], 0x82);
  EXPECT_EQ(response->endpoint_addrs[2], 0x01);
  // Verify inspect
  this->dut().RunInDriverContext([](UsbPeripheral& peripheral) {
    auto hierarchy = usb_inspect::ReadHierarchyFromInspector(peripheral.inspector());
    auto* node = hierarchy.GetByPath({"usb-peripheral", "hardware_info"});
    ASSERT_NE(node, nullptr);
    EXPECT_THAT(*node,
                NodeMatches(PropertyList(Contains(BoolIs("supports_dynamic_ep_sizing", false)))));

    auto* ep_81 = hierarchy.GetByPath({"usb-peripheral", "hardware_info", "ep_0x81"});
    ASSERT_NE(ep_81, nullptr);
    EXPECT_THAT(*ep_81, NodeMatches(PropertyList(Contains(UintIs("max_packet_size_limit", 64)))));
    EXPECT_THAT(
        *ep_81,
        NodeMatches(PropertyList(Contains(StringIs("supported_types", "bulk, interrupt")))));

    auto* ep_82 = hierarchy.GetByPath({"usb-peripheral", "hardware_info", "ep_0x82"});
    ASSERT_NE(ep_82, nullptr);
    EXPECT_THAT(*ep_82, NodeMatches(PropertyList(Contains(UintIs("max_packet_size_limit", 512)))));
    EXPECT_THAT(
        *ep_82,
        NodeMatches(PropertyList(Contains(StringIs("supported_types", "bulk, interrupt")))));

    auto* ep_01 = hierarchy.GetByPath({"usb-peripheral", "hardware_info", "ep_0x01"});
    ASSERT_NE(ep_01, nullptr);
    EXPECT_THAT(*ep_01, NodeMatches(PropertyList(Contains(UintIs("max_packet_size_limit", 512)))));
    EXPECT_THAT(
        *ep_01,
        NodeMatches(PropertyList(Contains(StringIs("supported_types", "bulk, interrupt")))));
  });
}

TEST_F(UsbPeripheralAllocationTest, Dynamic) {
  dut().RunInEnvironmentTypeContext(
      [](UsbPeripheralTestEnvironment& env) { env.dci().SetHardwareInfo({}, true); });

  StartDriverWithConfig(GetDriverConfig());

  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  auto function_client = std::move(function_client_result.value());

  fidl::Arena arena;
  auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 2);

  auto ep_endpoints1 = fidl::Endpoints<fendpoint::Endpoint>::Create();
  endpoints[0].direction = fdescriptor::wire::EndpointDirection::kIn;
  endpoints[0].ep_info = BulkEpInfo(arena);
  endpoints[0].max_packet_size = 512;
  endpoints[0].endpoint = std::move(ep_endpoints1.server);

  auto ep_endpoints2 = fidl::Endpoints<fendpoint::Endpoint>::Create();
  endpoints[1].direction = fdescriptor::wire::EndpointDirection::kOut;
  endpoints[1].ep_info = BulkEpInfo(arena);
  endpoints[1].max_packet_size = 512;
  endpoints[1].endpoint = std::move(ep_endpoints2.server);

  fidl::WireResult res = function_client->AllocResources(1, endpoints, {});
  ASSERT_TRUE(res.ok()) << res.FormatDescription();
  ASSERT_TRUE(res->is_ok()) << zx_status_get_string(res->error_value());

  auto* response = res->value();
  ASSERT_EQ(response->endpoint_addrs.size(), 2u);
  EXPECT_EQ(response->endpoint_addrs[0], 0x81);
  EXPECT_EQ(response->endpoint_addrs[1], 0x01);

  bool alloc_called = false;
  dut().RunInEnvironmentTypeContext(
      [&](UsbPeripheralTestEnvironment& env) { alloc_called = env.dci().alloc_called(); });
  EXPECT_TRUE(alloc_called);
}

TEST_F(UsbPeripheralAllocationTest, AllocationRollback) {
  dut().RunInEnvironmentTypeContext([](UsbPeripheralTestEnvironment& env) {
    env.dci().SetHardwareInfo({}, true);
    // Limit allocations to 1. The second one will fail.
    env.dci().set_max_allocs(1);
    env.dci().clear_freed_endpoints();
  });

  StartDriverWithConfig(GetDriverConfig());

  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  auto function_client = std::move(function_client_result.value());

  fidl::Arena arena;
  auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 2);

  auto ep_endpoints1 = fidl::Endpoints<fendpoint::Endpoint>::Create();
  endpoints[0].direction = fdescriptor::wire::EndpointDirection::kIn;
  endpoints[0].ep_info = BulkEpInfo(arena);
  endpoints[0].max_packet_size = 512;
  endpoints[0].endpoint = std::move(ep_endpoints1.server);

  auto ep_endpoints2 = fidl::Endpoints<fendpoint::Endpoint>::Create();
  endpoints[1].direction = fdescriptor::wire::EndpointDirection::kOut;
  endpoints[1].ep_info = BulkEpInfo(arena);
  endpoints[1].max_packet_size = 512;
  endpoints[1].endpoint = std::move(ep_endpoints2.server);

  // This should fail because we only allow 1 allocation, but we requested 2.
  fidl::WireResult res = function_client->AllocResources(1, endpoints, {});
  ASSERT_TRUE(res.ok()) << res.FormatDescription();
  ASSERT_TRUE(res->is_error());
  EXPECT_EQ(res->error_value(), ZX_ERR_NO_RESOURCES);

  // Verify that the first endpoint (0x81) was freed.
  std::vector<uint8_t> freed_endpoints;
  dut().RunInEnvironmentTypeContext(
      [&](UsbPeripheralTestEnvironment& env) { freed_endpoints = env.dci().freed_endpoints(); });
  ASSERT_EQ(freed_endpoints.size(), 1u);
  EXPECT_EQ(freed_endpoints[0], 0x81);
}

// Stall EP0 if Control returns error
TEST_F(UsbPeripheralFunctionTest, ControlStallsEp0OnError) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  fidl::WireResult alloc_res = function_client->AllocResources(1, {}, {});
  ASSERT_TRUE(alloc_res.ok()) << alloc_res.status_string();
  ASSERT_TRUE(alloc_res->is_ok());
  uint8_t interface_num = alloc_res->value()->interface_nums[0];

  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = interface_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 0,
      .b_interface_class = 8,
      .b_interface_sub_class = 6,
      .b_interface_protocol = 80,
  };
  std::vector<uint8_t> descriptors(sizeof(intf_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));

  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(fake_function_endpoint));
  ASSERT_TRUE(configure_res.ok());
  ASSERT_OK(configure_res.value());

  ASSERT_OK(dci()->SetConnected(true).status());
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  fidl::Arena arena;
  std::vector<uint8_t> unused;

  // Configure first.
  fdescriptor::wire::UsbSetup setup;
  setup.bm_request_type = USB_DIR_OUT | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_SET_CONFIGURATION;
  setup.w_value = 1;  // Configuration 1
  setup.w_index = 0;
  setup.w_length = 0;

  fidl::WireUnownedResult config_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  EXPECT_TRUE(config_res.ok()) << config_res.FormatDescription();
  ASSERT_OK(config_res.value());
  fake_function->WaitUntilCalled();

  // Set control status to return error.
  fake_function->set_control_status(ZX_ERR_NOT_SUPPORTED);

  // Test Control via provided endpoint request (vendor request, directed to interface).
  setup.bm_request_type = USB_DIR_IN | USB_RECIP_INTERFACE | USB_TYPE_VENDOR;
  setup.b_request = 0xAA;
  setup.w_value = 0x01;
  setup.w_index = interface_num;  // Use interface_num to route to this function.
  setup.w_length = 3;

  fidl::WireUnownedResult control_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));

  // The control call should return error status.
  ASSERT_TRUE(control_res.ok()) << control_res.FormatDescription();
  ASSERT_TRUE(control_res->is_error());
  EXPECT_STATUS(control_res->error_value(), ZX_ERR_NOT_SUPPORTED);
}

// [INTF-2.2] Stall EP0 if SetConfigured returns error
TEST_F(UsbPeripheralFunctionTest, SetConfiguredStallsEp0OnError) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  fidl::WireResult alloc_res = function_client->AllocResources(1, {}, {});
  ASSERT_TRUE(alloc_res.ok()) << alloc_res.status_string();
  ASSERT_TRUE(alloc_res->is_ok());
  uint8_t interface_num = alloc_res->value()->interface_nums[0];

  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = interface_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 0,
      .b_interface_class = 8,
      .b_interface_sub_class = 6,
      .b_interface_protocol = 80,
  };
  std::vector<uint8_t> descriptors(sizeof(intf_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));

  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(fake_function_endpoint));
  ASSERT_TRUE(configure_res.ok());
  ASSERT_OK(configure_res.value());

  ASSERT_OK(dci()->SetConnected(true).status());
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Set SetConfigured status to return error.
  fake_function->set_set_configured_status(ZX_ERR_BAD_STATE);

  fidl::Arena arena;
  std::vector<uint8_t> unused;
  fdescriptor::wire::UsbSetup setup;
  setup.bm_request_type = USB_DIR_OUT | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_SET_CONFIGURATION;
  setup.w_value = 1;
  setup.w_index = 0;
  setup.w_length = 0;

  fidl::WireUnownedResult control_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));

  // The control call should return error status.
  ASSERT_TRUE(control_res.ok()) << control_res.FormatDescription();
  ASSERT_TRUE(control_res->is_error());
  EXPECT_STATUS(control_res->error_value(), ZX_ERR_BAD_STATE);
}

// [INTF-3.2] Stall EP0 if SetInterface returns error
TEST_F(UsbPeripheralFunctionTest, SetInterfaceStallsEp0OnError) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  fidl::WireResult alloc_res = function_client->AllocResources(1, {}, {});
  ASSERT_TRUE(alloc_res.ok()) << alloc_res.status_string();
  ASSERT_TRUE(alloc_res->is_ok());
  uint8_t interface_num = alloc_res->value()->interface_nums[0];

  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = interface_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 0,
      .b_interface_class = 8,
      .b_interface_sub_class = 6,
      .b_interface_protocol = 80,
  };
  std::vector<uint8_t> descriptors(sizeof(intf_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));

  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(fake_function_endpoint));
  ASSERT_TRUE(configure_res.ok());
  ASSERT_OK(configure_res.value());

  ASSERT_OK(dci()->SetConnected(true).status());
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  fidl::Arena arena;
  std::vector<uint8_t> unused;

  // Configure first.
  fdescriptor::wire::UsbSetup setup;
  setup.bm_request_type = USB_DIR_OUT | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_SET_CONFIGURATION;
  setup.w_value = 1;  // Configuration 1
  setup.w_index = 0;
  setup.w_length = 0;

  fidl::WireUnownedResult config_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  EXPECT_TRUE(config_res.ok()) << config_res.FormatDescription();
  ASSERT_OK(config_res.value());
  fake_function->WaitUntilCalled();

  // Set SetInterface status to return error.
  fake_function->set_set_interface_status(ZX_ERR_NOT_SUPPORTED);

  setup.bm_request_type = USB_DIR_OUT | USB_RECIP_INTERFACE | USB_TYPE_STANDARD;
  setup.b_request = USB_REQ_SET_INTERFACE;
  setup.w_value = 1;  // Alt setting 1
  setup.w_index = interface_num;
  setup.w_length = 0;

  fidl::WireUnownedResult control_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));

  // The control call should return error status.
  ASSERT_TRUE(control_res.ok()) << control_res.FormatDescription();
  ASSERT_TRUE(control_res->is_error());
  EXPECT_STATUS(control_res->error_value(), ZX_ERR_NOT_SUPPORTED);
}

TEST_F(UsbPeripheralFunctionTest, SetFeatureEndpointHaltCancelsAll) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::Arena arena;
  zx::result<uint8_t> ep_addr_result = ConfigureDefaultFunction(function_client, arena);
  ASSERT_OK(ep_addr_result);
  uint8_t ep_addr = ep_addr_result.value();

  ASSERT_OK(dci()->SetConnected(true).status());
  ExpectState(UsbPeripheral::DeviceState::kHostConnected);

  // Configure first to make the peripheral active.
  fdescriptor::wire::UsbSetup config_setup;
  config_setup.bm_request_type = USB_DIR_OUT | USB_RECIP_DEVICE | USB_TYPE_STANDARD;
  config_setup.b_request = USB_REQ_SET_CONFIGURATION;
  config_setup.w_value = 1;  // Configuration 1
  config_setup.w_index = 0;
  config_setup.w_length = 0;

  std::vector<uint8_t> unused;
  fidl::WireUnownedResult config_res =
      dci().buffer(arena)->Control(config_setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  EXPECT_TRUE(config_res.ok()) << config_res.FormatDescription();
  ASSERT_OK(config_res.value());

  auto cleanup = fit::defer([&]() {
    dut().RunInEnvironmentTypeContext([](UsbPeripheralTestEnvironment& env) {
      env.dci().fail_cancel_all_status_.store(ZX_ERR_NOT_SUPPORTED);
    });
  });

  // Verify SET_FEATURE ENDPOINT_HALT cleanly succeeds when DCI CancelAll returns ZX_OK.
  dut().RunInEnvironmentTypeContext(
      [](UsbPeripheralTestEnvironment& env) { env.dci().fail_cancel_all_status_.store(ZX_OK); });

  fdescriptor::wire::UsbSetup halt_setup;
  halt_setup.bm_request_type = USB_DIR_OUT | USB_RECIP_ENDPOINT | USB_TYPE_STANDARD;
  halt_setup.b_request = USB_REQ_SET_FEATURE;
  halt_setup.w_value = USB_ENDPOINT_HALT;
  halt_setup.w_index = ep_addr;
  halt_setup.w_length = 0;

  auto res1 =
      dci().buffer(arena)->Control(halt_setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(res1.ok()) << res1.FormatDescription();
  ASSERT_OK(res1.value());

  // Verify SET_FEATURE ENDPOINT_HALT cleanly succeeds when DCI CancelAll returns
  // ZX_ERR_IO_NOT_PRESENT (e.g. disconnected or unpowered controller during teardown).
  dut().RunInEnvironmentTypeContext([](UsbPeripheralTestEnvironment& env) {
    env.dci().fail_cancel_all_status_.store(ZX_ERR_IO_NOT_PRESENT);
  });

  auto res2 =
      dci().buffer(arena)->Control(halt_setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(res2.ok()) << res2.FormatDescription();
  ASSERT_OK(res2.value());

  // Verify SET_FEATURE ENDPOINT_HALT cleanly succeeds when DCI CancelAll returns ZX_ERR_NOT_FOUND
  // or ZX_ERR_BAD_STATE (unconfigured or inactive endpoint during teardown sweep).
  dut().RunInEnvironmentTypeContext([](UsbPeripheralTestEnvironment& env) {
    env.dci().fail_cancel_all_status_.store(ZX_ERR_NOT_FOUND);
  });

  auto res3 =
      dci().buffer(arena)->Control(halt_setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(res3.ok()) << res3.FormatDescription();
  ASSERT_OK(res3.value());

  dut().RunInEnvironmentTypeContext([](UsbPeripheralTestEnvironment& env) {
    env.dci().fail_cancel_all_status_.store(ZX_ERR_BAD_STATE);
  });

  auto res4 =
      dci().buffer(arena)->Control(halt_setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(res4.ok()) << res4.FormatDescription();
  ASSERT_OK(res4.value());

  // Test SET_FEATURE ENDPOINT_HALT on an unallocated endpoint address rejects with error
  // before invoking CancelAll or EndpointSetStall.
  uint8_t unallocated_ep_addr = (ep_addr == 0x81) ? 0x82 : 0x81;
  fdescriptor::wire::UsbSetup invalid_halt_setup = halt_setup;
  invalid_halt_setup.w_index = unallocated_ep_addr;

  auto res5 = dci().buffer(arena)->Control(invalid_halt_setup,
                                           fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(res5.ok()) << res5.FormatDescription();
  ASSERT_TRUE(res5->is_error());
  EXPECT_STATUS(res5->error_value(), ZX_ERR_NOT_FOUND);
}

// Test that CompleteFunctionsTeardown() cancels all allocated endpoints without
// sweeping unallocated endpoints.
TEST_F(UsbPeripheralFunctionTest, ClearFunctionsCancelsOnlyAllocatedEndpoints) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::Arena arena;
  zx::result<uint8_t> configure_result = ConfigureDefaultFunction(function_client, arena);
  ASSERT_OK(configure_result);
  uint8_t ep_addr = configure_result.value();

  dut().RunInEnvironmentTypeContext(
      [](UsbPeripheralTestEnvironment& env) { env.dci().clear_cancelled_endpoints(); });

  auto peripheral_client = ConnectPeripheral();
  ASSERT_OK(peripheral_client);

  auto clear_res = peripheral_client.value()->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

  dut().runtime().RunUntilIdle();
  ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);

  dut().RunInEnvironmentTypeContext([ep_addr](UsbPeripheralTestEnvironment& env) {
    auto cancelled = env.dci().cancelled_endpoints();
    // Only allocated function endpoints should be canceled; EP0 is reset in StopController().
    EXPECT_THAT(cancelled, testing::ElementsAre(ep_addr));
  });
}

TEST_F(UsbPeripheralFunctionTest, RejectConfigureWhileStopping) {
  // Connect peripheral so we can call ClearFunctions().
  zx::result peripheral_client_result = ConnectPeripheral();
  ASSERT_OK(peripheral_client_result);
  auto peripheral_client = std::move(peripheral_client_result.value());

  // Connect function and allocate resources.
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  // Intercept SetConfigured(false) to stall teardown in kStopping state.
  std::mutex completer_lock;
  std::optional<FakeUsbFunction::SetConfiguredCompleterAsync> saved_completer;
  fake_function->set_on_set_configured_async(
      [&](bool configured, FakeUsbFunction::SetConfiguredCompleterAsync completer) {
        if (!configured) {
          std::lock_guard lock(completer_lock);
          saved_completer = std::move(completer);
        } else {
          completer.ReplySuccess();
        }
      });

  fidl::WireResult alloc_res = function_client->AllocResources(1, {}, {});
  ASSERT_TRUE(alloc_res.ok()) << alloc_res.status_string();
  ASSERT_TRUE(alloc_res->is_ok()) << zx_status_get_string(alloc_res->error_value());
  uint8_t interface_num = alloc_res->value()->interface_nums[0];

  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = interface_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 0,
      .b_interface_class = 8,
      .b_interface_sub_class = 6,
      .b_interface_protocol = 80,
      .i_interface = 0,
  };
  std::vector<uint8_t> descriptors(sizeof(intf_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));

  // Configure first to set up the topology.
  {
    fidl::WireResult result = function_client->Configure(
        fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
        std::move(fake_function_endpoint));
    ASSERT_TRUE(result.ok()) << result.FormatDescription();
    ASSERT_TRUE(result->is_ok());
  }
  ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);

  // Clear functions asynchronously on a background thread. This is necessary because
  // ClearFunctions() is a synchronous FIDL call that blocks waiting for SetConfigured(false)
  // to complete, which is held open by saved_completer. Calling it on a background thread
  // keeps the main test thread free to issue a concurrent Configure() call and verify it is
  // rejected with ZX_ERR_BAD_STATE while in kStopping, before completing saved_completer.
  auto clear_promise = std::async(std::launch::async, [&]() {
    auto clear_res = peripheral_client->ClearFunctions();
    EXPECT_TRUE(clear_res.ok()) << clear_res.FormatDescription();
  });
  // completer_cleanup is declared after clear_promise so that upon test failure or exception,
  // its destructor runs before ~future() blocks waiting on ClearFunctions().
  auto completer_cleanup = fit::defer([&]() {
    fake_function->set_on_set_configured_async(nullptr);
    std::optional<FakeUsbFunction::SetConfiguredCompleterAsync> completer_to_run;
    {
      std::lock_guard lock(completer_lock);
      completer_to_run = std::move(saved_completer);
      saved_completer.reset();
    }
    if (completer_to_run.has_value()) {
      completer_to_run->ReplySuccess();
    }
  });

  // Wait until FakeUsbFunction receives SetConfigured(false).
  dut().runtime().RunUntil([&]() {
    std::lock_guard lock(completer_lock);
    return saved_completer.has_value();
  });

  // The state is now kStopping (since unconfiguring is asynchronous).
  WaitUntilState(UsbPeripheral::DeviceState::kStopping);

  // While in kStopping, attempt to call Configure.
  // Note: we need new endpoints for a configure call.
  zx::result endpoints = fidl::CreateEndpoints<ffunction::UsbFunctionInterface>();
  ASSERT_OK(endpoints);
  auto second_fake = std::make_shared<FakeUsbFunction>();
  second_fake->Bind(dut().runtime().StartBackgroundDispatcher(), std::move(endpoints->server));

  fidl::WireResult second_configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(endpoints->client));

  ASSERT_TRUE(second_configure_res.ok()) << second_configure_res.FormatDescription();
  // Assert that configuration is rejected with ZX_ERR_BAD_STATE.
  EXPECT_TRUE(second_configure_res->is_error());
  EXPECT_EQ(second_configure_res->error_value(), ZX_ERR_BAD_STATE);

  // Complete the SetConfigured(false) call to resume/finish teardown.
  {
    std::lock_guard lock(completer_lock);
    ASSERT_TRUE(saved_completer.has_value());
  }
  completer_cleanup.call();

  // Wait for the ClearFunctions call to complete on the background thread.
  clear_promise.get();
  dut().runtime().RunUntilIdle();
}

// Test that calling SetConfigured(false) on a function whose interface is not registered or
// already unbound succeeds as a no-op with ZX_OK rather than failing with ZX_ERR_BAD_STATE.
TEST_F(UsbPeripheralFunctionTest, UnconfiguredFunctionUnconfigureSucceedsAsNoOp) {
  // Add a function without calling Configure() or binding an interface.
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);

  // Clear functions while the function is not yet bound to an interface.
  // This triggers SetConfigured(false) in UsbPeripheral::ClearFunctions().
  auto peripheral_client = ConnectPeripheral();
  ASSERT_OK(peripheral_client);

  auto clear_res = peripheral_client.value()->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

  dut().runtime().RunUntilIdle();
  ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);
}

// Test that calling SetConfigured(false) on a function whose interface unbinds or closes
// concurrently (e.g. during disconnect/suspend when the child function driver unbinds)
// succeeds cleanly with ZX_OK rather than failing with ZX_ERR_CANCELED.
TEST_F(UsbPeripheralFunctionTest, FunctionUnbindDuringDeconfigureSucceedsCleanly) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  fidl::Arena arena;
  zx::result<uint8_t> configure_result = ConfigureDefaultFunction(function_client, arena);
  ASSERT_OK(configure_result);

  // Now simulate the function unbinding its interface while ClearFunctions (SetConfigured(false))
  // runs.
  auto peripheral_client = ConnectPeripheral();
  ASSERT_OK(peripheral_client);

  // Close the function client channel, which triggers empty_set_handler and AsyncTeardown on
  // function_intf_.
  function_client = {};

  auto clear_res = peripheral_client.value()->ClearFunctions();
  ASSERT_TRUE(clear_res.ok()) << clear_res.FormatDescription();

  dut().runtime().RunUntilIdle();
  ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);
}

TEST_F(UsbPeripheralFunctionTest, EndpointRecipientControlRequestsHandling) {
  zx::result function_client_result = ConnectFunction();
  ASSERT_OK(function_client_result);
  fidl::WireSyncClient<ffunction::UsbFunction> function_client =
      std::move(function_client_result.value());

  zx::result fake_function_result = BindFakeFunction();
  ASSERT_OK(fake_function_result);
  auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

  fidl::Arena arena;
  auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 1);
  auto ep_endpoints = fidl::Endpoints<fendpoint::Endpoint>::Create();
  endpoints[0].direction = fdescriptor::wire::EndpointDirection::kIn;
  endpoints[0].ep_info = BulkEpInfo(arena);
  endpoints[0].max_packet_size = 512;
  endpoints[0].endpoint = std::move(ep_endpoints.server);

  fidl::WireResult alloc_res = function_client->AllocResources(1, endpoints, {});
  ASSERT_TRUE(alloc_res.ok()) << alloc_res.status_string();
  ASSERT_TRUE(alloc_res->is_ok()) << zx_status_get_string(alloc_res->error_value());
  uint8_t interface_num = alloc_res->value()->interface_nums[0];
  uint8_t allocated_ep = alloc_res->value()->endpoint_addrs[0];

  usb_interface_descriptor_t intf_desc = {
      .b_length = sizeof(usb_interface_descriptor_t),
      .b_descriptor_type = USB_DT_INTERFACE,
      .b_interface_number = interface_num,
      .b_alternate_setting = 0,
      .b_num_endpoints = 1,
      .b_interface_class = 8,
      .b_interface_sub_class = 6,
      .b_interface_protocol = 80,
      .i_interface = 0,
  };

  std::vector<uint8_t> descriptors(sizeof(intf_desc));
  memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));

  fidl::WireResult configure_res = function_client->Configure(
      fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
      std::move(fake_function_endpoint));
  ASSERT_TRUE(configure_res.ok()) << configure_res.FormatDescription();
  ASSERT_OK(configure_res.value());

  ExpectControllerStarted(true);
  ASSERT_OK(dci()->SetConnected(true).status());

  std::vector<uint8_t> unused;

  // Set configuration 1 to move device out of unconfigured state.
  fdescriptor::wire::UsbSetup setup = {
      .bm_request_type = USB_DIR_OUT | USB_RECIP_DEVICE | USB_TYPE_STANDARD,
      .b_request = USB_REQ_SET_CONFIGURATION,
      .w_value = 1,
      .w_index = 0,
      .w_length = 0,
  };
  fidl::WireUnownedResult config_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(config_res.ok()) << config_res.FormatDescription();
  ASSERT_OK(config_res.value());

  // 1. GET_STATUS on EP0 OUT (0x00) should succeed and return status 0.
  setup = {
      .bm_request_type = USB_DIR_IN | USB_RECIP_ENDPOINT | USB_TYPE_STANDARD,
      .b_request = USB_REQ_GET_STATUS,
      .w_value = 0,
      .w_index = 0x00,
      .w_length = 2,
  };
  auto ep0_out_status_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(ep0_out_status_res.ok()) << ep0_out_status_res.FormatDescription();
  ASSERT_OK(ep0_out_status_res.value());
  ASSERT_EQ(ep0_out_status_res.value()->read.size(), 2u);
  EXPECT_EQ(ep0_out_status_res.value()->read[0], 0);
  EXPECT_EQ(ep0_out_status_res.value()->read[1], 0);

  // 2. GET_STATUS on EP0 IN (0x80) should succeed and return status 0 (not ZX_ERR_NOT_FOUND).
  setup.w_index = 0x80;
  auto ep0_in_status_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(ep0_in_status_res.ok()) << ep0_in_status_res.FormatDescription();
  ASSERT_OK(ep0_in_status_res.value());
  ASSERT_EQ(ep0_in_status_res.value()->read.size(), 2u);
  EXPECT_EQ(ep0_in_status_res.value()->read[0], 0);
  EXPECT_EQ(ep0_in_status_res.value()->read[1], 0);

  // 3. SET_FEATURE(ENDPOINT_HALT) on unallocated endpoint (e.g. 0x02) should return
  // ZX_ERR_NOT_FOUND without invoking UsbDciCancelAll or UsbDciEndpointSetStall.
  setup = {
      .bm_request_type = USB_DIR_OUT | USB_RECIP_ENDPOINT | USB_TYPE_STANDARD,
      .b_request = USB_REQ_SET_FEATURE,
      .w_value = USB_ENDPOINT_HALT,
      .w_index = 0x02,
      .w_length = 0,
  };
  auto unalloc_set_feature_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(unalloc_set_feature_res.ok()) << unalloc_set_feature_res.FormatDescription();
  ASSERT_TRUE(unalloc_set_feature_res.value().is_error());
  EXPECT_EQ(unalloc_set_feature_res.value().error_value(), ZX_ERR_NOT_FOUND);

  // 4. SET_FEATURE(ENDPOINT_HALT) on allocated endpoint should succeed.
  setup.w_index = allocated_ep;
  auto alloc_set_feature_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(alloc_set_feature_res.ok()) << alloc_set_feature_res.FormatDescription();
  ASSERT_OK(alloc_set_feature_res.value());

  // Verify GET_STATUS on allocated endpoint reports halted (status == 1).
  setup = {
      .bm_request_type = USB_DIR_IN | USB_RECIP_ENDPOINT | USB_TYPE_STANDARD,
      .b_request = USB_REQ_GET_STATUS,
      .w_value = 0,
      .w_index = allocated_ep,
      .w_length = 2,
  };
  auto alloc_status_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(alloc_status_res.ok()) << alloc_status_res.FormatDescription();
  ASSERT_OK(alloc_status_res.value());
  ASSERT_EQ(alloc_status_res.value()->read.size(), 2u);
  EXPECT_EQ(alloc_status_res.value()->read[0], 1);
  EXPECT_EQ(alloc_status_res.value()->read[1], 0);

  // 5. CLEAR_FEATURE(ENDPOINT_HALT) on allocated endpoint should succeed.
  setup = {
      .bm_request_type = USB_DIR_OUT | USB_RECIP_ENDPOINT | USB_TYPE_STANDARD,
      .b_request = USB_REQ_CLEAR_FEATURE,
      .w_value = USB_ENDPOINT_HALT,
      .w_index = allocated_ep,
      .w_length = 0,
  };
  auto alloc_clear_feature_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(alloc_clear_feature_res.ok()) << alloc_clear_feature_res.FormatDescription();
  ASSERT_OK(alloc_clear_feature_res.value());

  // Verify GET_STATUS on allocated endpoint reports cleared (status == 0).
  setup = {
      .bm_request_type = USB_DIR_IN | USB_RECIP_ENDPOINT | USB_TYPE_STANDARD,
      .b_request = USB_REQ_GET_STATUS,
      .w_value = 0,
      .w_index = allocated_ep,
      .w_length = 2,
  };
  alloc_status_res =
      dci().buffer(arena)->Control(setup, fidl::VectorView<uint8_t>::FromExternal(unused));
  ASSERT_TRUE(alloc_status_res.ok()) << alloc_status_res.FormatDescription();
  ASSERT_OK(alloc_status_res.value());
  ASSERT_EQ(alloc_status_res.value()->read.size(), 2u);
  EXPECT_EQ(alloc_status_res.value()->read[0], 0);
  EXPECT_EQ(alloc_status_res.value()->read[1], 0);
}

}  // namespace
}  // namespace usb_peripheral::test
