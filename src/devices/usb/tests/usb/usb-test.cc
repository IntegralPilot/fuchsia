// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.usb.tester/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <unistd.h>

#include <filesystem>

#include <zxtest/zxtest.h>

namespace {

constexpr std::string_view kUsbTesterDir = "/dev/class/usb-tester";
constexpr std::string_view kUsbDeviceDir = "/dev/class/usb-device";

constexpr double ISOCH_MIN_PASS_PERCENT = 80;
constexpr uint64_t ISOCH_MIN_PACKETS = 10;

zx_status_t check_xhci_root_hubs() {
  uint8_t count = 0;
  for (auto const& dir_entry : std::filesystem::directory_iterator{kUsbDeviceDir}) {
    std::ignore = dir_entry;
    count++;
  }
  // TODO(ravoorir): Use FIDL apis to read the descriptors
  // of the devices and ensure that both 2.0 root hub and
  // 3.0 root hub showed up.
  if (count < 2) {
    return ZX_ERR_BAD_STATE;
  }
  return ZX_OK;
}

zx::result<fidl::ClientEnd<fuchsia_hardware_usb_tester::Device>> open_test_device() {
  for (auto const& dir_entry : std::filesystem::directory_iterator{kUsbTesterDir}) {
    return component::Connect<fuchsia_hardware_usb_tester::Device>(dir_entry.path().c_str());
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

TEST(UsbTests, RootHubsTest) {
  // TODO(https://fxbug.dev/42072584): There should be a matrix of hardware that should
  // be accessible from here. Depending on whether the hardware has
  // xhci/ehci, we should check the root hubs.
  if (zx_status_t status = check_xhci_root_hubs(); status != ZX_OK) {
    // TODO(https://fxbug.dev/42072584): At the moment we cannot restrict a test
    // to only run on hardware(https://fxbug.dev/42175425) and not the qemu instances.
    // We should fail here when running on hardware.
    printf("[SKIPPING] Root hub creation failed.\n");
    return;
  }
}

TEST(UsbTests, BulkLoopbackTest) {
  zx::result device_result = open_test_device();
  if (device_result.is_error()) {
    printf("[SKIPPING]\n");
    return;
  }

  {
    fuchsia_hardware_usb_tester::wire::BulkTestParams params = {
        .data_pattern = fuchsia_hardware_usb_tester::wire::DataPatternType::kConstant,
        .len = 64 * 1024,
    };
    fidl::WireResult result =
        fidl::WireCall(device_result.value())->BulkLoopback(params, nullptr, nullptr);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().s);
  }

  {
    fuchsia_hardware_usb_tester::wire::BulkTestParams params = {
        .data_pattern = fuchsia_hardware_usb_tester::wire::DataPatternType::kRandom,
        .len = 64 * 1024,
    };
    fidl::WireResult result =
        fidl::WireCall(device_result.value())->BulkLoopback(params, nullptr, nullptr);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().s);
  }
}

TEST(UsbTests, BulkScatterGatherTest) {
  zx::result device_result = open_test_device();
  if (device_result.is_error()) {
    printf("[SKIPPING]\n");
    return;
  }

  fuchsia_hardware_usb_tester::wire::BulkTestParams params = {
      .data_pattern = fuchsia_hardware_usb_tester::wire::DataPatternType::kRandom,
      .len = 230,
  };

  fuchsia_hardware_usb_tester::wire::SgList sg_list = {.len = 5};
  sg_list.entries[0] = {.length = 10, .offset = 100};
  sg_list.entries[1] = {.length = 30, .offset = 1000};
  sg_list.entries[2] = {.length = 100, .offset = 4000};
  sg_list.entries[3] = {.length = 40, .offset = 5000};
  sg_list.entries[4] = {.length = 50, .offset = 10000};

  auto sg_list_view =
      fidl::ObjectView<fuchsia_hardware_usb_tester::wire::SgList>::FromExternal(&sg_list);

  {
    fidl::WireResult result =
        fidl::WireCall(device_result.value())->BulkLoopback(params, sg_list_view, nullptr);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().s);
  }

  {
    fidl::WireResult result =
        fidl::WireCall(device_result.value())->BulkLoopback(params, nullptr, sg_list_view);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().s);
  }
}

void usb_isoch_verify_result(fuchsia_hardware_usb_tester::wire::IsochResult& result) {
  ASSERT_GT(result.num_packets, 0lu, "didn't transfer any isochronous packets");
  // Isochronous transfers aren't guaranteed, so just require a high enough percentage to pass.
  ASSERT_GE(result.num_packets, ISOCH_MIN_PACKETS,
            "num_packets is too low for a reliable result, should request more bytes");
  double percent_passed =
      (static_cast<double>(result.num_passed) / static_cast<double>(result.num_packets)) * 100;
  ASSERT_GE(percent_passed, ISOCH_MIN_PASS_PERCENT, "not enough isoch transfers succeeded");
}

