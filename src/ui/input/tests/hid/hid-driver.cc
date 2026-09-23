// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <fidl/fuchsia.hardware.hidctl/cpp/wire.h>
#include <fidl/fuchsia.hardware.input/cpp/wire.h>
#include <fidl/fuchsia.input.report/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/ddk/platform-defs.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/hid/boot.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/syslog/cpp/macros.h>
#include <stdio.h>
#include <stdlib.h>
#include <zircon/syscalls.h>

#include <fbl/unique_fd.h>
#include <src/ui/testing/util/fidl_cpp_helpers.h>
#include <zxtest/zxtest.h>

namespace {

class HidDriverTest : public zxtest::Test {
 protected:
  void SetUp() override {
    loop_.StartThread();

    // Create and build the realm.
    auto realm_builder = component_testing::RealmBuilder::Create();
    driver_test_realm::Setup(realm_builder, loop_.dispatcher(), {}, {});
    realm_ =
        std::make_unique<component_testing::RealmRoot>(realm_builder.Build(loop_.dispatcher()));

    // Start DriverTestRealm.
    zx::result<> boot_result = driver_test_realm::WaitForBootup(*realm_);
    ASSERT_EQ(ZX_OK, boot_result.status_value());

    auto node = driver_test_realm::WaitForNode(*realm_, "dev.sys.test.hidctl");
    ASSERT_TRUE(node.is_ok());

    auto exposed = realm_->component().CloneExposedDir();
    ASSERT_OK(fdio_fd_create(exposed.TakeChannel().release(), exposed_fd_.reset_and_get_address()));

    // TODO(https://fxbug.dev/377735979): Connect using a different mechanism.
    // Wait for HidCtl to be created.
    zx::result hidctl_channel =
        device_watcher::RecursiveWaitForFile(exposed_fd_.get(), "dev-topological/sys/test/hidctl");
    ASSERT_OK(hidctl_channel.status_value());

    fidl::ClientEnd<fuchsia_hardware_hidctl::Device> client_end(std::move(hidctl_channel.value()));
    hidctl_client_.Bind(std::move(client_end));
  }

  void TearDown() override { driver_test_realm::ShutdownRealm(*realm_); }

  template <typename Protocol>
  zx::result<fidl::ClientEnd<Protocol>> WaitForFirstDevice(const std::string& path) {
    // Open specified `path` in our exposed directory.
    fbl::unique_fd dev_fd;
    if (zx_status_t status = fdio_open3_fd_at(
            exposed_fd_.get(), path.c_str(),
            uint64_t{fuchsia_io::wire::kPermReadable | fuchsia_io::wire::Flags::kProtocolDirectory},
            dev_fd.reset_and_get_address());
        status != ZX_OK) {
      return zx::error(status);
    }
    fdio_cpp::UnownedFdioCaller caller(dev_fd);
    zx::result watch_result =
        device_watcher::WatchDirectoryForItems<zx::result<fidl::ClientEnd<Protocol>>>(
            caller.directory(), [&caller](std::string_view devpath) {
              return component::ConnectAt<Protocol>(caller.directory(), devpath);
            });
    if (watch_result.is_error()) {
      return watch_result.take_error();
    }
    return std::move(*watch_result);
  }

