// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#define CAMERA_SENSOR_IMX_227_TEST 1
#include "src/camera/drivers/sensors/imx227/imx227.h"

#include <endian.h>
#include <fidl/fuchsia.hardware.clock/cpp/wire.h>
#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fuchsia/hardware/mipicsi/cpp/banjo-mock.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/fake-clock/cpp/fake-clock.h>
#include <lib/mock-i2c/mock-i2c.h>

#include <zxtest/zxtest.h>

#include "src/camera/drivers/sensors/imx227/constants.h"
#include "src/camera/drivers/sensors/imx227/imx227_id.h"
#include "src/camera/drivers/sensors/imx227/imx227_seq.h"
#include "src/camera/drivers/sensors/imx227/mipi_ccs_regs.h"
#include "src/devices/gpio/testing/fake-gpio/fake-gpio.h"
#include "src/devices/testing/mock-ddk/mock-device.h"

// The following equality operators are necessary for mocks.

bool operator==(const dimensions_t& lhs, const dimensions_t& rhs) {
  return lhs.x == rhs.x && lhs.y == rhs.y;
}

bool operator==(const mipi_adap_info_t& lhs, const mipi_adap_info_t& rhs) {
  return lhs.resolution == rhs.resolution && lhs.format == rhs.format && lhs.mode == rhs.mode &&
         lhs.path == rhs.path;
}

bool operator==(const mipi_info_t& lhs, const mipi_info_t& rhs) {
  return lhs.channel == rhs.channel && lhs.lanes == rhs.lanes && lhs.ui_value == rhs.ui_value &&
         lhs.csi_version == rhs.csi_version;
}

namespace camera {
namespace {

const uint32_t kTestMode0 = 0;
const uint32_t kTestMode1 = 1;

std::vector<uint8_t> SplitBytes(uint16_t bytes) {
  return std::vector<uint8_t>{static_cast<uint8_t>(bytes >> 8), static_cast<uint8_t>(bytes & 0xff)};
}

class FakeImx227Device : public Imx227Device {
 public:
  FakeImx227Device(zx_device_t* parent, fidl::ClientEnd<fuchsia_hardware_clock::Clock> clk24,
                   fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> gpio_vana_enable,
                   fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> gpio_vdig_enable,
                   fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> gpio_cam_rst)
      : Imx227Device(parent, std::move(clk24), std::move(gpio_vana_enable),
                     std::move(gpio_vdig_enable), std::move(gpio_cam_rst)),
        proto_({&camera_sensor2_protocol_ops_, this}) {
    SetProtocols();
    ASSERT_OK(InitPdev());
    ASSERT_NO_FATAL_FAILURE(VerifyAll());
  }

  void ExpectDeInit() { mock_mipi_.ExpectDeInit(ZX_OK); }

  void ExpectGetSensorId() {
    const auto kSensorModelIdHiRegByteVec = SplitBytes(htobe16(kSensorModelIdReg));
    const auto kSensorModelIdLoRegByteVec = SplitBytes(htobe16(kSensorModelIdReg + 1));
    const auto kSensorModelIdDefaultByteVec = SplitBytes(kSensorModelIdDefault);
    // An I2C bus read is a write of the address followed by a read of the data.
    // In this case, there are two 8-bit reads occuring to get the full 16-bit Sensor Model ID.
    mock_i2c_.ExpectWrite({kSensorModelIdHiRegByteVec[1], kSensorModelIdHiRegByteVec[0]})
        .ExpectReadStop({kSensorModelIdDefaultByteVec[0]})
        .ExpectWrite({kSensorModelIdLoRegByteVec[1], kSensorModelIdLoRegByteVec[0]})
        .ExpectReadStop({kSensorModelIdDefaultByteVec[1]});
  }

  void ExpectRead16(const uint16_t addr, const uint16_t data) {
    const auto kSensorRegHiByteAddrVec = SplitBytes(htobe16(addr));
    const auto kSensorRegLoByteAddrVec = SplitBytes(htobe16(addr + 1));
    const auto kSensorDataByteVec = SplitBytes(htobe16(data));
    mock_i2c_.ExpectWrite({kSensorRegHiByteAddrVec[1], kSensorRegHiByteAddrVec[0]})
        .ExpectReadStop({kSensorDataByteVec[1]})
        .ExpectWrite({kSensorRegLoByteAddrVec[1], kSensorRegLoByteAddrVec[0]})
        .ExpectReadStop({kSensorDataByteVec[0]});
  }