TEST(UsbTests, IsochLoopbackTest) {
  zx::result device_result = open_test_device();
  if (device_result.is_error()) {
    printf("[SKIPPING]\n");
    return;
  }

  {
    fuchsia_hardware_usb_tester::wire::IsochTestParams params = {
        .data_pattern = fuchsia_hardware_usb_tester::wire::DataPatternType::kConstant,
        .num_packets = 64,
        .packet_size = 1024,
    };

    fidl::WireResult result = fidl::WireCall(device_result.value())->IsochLoopback(params);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().s);
    ASSERT_NO_FATAL_FAILURE(usb_isoch_verify_result(result.value().result));
  }

  {
    fuchsia_hardware_usb_tester::wire::IsochTestParams params = {
        .data_pattern = fuchsia_hardware_usb_tester::wire::DataPatternType::kRandom,
        .num_packets = 64,
        .packet_size = 1024,
    };

    fidl::WireResult result = fidl::WireCall(device_result.value())->IsochLoopback(params);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().s);
    ASSERT_NO_FATAL_FAILURE(usb_isoch_verify_result(result.value().result));
  }
}

TEST(UsbTests, CallbacksOptOutTest) {
  zx::result device_result = open_test_device();
  if (device_result.is_error()) {
    printf("[SKIPPING]\n");
    return;
  }

  fuchsia_hardware_usb_tester::wire::IsochTestParams params = {
      .data_pattern = fuchsia_hardware_usb_tester::wire::DataPatternType::kConstant,
      .num_packets = 64,
      .packet_size = 1024,
      .packet_opts_len = params.num_packets,
  };
  size_t reqs_per_callback = 10;
  for (size_t i = 0; i < params.num_packets; ++i) {
    // Set a callback on every 10 requests, and also on the last request.
    bool set_cb = ((i + 1) % reqs_per_callback == 0) || (i == params.num_packets - 1);
    params.packet_opts[i].set_cb = set_cb;
    params.packet_opts[i].expect_cb = set_cb;
  }

  fidl::WireResult result = fidl::WireCall(device_result.value())->IsochLoopback(params);
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().s);
  ASSERT_NO_FATAL_FAILURE(usb_isoch_verify_result(result.value().result));
}

TEST(UsbTests, SingleCallbackErrorTest) {
  zx::result device_result = open_test_device();
  if (device_result.is_error()) {
    printf("[SKIPPING]\n");
    return;
  }

  // We should always get a callback on error.
  fuchsia_hardware_usb_tester::wire::IsochTestParams params = {
      .data_pattern = fuchsia_hardware_usb_tester::wire::DataPatternType::kConstant,
      .num_packets = 1,
      .packet_size = 1024,
      .packet_opts_len = 1,
  };
  params.packet_opts[0] = {
      .set_cb = false,
      .set_error = true,
      .expect_cb = true,
  };

  fidl::WireResult result = fidl::WireCall(device_result.value())->IsochLoopback(params);
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().s);
  // Don't need to verify the transfer results since we only care about callbacks for this test.
}

TEST(UsbTests, CallbacksOnErrorTest) {
  zx::result device_result = open_test_device();
  if (device_result.is_error()) {
    printf("[SKIPPING]\n");
    return;
  }

  // Error on the last packet receiving a callback.
  fuchsia_hardware_usb_tester::wire::IsochTestParams params = {
      .data_pattern = fuchsia_hardware_usb_tester::wire::DataPatternType::kConstant,
      .num_packets = 4,
      .packet_size = 1024,
      .packet_opts_len = 4,
  };
  params.packet_opts[0] = {.set_cb = false, .set_error = false, .expect_cb = false};
  params.packet_opts[1] = {.set_cb = false, .set_error = true, .expect_cb = true};
  params.packet_opts[2] = {.set_cb = false, .set_error = false, .expect_cb = true};
  params.packet_opts[3] = {.set_cb = true, .set_error = true, .expect_cb = true};

  fidl::WireResult result = fidl::WireCall(device_result.value())->IsochLoopback(params);
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().s);
  // Don't need to verify the transfer results since we only care about callbacks for this test.
}

TEST(UsbTests, CallbacksOnMultipleErrorsTests) {
  zx::result device_result = open_test_device();
  if (device_result.is_error()) {
    printf("[SKIPPING]\n");
    return;
  }

  fuchsia_hardware_usb_tester::wire::IsochTestParams params = {
      .data_pattern = fuchsia_hardware_usb_tester::wire::DataPatternType::kConstant,
      .num_packets = 10,
      .packet_size = 1024,
      .packet_opts_len = 10,
  };
  params.packet_opts[0] = {.set_cb = false, .set_error = false, .expect_cb = false};
  params.packet_opts[1] = {.set_cb = false, .set_error = false, .expect_cb = true};
  params.packet_opts[2] = {.set_cb = false, .set_error = true, .expect_cb = true};
  params.packet_opts[3] = {.set_cb = true, .set_error = true, .expect_cb = true};
  params.packet_opts[4] = {.set_cb = false, .set_error = false, .expect_cb = false};
  params.packet_opts[5] = {.set_cb = false, .set_error = true, .expect_cb = true};
  params.packet_opts[6] = {.set_cb = false, .set_error = false, .expect_cb = false};
  params.packet_opts[7] = {.set_cb = true, .set_error = false, .expect_cb = true};
  params.packet_opts[8] = {.set_cb = false, .set_error = true, .expect_cb = true};
  params.packet_opts[9] = {.set_cb = true, .set_error = false, .expect_cb = true};

  fidl::WireResult result = fidl::WireCall(device_result.value())->IsochLoopback(params);
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().s);
  // Don't need to verify the transfer results since we only care about callbacks for this test.
}

}  // namespace