  async::Loop loop_ = async::Loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  fbl::unique_fd exposed_fd_;
  fidl::WireSyncClient<fuchsia_hardware_hidctl::Device> hidctl_client_;
  std::unique_ptr<component_testing::RealmRoot> realm_;
  fidl::SyncClient<fuchsia_driver_test::Realm> driver_test_realm_;
};

const uint8_t kBootMouseReportDesc[50] = {
    0x05, 0x01,  // Usage Page (Generic Desktop Ctrls)
    0x09, 0x02,  // Usage (Mouse)
    0xA1, 0x01,  // Collection (Application)
    0x09, 0x01,  //   Usage (Pointer)
    0xA1, 0x00,  //   Collection (Physical)
    0x05, 0x09,  //     Usage Page (Button)
    0x19, 0x01,  //     Usage Minimum (0x01)
    0x29, 0x03,  //     Usage Maximum (0x03)
    0x15, 0x00,  //     Logical Minimum (0)
    0x25, 0x01,  //     Logical Maximum (1)
    0x95, 0x03,  //     Report Count (3)
    0x75, 0x01,  //     Report Size (1)
    0x81, 0x02,  //     Input (Data,Var,Abs,No Wrap,Linear,No Null Position)
    0x95, 0x01,  //     Report Count (1)
    0x75, 0x05,  //     Report Size (5)
    0x81, 0x03,  //     Input (Const,Var,Abs,No Wrap,Linear,No Null Position)
    0x05, 0x01,  //     Usage Page (Generic Desktop Ctrls)
    0x09, 0x30,  //     Usage (X)
    0x09, 0x31,  //     Usage (Y)
    0x15, 0x81,  //     Logical Minimum (-127)
    0x25, 0x7F,  //     Logical Maximum (127)
    0x75, 0x08,  //     Report Size (8)
    0x95, 0x02,  //     Report Count (2)
    0x81, 0x06,  //     Input (Data,Var,Rel,No Wrap,Linear,No Null Position)
    0xC0,        //   End Collection
    0xC0,        // End Collection
};

TEST_F(HidDriverTest, BootMouseTest) {
  // Create a fake mouse device
  fuchsia_hardware_hidctl::wire::HidCtlConfig config = {};
  config.dev_num = 5;
  config.boot_device = false;
  config.dev_class = 0;
  auto result = hidctl_client_->MakeHidDevice(
      config, fidl::VectorView<uint8_t>::FromExternal(const_cast<uint8_t*>(kBootMouseReportDesc),
                                                      std::size(kBootMouseReportDesc)));
  ASSERT_OK(result.status());
  zx::socket hidctl_socket = std::move(result.value().report_socket);

  fbl::unique_fd fd_device;

  // Wait for input-report driver to bind to avoid binding to a device which is being torn down.
  ASSERT_OK(
      WaitForFirstDevice<fuchsia_input_report::InputDevice>("dev-topological/class/input-report")
          .status_value());
  // Open the /dev/class/input/ device created by MakeHidDevice.
  zx::result input_client =
      WaitForFirstDevice<fuchsia_hardware_input::Controller>("dev-topological/class/input");
  ASSERT_OK(input_client.status_value());

  // Open a FIDL channel to the HID device
  auto [device, server] = fidl::Endpoints<fuchsia_hardware_input::Device>::Create();

  ASSERT_OK(fidl::WireCall(input_client.value())->OpenSession(std::move(server)));

  fidl::WireSyncClient client(std::move(device));
  // Make a synchronous FIDL call to ensure the session connection is alive.
  ASSERT_OK(client->Query());

  // Send a single mouse report
  hid_boot_mouse_report_t mouse_report = {
      .rel_x = 50,
      .rel_y = 100,
  };
  ASSERT_OK(hidctl_socket.write(0, &mouse_report, sizeof(mouse_report), nullptr));

  // Get the report event.
  zx::event report_event;
  {
    auto result = client->GetReportsEvent();
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    report_event = std::move(result.value()->event);
  }

  // Check that the report comes through
  {
    report_event.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite(), nullptr);

    hid_boot_mouse_report_t test_report = {};

    auto response = client->ReadReport();
    ASSERT_TRUE(response.ok());
    ASSERT_TRUE(response->is_ok());
    ASSERT_EQ(response.value()->buf().size(), sizeof(test_report));

    memcpy(&test_report, response.value()->buf().data(), sizeof(test_report));
    ASSERT_EQ(mouse_report.rel_x, test_report.rel_x);
    ASSERT_EQ(mouse_report.rel_y, test_report.rel_y);
  }