  void ExpectWrite16(const uint16_t addr, const uint16_t data) {
    const auto kSensorRegHiByteAddrVec = SplitBytes(htobe16(addr));
    const auto kSensorDataByteVec = SplitBytes(htobe16(data));
    mock_i2c_.ExpectWriteStop({kSensorRegHiByteAddrVec[1], kSensorRegHiByteAddrVec[0],
                               kSensorDataByteVec[1], kSensorDataByteVec[0]});
  }

  // Expect hardware default values to be read during CameraSensor2Init().
  void ExpectCameraSensor2Init() {
    uint16_t analog_gain_global_value = 0x0000;
    uint16_t digital_gain_global_value = 0x0100;
    uint16_t coarse_integration_time_value = 0x03e8;

    ExpectRead16(kAnalogGainCodeGlobalReg, analog_gain_global_value);
    ExpectRead16(kDigitalGainGlobalReg, digital_gain_global_value);
    ExpectRead16(kCoarseIntegrationTimeReg, coarse_integration_time_value);
  }

  void ExpectReadAnalogGainConstants() {
    mock_i2c_.ExpectWrite({0x00, 0x84})
        .ExpectReadStop({// gain_code_min = 0
                         0, 0,
                         // gain_code_max = 224
                         0, 224,
                         // code_step_size = 1
                         0, 1,
                         // gain_type = 0
                         0, 0,
                         // m0 = 0
                         0, 0,
                         // c0 = 256
                         1, 0,
                         // m1 = -1
                         0xff, 0xff,
                         // c1 = 256
                         1, 0});
  }

  void ExpectReadDigitalGainConstants() {
    mock_i2c_.ExpectWrite({0x10, 0x84})
        .ExpectReadStop({// gain_min = 256
                         1, 0,
                         // gain_max = 4095
                         0x0f, 0xff,
                         // gain_step_size = 1
                         0, 1});
  }

  void SetProtocols() {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_i2c::Device>::Create();

    fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), &mock_i2c_);

    i2c_ = ddk::I2cChannel(std::move(endpoints.client));
    mipi_ = ddk::MipiCsiProtocolClient(mock_mipi_.GetProto());

    EXPECT_OK(loop_.StartThread());
  }

  void VerifyAll() {
    mock_i2c_.VerifyAndClear();
    mock_mipi_.VerifyAndClear();
  }

  fpromise::result<uint8_t, zx_status_t> GetRegValFromSeq(uint8_t index, uint16_t address) {
    return GetRegisterValueFromSequence(index, address);
  }
  fpromise::result<uint16_t, zx_status_t> GetRegValFromSeq16(uint8_t index, uint16_t address) {
    return GetRegisterValueFromSequence16(index, address);
  }

  const camera_sensor2_protocol_t* proto() const { return &proto_; }
  mock_i2c::MockI2c& mock_i2c() { return mock_i2c_; }

 private:
  camera_sensor2_protocol_t proto_;
  mock_i2c::MockI2c mock_i2c_;
  ddk::MockMipiCsi mock_mipi_;
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
};

class Imx227DeviceTest : public zxtest::Test {
 public:
  void SetUp() override {
    ASSERT_OK(mock_fidl_servers_loop_.StartThread("mock-fidl-servers"));

    ASSERT_FALSE(mock_clk24_.SyncCall(&fdf_fake::FakeClock::enabled));

    zx::result clk24_client = mock_clk24_.SyncCall(
        [&](fdf_fake::FakeClock* clock) { return zx::make_result(ZX_OK, clock->Connect()); });

    ASSERT_OK(clk24_client);

    fidl::ClientEnd gpio_vana_enable_client =
        mock_gpio_vana_enable_.SyncCall(&fake_gpio::FakeGpio::Connect);

    fidl::ClientEnd gpio_vdig_enable_client =
        mock_gpio_vdig_enable_.SyncCall(&fake_gpio::FakeGpio::Connect);

    fidl::ClientEnd gpio_cam_rst_client =
        mock_gpio_cam_rst_.SyncCall(&fake_gpio::FakeGpio::Connect);

    dut_.emplace(fake_parent_.get(), std::move(clk24_client.value()),
                 std::move(gpio_vana_enable_client), std::move(gpio_vdig_enable_client),
                 std::move(gpio_cam_rst_client));

    ASSERT_EQ(0, mock_gpio_vana_enable_.SyncCall(&fake_gpio::FakeGpio::GetWriteValue));
    ASSERT_EQ(0, mock_gpio_vdig_enable_.SyncCall(&fake_gpio::FakeGpio::GetWriteValue));
    ASSERT_EQ(1, mock_gpio_cam_rst_.SyncCall(&fake_gpio::FakeGpio::GetWriteValue));
    fake_parent_->AddProtocol(ZX_PROTOCOL_CAMERA_SENSOR2, dut().proto()->ops, &dut_);
    dut().ExpectCameraSensor2Init();
    ASSERT_OK(dut().CameraSensor2Init());
    ASSERT_TRUE(mock_clk24_.SyncCall(&fdf_fake::FakeClock::enabled));
    ASSERT_EQ(1, mock_gpio_vana_enable_.SyncCall(&fake_gpio::FakeGpio::GetWriteValue));
    ASSERT_EQ(1, mock_gpio_vdig_enable_.SyncCall(&fake_gpio::FakeGpio::GetWriteValue));
    ASSERT_EQ(0, mock_gpio_cam_rst_.SyncCall(&fake_gpio::FakeGpio::GetWriteValue));
  }

