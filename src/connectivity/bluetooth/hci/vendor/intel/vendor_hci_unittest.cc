// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vendor_hci.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>

#include <gtest/gtest-spi.h>
#include <gtest/gtest.h>

namespace bt_hci_intel {

namespace {

class VendorHciTest : public ::testing::Test {
 public:
  // There's no driver instance for this unittest, so create a driver logger for this test.
  VendorHciTest() {
    loop_ = std::make_unique<::async::Loop>(&kAsyncLoopConfigNeverAttachToThread);
    ZX_ASSERT(ZX_OK == loop_->StartThread(std::string("vendor-hci-worker").c_str(), nullptr));

    std::vector<fuchsia_component_runner::ComponentNamespaceEntry> entries;
    zx::result open_result = component::OpenServiceRoot();
    ZX_ASSERT(open_result.is_ok());

    ::fidl::ClientEnd<::fuchsia_io::Directory> svc = std::move(*open_result);
    entries.emplace_back(fuchsia_component_runner::ComponentNamespaceEntry{{
        .path = "/svc",
        .directory = std::move(svc),
    }});

    // Create Namespace object from the entries.
    auto ns = fdf::Namespace::Create(entries);
    ZX_ASSERT(ns.is_ok());
  }

 private:
  fdf_testing::ScopedGlobalLogger logger_;
  std::unique_ptr<async::Loop> loop_;
};

//////////////////////////////////////////////////////////////////////////////////////
//
// Test cases for fetch_tlv_value()
//

TEST_F(VendorHciTest, NormalCase) {
  const uint8_t tlv[] = {
      0x01,  // type
      0x02,  // length
      0x03,  // value
      0x04,  // value
  };
  EXPECT_EQ(fetch_tlv_value(tlv, 2), 0x0403U);
}

//////////////////////////////////////////////////////////////////////////////////////
//
// Test cases for parse_tlv_version_return_params()
//

TEST_F(VendorHciTest, CornerCaseZeroByte) {
  const uint8_t tlvs[] = {};
  auto parsed = parse_tlv_version_return_params(tlvs, sizeof(tlvs));
  EXPECT_EQ(parsed.status, pw::bluetooth::emboss::StatusCode::PARAMETER_OUT_OF_MANDATORY_RANGE);
}

TEST_F(VendorHciTest, CornerCaseOnlyStatusCode) {
  const uint8_t tlvs[] = {
      0x00,  // StatusCode: OK
  };
  auto parsed = parse_tlv_version_return_params(tlvs, sizeof(tlvs));
  EXPECT_EQ(parsed.status, pw::bluetooth::emboss::StatusCode::PARAMETER_OUT_OF_MANDATORY_RANGE);
}

TEST_F(VendorHciTest, CornerCaseOnlyType) {
  const uint8_t tlvs[] = {
      0x00,  // StatusCode: OK
      0x00,  // type
  };
  auto parsed = parse_tlv_version_return_params(tlvs, sizeof(tlvs));
  EXPECT_EQ(parsed.status, pw::bluetooth::emboss::StatusCode::PARAMETER_OUT_OF_MANDATORY_RANGE);
}

TEST_F(VendorHciTest, CornerCaseShortValue) {
  const uint8_t tlvs[] = {
      0x00,  // StatusCode: OK
      0x00,  // type
      0x01,  // length
             // The 'length' field above indicates a 1-byte value, but it is absent.
  };
  auto parsed = parse_tlv_version_return_params(tlvs, sizeof(tlvs));
  EXPECT_EQ(parsed.status, pw::bluetooth::emboss::StatusCode::PARAMETER_OUT_OF_MANDATORY_RANGE);
}

TEST_F(VendorHciTest, CornerCaseOnlyEndOfRecord) {
  const uint8_t tlvs[] = {
      0x00,  // StatusCode: OK
      0x00,  // type: end of the TLV record.
      0x00,  // length
  };
  auto parsed = parse_tlv_version_return_params(tlvs, sizeof(tlvs));
  EXPECT_EQ(parsed.status, pw::bluetooth::emboss::StatusCode::SUCCESS);
}

TEST_F(VendorHciTest, NormalCases) {
  const uint8_t tlvs[] = {
      0x00,  // StatusCode: OK

      0x10,  // CNVi hardware version
      0x04,  // length
      0x55, 0xaa, 0x11, 0x33,

      0x11,  // CNVR hardware version
      0x04,  // length
      0x33, 0x11, 0xaa, 0x55,

      0x12,  // hardware info
      0x04,  // length
      0x33, 0x11, 0xaa, 0x55,

      0x16,  // Device revision
      0x02,  // length
      0x33, 0x11,

      0x1c,  // Current mode of operation
      0x01,  // length
      0x55,

      0x1d,  // Timestamp
      0x02,  // length
      0x33, 0x11,

      0x1e,  // Build type
      0x01,  // length
      0xbe,

      0x1f,  // Build number
      0x04,  // length
      0xef, 0xbe, 0xad, 0xde,

      0x28,  // Secure boot
      0x01,  // length
      0xde,

      0x2a,  // OTP lock
      0x01,  // length
      0xad,

      0x2b,  // API lock
      0x01,  // length
      0xbe,

      0x2c,  // debug boot
      0x01,  // length
      0xef,

      0x2d,  // firmware build
      0x03,  // length
      0x56, 0x34, 0x12,

      0x2f,  // Secure boot engine type
      0x01,  // length
      0x02,

      0x30,  // Bluetooth device address
      0x06,  // length
      0x66, 0x55, 0x44, 0x33, 0x22, 0x11,

      0x00,  // type: end of the TLV record.
      0x00,  // length
  };
  auto parsed = parse_tlv_version_return_params(tlvs, sizeof(tlvs));
  EXPECT_EQ(parsed.status, pw::bluetooth::emboss::StatusCode::SUCCESS);
  EXPECT_EQ(parsed.CNVi, 0x53a5);
  EXPECT_EQ(parsed.CNVR, 0x3513);
  EXPECT_EQ(parsed.hw_platform, 0x11);
  EXPECT_EQ(parsed.hw_variant, 0x2a);
  EXPECT_EQ(parsed.device_revision, 0x1133);
  EXPECT_EQ(parsed.current_mode_of_operation, 0x55);
  EXPECT_EQ(parsed.timestamp_calendar_week, 0x33);
  EXPECT_EQ(parsed.timestamp_year, 0x11);
  EXPECT_EQ(parsed.build_type, 0xbe);
  EXPECT_EQ(parsed.build_number, 0xdeadbeef);
  EXPECT_EQ(parsed.secure_boot, 0xde);
  EXPECT_EQ(parsed.otp_lock, 0xad);
  EXPECT_EQ(parsed.api_lock, 0xbe);
  EXPECT_EQ(parsed.debug_lock, 0xef);
  EXPECT_EQ(parsed.firmware_build_number, 0x56);
  EXPECT_EQ(parsed.firmware_build_calendar_week, 0x34);
  EXPECT_EQ(parsed.firmware_build_year, 0x12);
  EXPECT_EQ(parsed.secure_boot_engine_type, 0x02);
  EXPECT_EQ(parsed.bluetooth_address[0], 0x66);
  EXPECT_EQ(parsed.bluetooth_address[5], 0x11);
}

}  // anonymous namespace

}  // namespace bt_hci_intel