  // Check that report descriptors match completely
  {
    auto response = client->GetReportDesc();
    ASSERT_OK(response.status());
    ASSERT_EQ(response.value().desc.size(), sizeof(kBootMouseReportDesc));
    for (size_t i = 0; i < sizeof(kBootMouseReportDesc); i++) {
      if (kBootMouseReportDesc[i] != response.value().desc[i]) {
        printf("Index %ld of the report descriptor doesn't match\n", i);
      }
      EXPECT_EQ(kBootMouseReportDesc[i], response.value().desc[i]);
    }
  }
}

TEST_F(HidDriverTest, BootMouseTestInputReport) {
  // Create a fake mouse device
  fuchsia_hardware_hidctl::wire::HidCtlConfig config;
  config.dev_num = 5;
  config.boot_device = false;
  config.dev_class = 0;
  auto result = hidctl_client_->MakeHidDevice(
      config, fidl::VectorView<uint8_t>::FromExternal(const_cast<uint8_t*>(kBootMouseReportDesc),
                                                      std::size(kBootMouseReportDesc)));
  ASSERT_OK(result.status());
  zx::socket hidctl_socket = std::move(result.value().report_socket);

  // Open the /dev/class/input/ device created by MakeHidDevice.
  zx::result input_client =
      WaitForFirstDevice<fuchsia_input_report::InputDevice>("dev-topological/class/input-report");
  ASSERT_OK(input_client.status_value());
  fidl::WireSyncClient client(std::move(input_client.value()));

  auto reader_endpoints = fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
  auto get_reader_result = client->GetInputReportsReaderV2(
      std::move(reader_endpoints.server),
      static_cast<uint16_t>(fuchsia_input_report::wire::kMaxDeviceReportCount));
  ASSERT_OK(get_reader_result.status());
  auto reader = fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2>(
      std::move(reader_endpoints.client));

  // Check the Descriptor.
  {
    auto response = client->GetDescriptor();
    ASSERT_OK(response.status());
    ASSERT_TRUE(response.value().descriptor.has_mouse());
    ASSERT_TRUE(response.value().descriptor.mouse().has_input());
    ASSERT_TRUE(response.value().descriptor.mouse().input().has_movement_x());
    ASSERT_TRUE(response.value().descriptor.mouse().input().has_movement_y());
  }

  // Send a single mouse report
  hid_boot_mouse_report_t mouse_report = {};
  mouse_report.rel_x = 50;
  mouse_report.rel_y = 100;
  ASSERT_OK(hidctl_socket.write(0, &mouse_report, sizeof(mouse_report), nullptr));

  // Get the mouse InputReport via InputReportsReaderV2 event.
  struct MouseMovement {
    int64_t movement_x = 0;
    int64_t movement_y = 0;
  };
  std::vector<MouseMovement> received_movements;
  uint64_t last_stamp = 0;

  class EventHandler
      : public fidl::WireSyncEventHandler<fuchsia_input_report::InputReportsReaderV2> {
   public:
    EventHandler(std::vector<MouseMovement>& movements, uint64_t& stamp)
        : movements_(movements), stamp_(stamp) {}

    void OnInputReports(fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*
                            event) override {
      stamp_ = event->last_report_stamp;
      for (const auto& report : event->reports) {
        if (report.has_mouse() && report.mouse().has_movement_x() &&
            report.mouse().has_movement_y()) {
          movements_.push_back({
              .movement_x = report.mouse().movement_x(),
              .movement_y = report.mouse().movement_y(),
          });
        }
      }
    }
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_input_report::InputReportsReaderV2> metadata) override {}

   private:
    std::vector<MouseMovement>& movements_;
    uint64_t& stamp_;
  };

  EventHandler handler(received_movements, last_stamp);
  ASSERT_OK(reader.HandleOneEvent(handler).status());
  ASSERT_OK(reader->AcknowledgeReports(last_stamp).status());

  ASSERT_EQ(1, received_movements.size());
  ASSERT_EQ(50, received_movements[0].movement_x);
  ASSERT_EQ(100, received_movements[0].movement_y);
}

}  // namespace