  void TearDown() override {
    dut().ExpectDeInit();
    ASSERT_OK(dut().CameraSensor2DeInit());
    ASSERT_EQ(0, mock_gpio_vana_enable_.SyncCall(&fake_gpio::FakeGpio::GetWriteValue));
    ASSERT_EQ(0, mock_gpio_vdig_enable_.SyncCall(&fake_gpio::FakeGpio::GetWriteValue));
    ASSERT_EQ(1, mock_gpio_cam_rst_.SyncCall(&fake_gpio::FakeGpio::GetWriteValue));
    ASSERT_FALSE(mock_clk24_.SyncCall(&fdf_fake::FakeClock::enabled));
    ASSERT_NO_FATAL_FAILURE(dut().VerifyAll());
  }

  FakeImx227Device& dut() { return dut_.value(); }

  std::optional<FakeImx227Device> dut_;

  async::Loop mock_fidl_servers_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};

  async_patterns::TestDispatcherBound<fdf_fake::FakeClock> mock_clk24_{

      mock_fidl_servers_loop_.dispatcher(), std::in_place};
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio> mock_gpio_vana_enable_{
      mock_fidl_servers_loop_.dispatcher(), std::in_place};
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio> mock_gpio_vdig_enable_{
      mock_fidl_servers_loop_.dispatcher(), std::in_place};
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio> mock_gpio_cam_rst_{
      mock_fidl_servers_loop_.dispatcher(), std::in_place};
  std::shared_ptr<MockDevice> fake_parent_ = MockDevice::FakeRootParent();
};

// Returns the coarse integration time corresponding to the requested |frame_rate| if found in the
// lookup table provided.
static uint32_t GetCoarseMaxIntegrationTime(const frame_rate_info_t* lut, uint32_t size,
                                            uint32_t frame_rate) {
  for (uint32_t i = 0; i < size; i++) {
    auto fps =
        lut[i].frame_rate.frames_per_sec_numerator / lut[i].frame_rate.frames_per_sec_denominator;

    if (frame_rate == fps) {
      return lut[i].max_coarse_integration_time;
    }
  }
  return 0;
}

// Basic test that verifies correct behavior of Setup() and TearDown().
TEST_F(Imx227DeviceTest, Sanity) {}

TEST_F(Imx227DeviceTest, ResetCycleOnAndOff) {
  dut().CycleReset();
  std::vector states = mock_gpio_cam_rst_.SyncCall(&fake_gpio::FakeGpio::GetStateLog);
  ASSERT_GE(states.size(), 2);
  ASSERT_EQ(fake_gpio::WriteSubState{.value = 1}, states[states.size() - 2].sub_state);
  ASSERT_EQ(fake_gpio::WriteSubState{.value = 0}, states[states.size() - 1].sub_state);
}

TEST_F(Imx227DeviceTest, GetSensorId) {
  dut().ExpectGetSensorId();
  uint32_t out_id;
  ASSERT_OK(dut().CameraSensor2GetSensorId(&out_id));
  ASSERT_EQ(out_id, kSensorModelIdDefault);
}

TEST_F(Imx227DeviceTest, GetSetTestPatternMode) {
  dut().ExpectRead16(kTestPatternReg, kTestMode0);
  dut().ExpectWrite16(kTestPatternReg, kTestMode1);
  dut().ExpectRead16(kTestPatternReg, kTestMode1);
  uint16_t out_mode;
  ASSERT_OK(dut().CameraSensor2GetTestPatternMode(&out_mode));
  ASSERT_EQ(out_mode, kTestMode0);
  ASSERT_OK(dut().CameraSensor2SetTestPatternMode(kTestMode1));
  ASSERT_OK(dut().CameraSensor2GetTestPatternMode(&out_mode));
  ASSERT_EQ(out_mode, kTestMode1);
}

