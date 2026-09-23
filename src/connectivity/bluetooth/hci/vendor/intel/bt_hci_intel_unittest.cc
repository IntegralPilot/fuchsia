// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.bluetooth/cpp/fidl.h>
#include <fidl/fuchsia.hardware.bluetooth/cpp/wire.h>
#include <fuchsia/hardware/usb/cpp/banjo.h>
#include <lib/async/cpp/wait.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>

#include "device.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"
#include "src/storage/lib/vfs/cpp/vmo_file.h"

namespace bt_hci_intel {
namespace {
namespace fhbt = fuchsia_hardware_bluetooth;

// Firmware binaries are a sequence of HCI commands containing the firmware as payloads. For
// testing, we use 1 HCI command with a 1 byte payload.
const std::vector<uint8_t> kFirmware = {
    0x01, 0x02,  // arbitrary "firmware opcode"
    0x01,        // parameter_total_size
    0x03         // payload
};
const char* kFirmwarePath = "ibt-0041-0041.sfi";

const std::array<uint8_t, 6> kResetCommandCompleteEvent = {
    static_cast<uint8_t>(pw::bluetooth::emboss::EventCode::COMMAND_COMPLETE),
    0x04,  // parameter_total_size
    0x01,  // num_hci_command_packets
    0x00,
    0x00,  // command opcode (hardcoded for simplicity since this isn't checked by the driver)
    0x00,  // return_code (success)
};

// clang-format off
const std::array<uint8_t, 109> kReadVersionTlvCompleteEvent = {
    static_cast<uint8_t>(pw::bluetooth::emboss::EventCode::COMMAND_COMPLETE),
    0x6b,  // parameter_total_size
    0x01,  // num_hci_command_packets
    // command opcode
    0x05, 0xfc,
    // return_code (success)
    0x00,
    // Tlvs
    0x10, 0x04, 0x10, 0x04, 0x40, 0x00, 0x11, 0x04, 0x10, 0x04, 0x40, 0x00, 0x12, 0x04, 0x00, 0x37,
    0x17, 0x00, 0x13, 0x04, 0x20, 0x37, 0x12, 0x00, 0x15, 0x02, 0x13, 0x04, 0x16, 0x02, 0x00, 0x00,
    0x17, 0x02, 0x87, 0x80, 0x18, 0x02, 0x32, 0x00, 0x1c, 0x01, 0x03, 0x1d, 0x02, 0x30, 0x17, 0x1e,
    0x01, 0x01, 0x1f, 0x04, 0x3c, 0x26, 0x01, 0x00, 0x20, 0x01, 0x06, 0x21, 0x01, 0x06, 0x22, 0x01,
    0xa0, 0x23, 0x01, 0x00, 0x24, 0x02, 0x02, 0x00, 0x25, 0x02, 0x3c, 0x36, 0x26, 0x02, 0x3c, 0x36,
    0x2a, 0x01, 0x01, 0x2b, 0x01, 0x01, 0x32, 0x04, 0x58, 0xc5, 0xba, 0x23, 0x33, 0x01, 0x00, 0x34,
    0x00, 0x35, 0x04, 0x00, 0x00, 0x00,
    // End
};
// clang-format on

class FakeUsbServer : public ddk::UsbProtocol<FakeUsbServer> {
 public:
  FakeUsbServer() = default;
  zx_status_t UsbControlOut(uint8_t request_type, uint8_t request, uint16_t value, uint16_t index,
                            int64_t timeout, const uint8_t* write_buffer, size_t write_size) {
    return ZX_OK;
  }
  zx_status_t UsbControlIn(uint8_t request_type, uint8_t request, uint16_t value, uint16_t index,
                           int64_t timeout, uint8_t* out_read_buffer, size_t read_size,
                           size_t* out_read_actual) {
    return ZX_OK;
  }
  void UsbRequestQueue(usb_request_t* usb_request,
                       const usb_request_complete_callback_t* complete_cb) {}
  usb_speed_t UsbGetSpeed() { return 0; }
  zx_status_t UsbSetInterface(uint8_t interface_number, uint8_t alt_setting) { return ZX_OK; }
  uint8_t UsbGetConfiguration() { return 0; }
  zx_status_t UsbSetConfiguration(uint8_t configuration) { return 0; }
  zx_status_t UsbEnableEndpoint(const usb_endpoint_descriptor_t* ep_desc,
                                const usb_ss_ep_comp_descriptor_t* ss_com_desc, bool enable) {
    return ZX_OK;
  }
  zx_status_t UsbResetEndpoint(uint8_t ep_address) { return ZX_OK; }
  zx_status_t UsbResetDevice() { return ZX_OK; }
  size_t UsbGetMaxTransferSize(uint8_t ep_address) { return 0; }
  uint32_t UsbGetDeviceId() { return 0; }
  void UsbGetDeviceDescriptor(usb_device_descriptor_t* out_desc) {
    // AX210 product id
    out_desc->id_product = 0x0032;
  }
  zx_status_t UsbGetConfigurationDescriptorLength(uint8_t configuration, size_t* out_length) {
    return ZX_OK;
  }
  zx_status_t UsbGetConfigurationDescriptor(uint8_t configuration, uint8_t* out_desc_buffer,
                                            size_t desc_size, size_t* out_desc_actual) {
    return ZX_OK;
  }
  size_t UsbGetDescriptorsLength() { return 0; }
  void UsbGetDescriptors(uint8_t* out_descs_buffer, size_t descs_size, size_t* out_descs_actual) {}
  zx_status_t UsbGetStringDescriptor(uint8_t desc_id, uint16_t lang_id, uint16_t* out_lang_id,
                                     uint8_t* out_string_buffer, size_t string_size,
                                     size_t* out_string_actual) {
    return ZX_OK;
  }
  zx_status_t UsbCancelAll(uint8_t ep_address) { return ZX_OK; }
  uint64_t UsbGetCurrentFrame() { return 0; }
  size_t UsbGetRequestSize() { return 0; }