TEST_F(Imx227DeviceTest, GetFrameRateCoarseIntLut) {
  extension_value_data_type_t ext_val;
  ASSERT_OK(dut().CameraSensor2GetExtensionValue(FRAME_RATE_COARSE_INT_LUT, &ext_val));
  EXPECT_EQ(
      kMaxCoarseIntegrationTimeFor30fpsInLines,
      GetCoarseMaxIntegrationTime(ext_val.frame_rate_info_value, EXTENSION_VALUE_ARRAY_LEN, 30));
  EXPECT_EQ(
      kMaxCoarseIntegrationTimeFor15fpsInLines,
      GetCoarseMaxIntegrationTime(ext_val.frame_rate_info_value, EXTENSION_VALUE_ARRAY_LEN, 15));
}

TEST_F(Imx227DeviceTest, UpdateAnalogGain) {
  dut().ExpectReadAnalogGainConstants();
  dut().ExpectReadDigitalGainConstants();

  // Change gain, verify the new value is written to the sensor.
  float out_gain;
  ASSERT_OK(dut().CameraSensor2SetAnalogGain(8.0, &out_gain));
  dut().mock_i2c().VerifyAndClear();
  ASSERT_EQ(8.0, out_gain);

  dut()
      .mock_i2c()
      // Grouped parameter hold == true
      .ExpectWriteStop({0x01, 0x04, 1})
      // Set Analog Gain:
      //   8 = 256 / (256 - X) -- X == 224
      .ExpectWriteStop({0x02, 0x04, 0, 224})
      // Grouped parameter hold == false
      .ExpectWriteStop({0x01, 0x04, 0});
  ASSERT_OK(dut().CameraSensor2Update());
  dut().mock_i2c().VerifyAndClear();

  // Set the gain to the same value again; we should not update the sensor again.
  ASSERT_OK(dut().CameraSensor2SetAnalogGain(8.0, &out_gain));
  dut().mock_i2c().VerifyAndClear();
  ASSERT_EQ(8.0, out_gain);

  // No i2c interactions expected.
  ASSERT_OK(dut().CameraSensor2Update());
  dut().mock_i2c().VerifyAndClear();
}

TEST_F(Imx227DeviceTest, GetRegisterValueFromSequence) {
  auto result_good = dut().GetRegValFromSeq(0, kFrameLengthLinesReg);
  ASSERT_FALSE(result_good.is_error());
  ASSERT_EQ(result_good.value(), 0x0a);

  auto result_index_too_big = dut().GetRegValFromSeq(kSEQUENCE_TABLE.size(), kFrameLengthLinesReg);
  ASSERT_TRUE(result_index_too_big.is_error());
  ASSERT_EQ(result_index_too_big.error(), ZX_ERR_INVALID_ARGS);

  auto result_addr_not_found = dut().GetRegValFromSeq(0, 0xfff0);
  ASSERT_TRUE(result_addr_not_found.is_error());
  ASSERT_EQ(result_addr_not_found.error(), ZX_ERR_NOT_FOUND);
}

TEST_F(Imx227DeviceTest, GetRegisterValueFromSequence16) {
  auto result_good = dut().GetRegValFromSeq16(0, kFrameLengthLinesReg);
  ASSERT_FALSE(result_good.is_error());
  ASSERT_EQ(result_good.value(), 0x0ae0);

  auto result_index_too_big =
      dut().GetRegValFromSeq16(kSEQUENCE_TABLE.size(), kFrameLengthLinesReg);
  ASSERT_TRUE(result_index_too_big.is_error());
  ASSERT_EQ(result_index_too_big.error(), ZX_ERR_INVALID_ARGS);

  auto result_first_addr_not_found = dut().GetRegValFromSeq16(0, 0x0300);
  ASSERT_TRUE(result_first_addr_not_found.is_error());
  ASSERT_EQ(result_first_addr_not_found.error(), ZX_ERR_NOT_FOUND);

  auto result_second_addr_not_found = dut().GetRegValFromSeq16(0, 0x0301);
  ASSERT_TRUE(result_second_addr_not_found.is_error());
  ASSERT_EQ(result_second_addr_not_found.error(), ZX_ERR_NOT_FOUND);

  auto result_both_addr_not_found = dut().GetRegValFromSeq16(0, 0xfff0);
  ASSERT_TRUE(result_both_addr_not_found.is_error());
  ASSERT_EQ(result_both_addr_not_found.error(), ZX_ERR_NOT_FOUND);
}

}  // namespace
}  // namespace camera