  usb_protocol_t proto() { return {.ops = &usb_protocol_ops_, .ctx = this}; }
};

class FakeBtHciServer : public fidl::Server<fhbt::HciTransport> {
 public:
  FakeBtHciServer() = default;

  fhbt::HciService::InstanceHandler GetHciInstanceHandler() {
    return fhbt::HciService::InstanceHandler({
        .hci_transport = hci_transport_binding_.CreateHandler(
            this, fdf::Dispatcher::GetCurrent()->async_dispatcher(), fidl::kIgnoreBindingClosure),
    });
  }

  // fhbt::HciTransport request handler implementations:
  void Send(SendRequest& request, SendCompleter::Sync& completer) override {
    std::vector<uint8_t> reply;
    if (!reset_) {
      reply = std::vector<uint8_t>(
          kResetCommandCompleteEvent.data(),
          kResetCommandCompleteEvent.data() + kResetCommandCompleteEvent.size());
      reset_ = true;
    } else {
      reply = std::vector<uint8_t>(
          kReadVersionTlvCompleteEvent.data(),
          kReadVersionTlvCompleteEvent.data() + kReadVersionTlvCompleteEvent.size());
    }

    hci_transport_binding_.ForEachBinding(
        [&](const fidl::ServerBinding<fhbt::HciTransport>& binding) {
          auto received_packet = fhbt::ReceivedPacket::WithEvent(reply);
          fit::result<fidl::OneWayError> result =
              fidl::SendEvent(binding)->OnReceive(received_packet);
          ASSERT_FALSE(result.is_error());
        });

    completer.Reply();
  }

  void AckReceive(AckReceiveCompleter::Sync& completer) override {}
  void ConfigureSco(
      fidl::Server<fhbt::HciTransport>::ConfigureScoRequest& request,
      fidl::Server<fhbt::HciTransport>::ConfigureScoCompleter::Sync& completer) override {}
  void handle_unknown_method(::fidl::UnknownMethodMetadata<fhbt::HciTransport> metadata,
                             ::fidl::UnknownMethodCompleter::Sync& completer) override {
    ZX_PANIC("Unknown method in HciTransport requests");
  }

 private:
  // Mark the state of driver initialization. |reset_| is true means that the driver has sent the
  // reset command and |FakeBtHciServer| should expect a ReadVersionTlv command.
  bool reset_{false};
  fidl::ServerBindingGroup<fhbt::HciTransport> hci_transport_binding_;
};

class TestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    auto dir_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    firmware_server_.SetDispatcher(fdf::Dispatcher::GetCurrent()->async_dispatcher());
    // Serve our firmware directory (will start serving FIDL requests on dir_endpoints with
    // dispatcher on previous line)
    ZX_ASSERT(firmware_server_.ServeDirectory(firmware_dir_, std::move(dir_endpoints.server)) ==
              ZX_OK);
    // Attach the firmware directory endpoint to "pkg/lib"
    ZX_ASSERT(to_driver_vfs.component()
                  .AddDirectoryAt(std::move(dir_endpoints.client), "pkg/lib", "firmware")
                  .is_ok());

    // Create vmo for firmware file.
    zx::vmo vmo;
    zx::vmo::create(4096, 0, &vmo);
    vmo.write(kFirmware.data(), 0, kFirmware.size());
    vmo.set_prop_content_size(kFirmware.size());

    //  Create firmware file, and add it to the "firmware" directory we added under pkg/lib.
    fbl::RefPtr<fs::VmoFile> firmware_file =
        fbl::MakeRefCounted<fs::VmoFile>(std::move(vmo), kFirmware.size());
    ZX_ASSERT(firmware_dir_->AddEntry(kFirmwarePath, firmware_file) == ZX_OK);

    compat::DeviceServer::BanjoConfig banjo_config{ZX_PROTOCOL_USB};
    banjo_config.callbacks[ZX_PROTOCOL_USB] = [this]() {
      return compat::DeviceServer::GenericProtocol{.ops = usb_server_.proto().ops,
                                                   .ctx = usb_server_.proto().ctx};
    };

    auto result =
        to_driver_vfs.AddService<fhbt::HciService>(bt_hci_server_.GetHciInstanceHandler());
    EXPECT_TRUE(result.is_ok());

    device_server_.Initialize(component::kDefaultInstance, std::nullopt, std::move(banjo_config));
    return zx::make_result(
        device_server_.Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(), &to_driver_vfs));
  }

  FakeUsbServer usb_server_;
  FakeBtHciServer bt_hci_server_;
  compat::DeviceServer device_server_;
  fbl::RefPtr<fs::PseudoDir> firmware_dir_ = fbl::MakeRefCounted<fs::PseudoDir>();
  fs::SynchronousVfs firmware_server_;
};

class FixtureConfig {
 public:
  using DriverType = Device;
  using EnvironmentType = TestEnvironment;
};

class BtHciIntelTest : public ::testing::Test {
 public:
  BtHciIntelTest() = default;

  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(BtHciIntelTest, LifecycleTest) {}

TEST_F(BtHciIntelTest, ConnectVendorService) {
  zx::result<fidl::ClientEnd<fhbt::Vendor>> client_end =
      driver_test().Connect<fhbt::Service::Vendor>();
  ASSERT_TRUE(client_end.is_ok());
}

}  // namespace
}  // namespace bt_hci_intel
