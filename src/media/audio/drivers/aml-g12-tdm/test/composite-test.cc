// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/drivers/aml-g12-tdm/composite.h"

#include <fidl/fuchsia.hardware.clock/cpp/wire_test_base.h>
#include <fidl/fuchsia.hardware.gpio/cpp/wire_test_base.h>
#include <fidl/fuchsia.hardware.pin/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/fidl.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/fake-bti/cpp/fake-bti.h>
#include <lib/driver/fake-mmio-reg/cpp/fake-mmio-reg.h>
#include <lib/driver/fake-pin/cpp/fake-pin.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fdio/directory.h>
#include <lib/fzl/vmo-mapper.h>
#include <zircon/errors.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace audio::aml_g12 {

// These tests check the audio-composite driver implementation variation of AMLogic SoCs audio
// support. Helper code in AmlTdmConfigDevice and //src/devices/lib/amlogic is tested in other
// versions of this driver (StreamConfig and DAI).

constexpr fuchsia_hardware_audio::ElementId kEngineARingBufferIdOutput = 4;
constexpr fuchsia_hardware_audio::ElementId kEngineBRingBufferIdOutput = 5;
constexpr fuchsia_hardware_audio::ElementId kEngineCRingBufferIdOutput = 6;
constexpr fuchsia_hardware_audio::ElementId kEngineARingBufferIdInput = 7;
constexpr fuchsia_hardware_audio::ElementId kEngineBRingBufferIdInput = 8;
constexpr fuchsia_hardware_audio::ElementId kEngineCRingBufferIdInput = 9;
constexpr fuchsia_hardware_audio::ElementId kInvalidBufferId0 = 3;
constexpr fuchsia_hardware_audio::ElementId kInvalidBufferId1 = 10;

constexpr std::array<fuchsia_hardware_audio::ElementId, 6> kAllValidRingBufferIds{
    kEngineARingBufferIdOutput, kEngineBRingBufferIdOutput, kEngineCRingBufferIdOutput,
    kEngineARingBufferIdInput,  kEngineBRingBufferIdInput,  kEngineCRingBufferIdInput,
};

// Fake clock for power management test.
class FakeClock : public fidl::testing::WireTestBase<fuchsia_hardware_clock::Clock> {
 public:
  FakeClock() = default;

  bool IsFakeClockEnabled() const { return enabled_; }
  uint32_t enable_count() const { return enable_count_; }

  fuchsia_hardware_clock::Service::InstanceHandler GetInstanceHandler() {
    return fuchsia_hardware_clock::Service::InstanceHandler({
        .clock = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                         fidl::kIgnoreBindingClosure),
    });
  }

 protected:
  void Enable(EnableCompleter::Sync& completer) override {
    enabled_ = true;
    enable_count_++;
    completer.Reply(zx::ok());
  }
  void Disable(DisableCompleter::Sync& completer) override {
    enabled_ = false;
    completer.Reply(zx::ok());
  }
  void IsEnabled(IsEnabledCompleter::Sync& completer) override {
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
  }

  void SetRate(::fuchsia_hardware_clock::wire::ClockSetRateRequest* request,
               SetRateCompleter::Sync& completer) override {
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
  }
  void QuerySupportedRate(::fuchsia_hardware_clock::wire::ClockQuerySupportedRateRequest* request,
                          QuerySupportedRateCompleter::Sync& completer) override {
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
  }

  void GetRate(GetRateCompleter::Sync& completer) override {
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
  }
  void SetInput(::fuchsia_hardware_clock::wire::ClockSetInputRequest* request,
                SetInputCompleter::Sync& completer) override {
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
  }
  void GetNumInputs(GetNumInputsCompleter::Sync& completer) override {
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
  }
  void GetInput(GetInputCompleter::Sync& completer) override {
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
  }
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  bool enabled_ = false;
  uint32_t enable_count_ = 0;
  fidl::ServerBindingGroup<fuchsia_hardware_clock::Clock> bindings_;
};

class FakeGpio : public fidl::testing::WireTestBase<fuchsia_hardware_gpio::Gpio> {
 public:
  FakeGpio() = default;

  zx::result<> Serve(async_dispatcher_t* dispatcher, fdf::OutgoingDirectory& to_driver_vfs,
                     std::string_view instance) {
    fuchsia_hardware_gpio::Service::InstanceHandler handler{{
        .device = gpio_bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure),
    }};

    return to_driver_vfs.AddService<fuchsia_hardware_gpio::Service>(std::move(handler), instance);
  }

 protected:
  void SetBufferMode(SetBufferModeRequestView request,
                     SetBufferModeCompleter::Sync& completer) override {
    completer.ReplySuccess();
  }
  void Read(ReadCompleter::Sync& completer) override { completer.ReplySuccess(0); }
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_gpio::Gpio> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL() << "unknown method (Gpio) ordinal " << metadata.method_ordinal;
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_gpio::Gpio> gpio_bindings_;
};

class AmlG12CompositeTestEnvironment : public fdf_testing::Environment {
 public:
  static constexpr size_t kMmioRegSize = sizeof(uint32_t);
  static constexpr size_t kMmioRegCount = 0x1000 / kMmioRegSize;

  void Init() {
    zx::result bti = fake_bti::CreateFakeBti();
    ASSERT_OK(bti);
    std::map<uint32_t, zx::bti> btis;
    btis.insert({0, std::move(bti.value())});

    std::map<uint32_t, fdf_fake::Mmio> mmios;
    mmios.insert({0, mmio_reg_region_.GetMmioBuffer()});

    pdev_.SetConfig({.mmios = std::move(mmios),
                     .btis = std::move(btis),
                     .device_info{{
                         .vid = PDEV_VID_AMLOGIC,
                         .pid = PDEV_PID_AMLOGIC_A311D,
                     }}});

    for (size_t i = 0; i < kMmioRegCount; ++i) {
      auto& mmio_reg = mmio_reg_region_[i * kMmioRegSize];
      mmio_reg.SetReadCallback([this, i]() { return mmio_reg_values_[i]; });
      mmio_reg.SetWriteCallback([this, i](uint64_t value) { mmio_reg_values_[i] = value; });
    }
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    auto* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
          pdev_.GetInstanceHandler(dispatcher));
      if (result.is_error()) {
        return result.take_error();
      }
    }

    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_clock::Service>(
          clock_gate_.GetInstanceHandler(), Driver::kClockGateParentName);
      if (result.is_error()) {
        return result.take_error();
      }
    }

    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_clock::Service>(
          clock_pll_.GetInstanceHandler(), Driver::kClockPllParentName);
      if (result.is_error()) {
        return result.take_error();
      }
    }

    {
      zx::result result =
          sclk_tdm_a_.Serve(dispatcher, to_driver_vfs, Driver::kGpioTdmASclkParentName);
      if (result.is_error()) {
        return result.take_error();
      }
    }

    {
      zx::result result =
          sclk_tdm_b_.Serve(dispatcher, to_driver_vfs, Driver::kGpioTdmBSclkParentName);
      if (result.is_error()) {
        return result.take_error();
      }
    }

    {
      zx::result result =
          sclk_tdm_c_.Serve(dispatcher, to_driver_vfs, Driver::kGpioTdmCSclkParentName);
      if (result.is_error()) {
        return result.take_error();
      }
    }

    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_pin::Service>(
          sclk_tdm_a_pin_.CreateInstanceHandler(), Driver::kGpioTdmASclkParentName);
      if (result.is_error()) {
        return result.take_error();
      }
    }

    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_pin::Service>(
          sclk_tdm_b_pin_.CreateInstanceHandler(), Driver::kGpioTdmBSclkParentName);
      if (result.is_error()) {
        return result.take_error();
      }
    }

    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_pin::Service>(
          sclk_tdm_c_pin_.CreateInstanceHandler(), Driver::kGpioTdmCSclkParentName);
      if (result.is_error()) {
        return result.take_error();
      }
    }

    return zx::ok();
  }

  FakeClock& clock_gate() { return clock_gate_; }
  FakeClock& clock_pll() { return clock_pll_; }
  fdf_fake::FakePin& sclk_tdm_a_pin() { return sclk_tdm_a_pin_; }
  fdf_fake::FakePin& sclk_tdm_b_pin() { return sclk_tdm_b_pin_; }
  fdf_fake::FakePin& sclk_tdm_c_pin() { return sclk_tdm_c_pin_; }
  fake_mmio::FakeMmioRegRegion& mmio() { return mmio_reg_region_; }

 private:
  fdf_fake::FakePDev pdev_;
  FakeClock clock_gate_;
  FakeClock clock_pll_;
  FakeGpio sclk_tdm_a_;
  FakeGpio sclk_tdm_b_;
  FakeGpio sclk_tdm_c_;
  fdf_fake::FakePin sclk_tdm_a_pin_;
  fdf_fake::FakePin sclk_tdm_b_pin_;
  fdf_fake::FakePin sclk_tdm_c_pin_;

  fake_mmio::FakeMmioRegRegion mmio_reg_region_{kMmioRegSize, kMmioRegCount};
  std::array<uint64_t, kMmioRegCount> mmio_reg_values_;
};

class FixtureConfig final {
 public:
  using DriverType = Driver;
  using EnvironmentType = AmlG12CompositeTestEnvironment;
};

class AmlG12CompositeTest : public testing::Test {
 public:
  void SetUp() override {
    driver_test_.RunInEnvironmentTypeContext([](auto& env) { env.Init(); });
    ASSERT_OK(driver_test_.StartDriver());
    zx::result client =
        driver_test_.ConnectThroughDevfs<fuchsia_hardware_audio::Composite>(Driver::kDriverName);
    ASSERT_OK(client);
    client_.Bind(std::move(client.value()));
  }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

 protected:
  static fuchsia_hardware_audio::DaiFormat GetDefaultDaiFormat() {
    return fuchsia_hardware_audio::DaiFormat(
        2, 3, fuchsia_hardware_audio::DaiSampleFormat::kPcmSigned,
        fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatStandard(
            fuchsia_hardware_audio::DaiFrameFormatStandard::kI2S),
        48'000, 32, 16);
  }

  static fuchsia_hardware_audio::Format2 GetDefaultRingBufferFormat() {
    fuchsia_hardware_audio::PcmFormat pcm_format{
        2, fuchsia_hardware_audio::SampleFormat::kPcmSigned, 2, 16, 48'000};
    return fuchsia_hardware_audio::Format2::WithPcmFormat(std::move(pcm_format));
  }

  bool IsFakeClockGateEnabled() {
    return driver_test_.RunInEnvironmentTypeContext<bool>(
        [](auto& env) { return env.clock_gate().IsFakeClockEnabled(); });
  }

  uint32_t GetFakeClockGateEnableCount() {
    return driver_test_.RunInEnvironmentTypeContext<uint32_t>(
        [](auto& env) { return env.clock_gate().enable_count(); });
  }

  bool IsFakeClockPllEnabled() {
    return driver_test_.RunInEnvironmentTypeContext<bool>(
        [](auto& env) { return env.clock_pll().IsFakeClockEnabled(); });
  }

  bool IsTdmASclkSet() {
    return driver_test_.RunInEnvironmentTypeContext<bool>(
        [](auto& env) { return env.sclk_tdm_a_pin().take_function() == kSclkPinFunction; });
  }

  bool IsTdmBSclkSet() {
    return driver_test_.RunInEnvironmentTypeContext<bool>(
        [](auto& env) { return env.sclk_tdm_b_pin().take_function() == kSclkPinFunction; });
  }

  bool IsTdmCSclkSet() {
    return driver_test_.RunInEnvironmentTypeContext<bool>(
        [](auto& env) { return env.sclk_tdm_c_pin().take_function() == kSclkPinFunction; });
  }

  void CheckDefaultDaiFormats(fuchsia_hardware_audio::ElementId id) {
    auto dai_formats_result = client()->GetDaiFormats(id);
    ASSERT_TRUE(dai_formats_result.is_ok());
    ASSERT_EQ(1, dai_formats_result->dai_formats().size());
    auto& dai_formats = dai_formats_result->dai_formats()[0];
    ASSERT_EQ(2, dai_formats.number_of_channels().size());
    ASSERT_EQ(1, dai_formats.number_of_channels()[0]);
    ASSERT_EQ(2, dai_formats.number_of_channels()[1]);
    ASSERT_EQ(1, dai_formats.sample_formats().size());
    ASSERT_EQ(fuchsia_hardware_audio::DaiSampleFormat::kPcmSigned, dai_formats.sample_formats()[0]);
    ASSERT_EQ(7, dai_formats.frame_formats().size());
    ASSERT_EQ(fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatStandard(
                  fuchsia_hardware_audio::DaiFrameFormatStandard::kI2S),
              dai_formats.frame_formats()[0]);
    ASSERT_EQ(fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatStandard(
                  fuchsia_hardware_audio::DaiFrameFormatStandard::kTdm1),
              dai_formats.frame_formats()[1]);
    ASSERT_EQ(fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatStandard(
                  fuchsia_hardware_audio::DaiFrameFormatStandard::kTdm2),
              dai_formats.frame_formats()[2]);
    ASSERT_EQ(fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatStandard(
                  fuchsia_hardware_audio::DaiFrameFormatStandard::kTdm3),
              dai_formats.frame_formats()[3]);
    ASSERT_EQ(fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatStandard(
                  fuchsia_hardware_audio::DaiFrameFormatStandard::kStereoLeft),
              dai_formats.frame_formats()[4]);
    ASSERT_EQ(fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatCustom(
                  fuchsia_hardware_audio::DaiFrameFormatCustom(true, true, 1, 1)),
              dai_formats.frame_formats()[5]);
    ASSERT_EQ(fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatCustom(
                  fuchsia_hardware_audio::DaiFrameFormatCustom(true, false, 1, 1)),
              dai_formats.frame_formats()[6]);
    ASSERT_EQ(5, dai_formats.frame_rates().size());
    ASSERT_EQ(8'000, dai_formats.frame_rates()[0]);
    ASSERT_EQ(16'000, dai_formats.frame_rates()[1]);
    ASSERT_EQ(32'000, dai_formats.frame_rates()[2]);
    ASSERT_EQ(48'000, dai_formats.frame_rates()[3]);
    ASSERT_EQ(96'000, dai_formats.frame_rates()[4]);
    ASSERT_EQ(2, dai_formats.bits_per_slot().size());
    ASSERT_EQ(16, dai_formats.bits_per_slot()[0]);
    ASSERT_EQ(32, dai_formats.bits_per_slot()[1]);
    ASSERT_EQ(2, dai_formats.bits_per_sample().size());
    ASSERT_EQ(16, dai_formats.bits_per_sample()[0]);
    ASSERT_EQ(32, dai_formats.bits_per_sample()[1]);
  }

  void SetDaiFormatDefault(fuchsia_hardware_audio::ElementId id) {
    auto format = GetDefaultDaiFormat();
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(id, std::move(format));
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_ok());
  }

  void SetDaiFormatFrameFormat(fuchsia_hardware_audio::ElementId id,
                               fuchsia_hardware_audio::DaiFrameFormat frame_format) {
    auto format = GetDefaultDaiFormat();
    format.frame_format(std::move(frame_format));
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(id, std::move(format));
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_ok());
  }

  void SetDaiFormatFrameRate(fuchsia_hardware_audio::ElementId id, uint32_t rate) {
    auto format = GetDefaultDaiFormat();
    format.frame_rate(rate);
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(id, std::move(format));
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_ok());
  }

  void SetDaiFormatBitsPerSampleAndBitsPerSlot(fuchsia_hardware_audio::ElementId id,
                                               uint8_t bits_per_sample, uint8_t bits_per_slot) {
    auto format = GetDefaultDaiFormat();
    format.bits_per_sample(bits_per_sample);
    format.bits_per_slot(bits_per_slot);
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(id, std::move(format));
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_ok());
  }

  void SetDaiFormatFrameFormatNumberOfChannels(uint64_t id,
                                               fuchsia_hardware_audio::DaiFrameFormat frame_format,
                                               uint32_t number_of_channels) {
    auto format = GetDefaultDaiFormat();
    format.number_of_channels(number_of_channels);
    format.channels_to_use_bitmask((1 << number_of_channels) - 1);
    format.frame_format(std::move(frame_format));
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(id, std::move(format));
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_ok());
  }

  void TestPowerState(bool on) {
    EXPECT_EQ(IsTdmASclkSet(), on);
    EXPECT_EQ(IsTdmBSclkSet(), on);
    EXPECT_EQ(IsTdmCSclkSet(), on);
    EXPECT_EQ(IsFakeClockGateEnabled(), on);
    EXPECT_EQ(IsFakeClockPllEnabled(), on);
  }

  fidl::SyncClient<fuchsia_hardware_audio::Composite>& client() { return client_; }

  uint32_t GetMmioValue(uint32_t mmio_reg_index) {
    auto address = mmio_reg_index * AmlG12CompositeTestEnvironment::kMmioRegSize;
    return driver_test_.RunInEnvironmentTypeContext<uint32_t>(
        [address](auto& env) { return env.mmio()[address].Read(); });
  }

 private:
  static constexpr uint64_t kSclkPinFunction = 1;

  fidl::SyncClient<fuchsia_hardware_audio::Composite> client_;
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(AmlG12CompositeTest, CompositeProperties) {
  fidl::Result result = client()->GetProperties();
  ASSERT_TRUE(result.is_ok());
  ASSERT_EQ("Amlogic", result->properties().manufacturer().value());
  ASSERT_EQ("g12", result->properties().product().value());
  ASSERT_EQ(fuchsia_hardware_audio::kClockDomainMonotonic,
            result->properties().clock_domain().value());
}

TEST_F(AmlG12CompositeTest, Reset) {
  // TODO(https://fxbug.dev/42082341): Add behavior to change state recovered to the configuration
  // below.
  fidl::Result reset_result = client()->Reset();
  ASSERT_TRUE(reset_result.is_ok());

  // After reset we check we have configured all engines for TDM output and input.

  // Configure TDM OUT for I2S 16 bits per slot and per sample (default).
  // TDM OUT CTRL0 config, bitoffset 2, 2 slots, 16 bits per slot.
  ASSERT_EQ(0x3001'002F, GetMmioValue(0x500 / 4));  // A output.
  ASSERT_EQ(0x3001'002F, GetMmioValue(0x540 / 4));  // B output.
  ASSERT_EQ(0x3001'002F, GetMmioValue(0x580 / 4));  // C output.

  // Configure TDM IN for I2S 16 bits per slot and per sample (default).
  // TDM IN CTRL0 config, PAD_TDMIN A(0), B(1) and C(2).
  ASSERT_EQ(0x7003'000F, GetMmioValue(0x300 / 4));  // A input.
  ASSERT_EQ(0x7013'000F, GetMmioValue(0x340 / 4));  // B input.
  ASSERT_EQ(0x7023'000F, GetMmioValue(0x380 / 4));  // C input.

  // Configure clocks.
  // SCLK CTRL, clk in/out enabled, 24 sdiv, 16 lrduty, 32 lrdiv.
  ASSERT_EQ(0xc180'3c1f, GetMmioValue(0x040 / 4));  // A.
  ASSERT_EQ(0xc180'3c1f, GetMmioValue(0x048 / 4));  // B.
  ASSERT_EQ(0xc180'3c1f, GetMmioValue(0x050 / 4));  // C.
}

TEST_F(AmlG12CompositeTest, ElementsAndTopology) {
  auto endpoints =
      fidl::CreateEndpoints<fuchsia_hardware_audio_signalprocessing::SignalProcessing>();
  auto connect_result = client()->SignalProcessingConnect(std::move(endpoints->server));
  ASSERT_TRUE(connect_result.is_ok());
  fidl::SyncClient signal_client{std::move(endpoints->client)};
  auto elements_result = signal_client->GetElements();
  ASSERT_TRUE(elements_result.is_ok());

  // Got ring buffer and DAIs for all engines input and output.
  constexpr size_t kNumberOfElements = 9;
  ASSERT_EQ(kNumberOfElements, elements_result->processing_elements().size());
  for (size_t i = 0; i < kNumberOfElements; ++i) {
    auto& element = elements_result->processing_elements()[i];
    ASSERT_TRUE(
        element.type() == fuchsia_hardware_audio_signalprocessing::ElementType::kRingBuffer ||
        element.type() == fuchsia_hardware_audio_signalprocessing::ElementType::kDaiInterconnect);
    if (element.type() == fuchsia_hardware_audio_signalprocessing::ElementType::kDaiInterconnect) {
      ASSERT_EQ(fuchsia_hardware_audio_signalprocessing::PlugDetectCapabilities::kHardwired,
                element.type_specific()->dai_interconnect()->plug_detect_capabilities());
      auto watch_element_result = signal_client->WatchElementState(*element.id());
      ASSERT_TRUE(watch_element_result.is_ok());
      ASSERT_EQ(true, watch_element_result->state()
                          .type_specific()
                          ->dai_interconnect()
                          ->plug_state()
                          ->plugged());
    }
  }
  auto& element0 = elements_result->processing_elements()[0];
  auto& element1 = elements_result->processing_elements()[1];
  auto& element2 = elements_result->processing_elements()[2];
  auto& element3 = elements_result->processing_elements()[3];
  auto& element4 = elements_result->processing_elements()[4];
  auto& element5 = elements_result->processing_elements()[5];
  auto& element6 = elements_result->processing_elements()[6];
  auto& element7 = elements_result->processing_elements()[7];
  auto& element8 = elements_result->processing_elements()[8];

  ASSERT_EQ(fuchsia_hardware_audio_signalprocessing::ElementType::kRingBuffer, element0.type());
  ASSERT_EQ(fuchsia_hardware_audio_signalprocessing::ElementType::kRingBuffer, element1.type());
  ASSERT_EQ(fuchsia_hardware_audio_signalprocessing::ElementType::kRingBuffer, element2.type());
  ASSERT_EQ(fuchsia_hardware_audio_signalprocessing::ElementType::kRingBuffer, element3.type());
  ASSERT_EQ(fuchsia_hardware_audio_signalprocessing::ElementType::kRingBuffer, element4.type());
  ASSERT_EQ(fuchsia_hardware_audio_signalprocessing::ElementType::kRingBuffer, element5.type());

  ASSERT_EQ(fuchsia_hardware_audio_signalprocessing::ElementType::kDaiInterconnect,
            element6.type());
  ASSERT_EQ(fuchsia_hardware_audio_signalprocessing::ElementType::kDaiInterconnect,
            element7.type());
  ASSERT_EQ(fuchsia_hardware_audio_signalprocessing::ElementType::kDaiInterconnect,
            element8.type());

  constexpr fuchsia_hardware_audio::TopologyId kExpectedTopologyId = 1;
  constexpr fuchsia_hardware_audio::TopologyId kUnrecognizedTopologyId = 2;
  auto topology_result = signal_client->GetTopologies();
  ASSERT_TRUE(topology_result.is_ok());
  ASSERT_EQ(1, topology_result->topologies().size());
  auto& topology = topology_result->topologies()[0];
  ASSERT_EQ(kExpectedTopologyId, topology.id());

  auto set_topology_result = signal_client->SetTopology(kUnrecognizedTopologyId);
  ASSERT_TRUE(set_topology_result.is_error());
  ASSERT_TRUE(set_topology_result.error_value().is_domain_error());
  ASSERT_EQ(set_topology_result.error_value().domain_error(), ZX_ERR_INVALID_ARGS);

  set_topology_result = signal_client->SetTopology(kExpectedTopologyId);
  ASSERT_TRUE(set_topology_result.is_ok());

  // Get edges for ring buffer and DAIs for all engines.
  constexpr size_t kNumberOfEdges = 6;
  ASSERT_EQ(kNumberOfEdges, topology.processing_elements_edge_pairs()->size());
  auto& edge0 = (*topology.processing_elements_edge_pairs())[0];
  auto& edge1 = (*topology.processing_elements_edge_pairs())[1];
  auto& edge2 = (*topology.processing_elements_edge_pairs())[2];
  auto& edge3 = (*topology.processing_elements_edge_pairs())[3];
  auto& edge4 = (*topology.processing_elements_edge_pairs())[4];
  auto& edge5 = (*topology.processing_elements_edge_pairs())[5];

  // Output.
  ASSERT_EQ(4, edge0.processing_element_id_from());
  ASSERT_EQ(1, edge0.processing_element_id_to());
  ASSERT_EQ(5, edge1.processing_element_id_from());
  ASSERT_EQ(2, edge1.processing_element_id_to());
  ASSERT_EQ(6, edge2.processing_element_id_from());
  ASSERT_EQ(3, edge2.processing_element_id_to());

  // Input.
  ASSERT_EQ(1, edge3.processing_element_id_from());
  ASSERT_EQ(7, edge3.processing_element_id_to());
  ASSERT_EQ(2, edge4.processing_element_id_from());
  ASSERT_EQ(8, edge4.processing_element_id_to());
  ASSERT_EQ(3, edge5.processing_element_id_from());
  ASSERT_EQ(9, edge5.processing_element_id_to());

  auto watch_topology_result = signal_client->WatchTopology();
  ASSERT_TRUE(watch_topology_result.is_ok());
  ASSERT_EQ(1, watch_topology_result->topology_id());
}

TEST_F(AmlG12CompositeTest, ElementsState) {
  auto endpoints =
      fidl::CreateEndpoints<fuchsia_hardware_audio_signalprocessing::SignalProcessing>();
  auto connect_result = client()->SignalProcessingConnect(std::move(endpoints->server));
  ASSERT_TRUE(connect_result.is_ok());
  fidl::SyncClient signal_client{std::move(endpoints->client)};
  auto elements_result = signal_client->GetElements();
  ASSERT_TRUE(elements_result.is_ok());

  // Able to set element state for all (dai_interconnect) elements with no parameters.
  for (fuchsia_hardware_audio_signalprocessing::Element& element :
       elements_result->processing_elements()) {
    if (element.type() == fuchsia_hardware_audio_signalprocessing::ElementType::kDaiInterconnect) {
      fuchsia_hardware_audio_signalprocessing::SignalProcessingSetElementStateRequest request;
      request.processing_element_id(*element.id());
      auto set_element_result = signal_client->SetElementState(request);
      ASSERT_TRUE(set_element_result.is_ok());
    }
  }
}

TEST_F(AmlG12CompositeTest, GetDaiFormatsErrors) {
  // Only ids 1, 2, and 3 configure HW DAI formats.
  {
    auto dai_formats_result = client()->GetDaiFormats(0);
    ASSERT_TRUE(dai_formats_result.is_error());
    ASSERT_TRUE(dai_formats_result.error_value().is_domain_error());
    ASSERT_TRUE(dai_formats_result.error_value().domain_error() ==
                fuchsia_hardware_audio::DriverError::kInvalidArgs);
  }
  {
    auto dai_formats_result = client()->GetDaiFormats(4);
    ASSERT_TRUE(dai_formats_result.is_error());
    ASSERT_TRUE(dai_formats_result.error_value().is_domain_error());
    ASSERT_TRUE(dai_formats_result.error_value().domain_error() ==
                fuchsia_hardware_audio::DriverError::kInvalidArgs);
  }
}

TEST_F(AmlG12CompositeTest, GetDaiFormatsValid) {
  // Ids 1, 2, and 3 are valid configure HW DAI formats.
  CheckDefaultDaiFormats(1);
  CheckDefaultDaiFormats(2);
  CheckDefaultDaiFormats(3);
}

TEST_F(AmlG12CompositeTest, SetDaiFormatsErrors) {
  // Only ids 1, 2, and 3 configure HW DAI formats.
  {
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(0, GetDefaultDaiFormat());
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_error());
    ASSERT_TRUE(dai_formats_result.error_value().is_domain_error());
    ASSERT_TRUE(dai_formats_result.error_value().domain_error() ==
                fuchsia_hardware_audio::DriverError::kInvalidArgs);
  }
  {
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(4, GetDefaultDaiFormat());
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_error());
    ASSERT_TRUE(dai_formats_result.error_value().is_domain_error());
    ASSERT_TRUE(dai_formats_result.error_value().domain_error() ==
                fuchsia_hardware_audio::DriverError::kInvalidArgs);
  }
  {
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(1, GetDefaultDaiFormat());
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_ok());

    // Configure TDM A for 32 bits I2S:
    // TDM OUT CTRL0 config, bitoffset 2, 2 slots, 32 bits per slot.
    ASSERT_EQ(0x3001'003F, GetMmioValue(0x500 / 4));
    // TDM IN CTRL0 config, PAD_TDMIN A, 32 bits per slot.
    ASSERT_EQ(0x7003'001F, GetMmioValue(0x300 / 4));
  }
  {
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(2, GetDefaultDaiFormat());
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_ok());
    // Configure TDM B for 32 bits I2S:
    // TDM OUT CTRL0 config, bitoffset 2, 2 slots, 32 bits per slot.
    ASSERT_EQ(0x3001'003F, GetMmioValue(0x540 / 4));
    // TDM IN CTRL0 config, PAD_TDMIN B, 32 bits per slot.
    ASSERT_EQ(0x7013'001F, GetMmioValue(0x340 / 4));
  }
  {
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(3, GetDefaultDaiFormat());
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_ok());
    // Configure TDM C for 32 bits I2S:
    // TDM OUT CTRL0 config, bitoffset 2, 2 slots, 32 bits per slot.
    ASSERT_EQ(0x3001'003F, GetMmioValue(0x580 / 4));
    // TDM IN CTRL0 config, PAD_TDMIN C, 32bits per slot.
    ASSERT_EQ(0x7023'001F, GetMmioValue(0x380 / 4));
  }

  // Any DAI field not in the supported ones returns an error.
  {
    auto format = GetDefaultDaiFormat();
    format.number_of_channels(123);
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(3, std::move(format));
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_error());
    ASSERT_TRUE(dai_formats_result.error_value().is_domain_error());
    ASSERT_TRUE(dai_formats_result.error_value().domain_error() ==
                fuchsia_hardware_audio::DriverError::kInvalidArgs);
  }
  {
    auto format = GetDefaultDaiFormat();
    format.sample_format(fuchsia_hardware_audio::DaiSampleFormat::kPcmFloat);
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(3, std::move(format));
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_error());
    ASSERT_TRUE(dai_formats_result.error_value().is_domain_error());
    ASSERT_TRUE(dai_formats_result.error_value().domain_error() ==
                fuchsia_hardware_audio::DriverError::kInvalidArgs);
  }
  {
    auto format = GetDefaultDaiFormat();
    format.frame_format(fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatStandard(
        fuchsia_hardware_audio::DaiFrameFormatStandard::kStereoRight));
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(3, std::move(format));
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_error());
    ASSERT_TRUE(dai_formats_result.error_value().is_domain_error());
    ASSERT_TRUE(dai_formats_result.error_value().domain_error() ==
                fuchsia_hardware_audio::DriverError::kInvalidArgs);
  }
  {
    auto format = GetDefaultDaiFormat();
    format.frame_rate(123);
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(3, std::move(format));
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_error());
    ASSERT_TRUE(dai_formats_result.error_value().is_domain_error());
    ASSERT_TRUE(dai_formats_result.error_value().domain_error() ==
                fuchsia_hardware_audio::DriverError::kInvalidArgs);
  }
  {
    auto format = GetDefaultDaiFormat();
    format.bits_per_slot(123);
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(3, std::move(format));
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_error());
    ASSERT_TRUE(dai_formats_result.error_value().is_domain_error());
    ASSERT_TRUE(dai_formats_result.error_value().domain_error() ==
                fuchsia_hardware_audio::DriverError::kInvalidArgs);
  }
  {
    auto format = GetDefaultDaiFormat();
    format.bits_per_sample(123);
    fuchsia_hardware_audio::CompositeSetDaiFormatRequest request(3, std::move(format));
    auto dai_formats_result = client()->SetDaiFormat(request);
    ASSERT_TRUE(dai_formats_result.is_error());
    ASSERT_TRUE(dai_formats_result.error_value().is_domain_error());
    ASSERT_TRUE(dai_formats_result.error_value().domain_error() ==
                fuchsia_hardware_audio::DriverError::kInvalidArgs);
  }
}

TEST_F(AmlG12CompositeTest, SetDaiFormatsValid) {
  // Ids 1, 2, and 3 are valid configure HW DAI formats.

  SetDaiFormatDefault(1);
  // Configure TDM A for 32 bits I2S:
  // TDM OUT CTRL0 config, bitoffset 2, 2 slots, 32 bits per slot.
  ASSERT_EQ(0x3001'003F, GetMmioValue(0x500 / 4));
  // TDM IN CTRL0 config, PAD_TDMIN A, 32 bits per slot.
  ASSERT_EQ(0x7003'001F, GetMmioValue(0x300 / 4));

  SetDaiFormatDefault(2);
  // Configure TDM B for 32 bits I2S:
  // TDM OUT CTRL0 config, bitoffset 2, 2 slots, 32 bits per slot.
  ASSERT_EQ(0x3001'003F, GetMmioValue(0x540 / 4));
  // TDM IN CTRL0 config, PAD_TDMIN B, 32 bits per slot.
  ASSERT_EQ(0x7013'001F, GetMmioValue(0x340 / 4));

  SetDaiFormatDefault(3);
  // Configure TDM C for 32 bits I2S:
  // TDM OUT CTRL0 config, bitoffset 2, 2 slots, 32 bits per slot.
  ASSERT_EQ(0x3001'003F, GetMmioValue(0x580 / 4));
  // TDM IN CTRL0 config, PAD_TDMIN C, 32bits per slot.
  ASSERT_EQ(0x7023'001F, GetMmioValue(0x380 / 4));

  SetDaiFormatFrameFormat(1, fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatStandard(
                                 fuchsia_hardware_audio::DaiFrameFormatStandard::kTdm1));
  // TDM OUT CTRL0 config, reg_tdm_init_bitnum (bitoffset) 3
  ASSERT_EQ(0x3001'803F, GetMmioValue(0x500 / 4));
  // TDM IN CTRL0 config, reg_tdmin_in_bit_skew 4
  ASSERT_EQ(0x3004'001F, GetMmioValue(0x300 / 4));

  SetDaiFormatFrameFormat(1, fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatStandard(
                                 fuchsia_hardware_audio::DaiFrameFormatStandard::kTdm2));
  // TDM OUT CTRL0 config, reg_tdm_init_bitnum (bitoffset) 2
  ASSERT_EQ(0x3001'003F, GetMmioValue(0x500 / 4));
  // TDM IN CTRL0 config, reg_tdmin_in_bit_skew 3
  ASSERT_EQ(0x3003'001F, GetMmioValue(0x300 / 4));

  SetDaiFormatFrameFormat(1, fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatStandard(
                                 fuchsia_hardware_audio::DaiFrameFormatStandard::kTdm3));
  // TDM OUT CTRL0 config, reg_tdm_init_bitnum (bitoffset) 1
  ASSERT_EQ(0x3000'803F, GetMmioValue(0x500 / 4));
  // TDM IN CTRL0 config, reg_tdmin_in_bit_skew 2
  ASSERT_EQ(0x3002'001F, GetMmioValue(0x300 / 4));

  SetDaiFormatFrameFormat(1, fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatStandard(
                                 fuchsia_hardware_audio::DaiFrameFormatStandard::kStereoLeft));
  // TDM OUT CTRL0 config, reg_tdm_init_bitnum (bitoffset) 3
  ASSERT_EQ(0x3001'803F, GetMmioValue(0x500 / 4));
  // TDM IN CTRL0 config, reg_tdmin_in_bit_skew 4
  ASSERT_EQ(0x3004'001F, GetMmioValue(0x300 / 4));

  // Custom with sclk_on_raising=true, 1 channel.
  SetDaiFormatFrameFormatNumberOfChannels(
      1,
      fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatCustom(
          fuchsia_hardware_audio::DaiFrameFormatCustom(true, true, 1, 1)),
      1);
  // TDM OUT CTRL0 config, reg_tdm_init_bitnum (bitoffset) 2
  // (same as TDM2 for frame_sync_sclks_offset=1).
  ASSERT_EQ(0x3001'001F, GetMmioValue(0x500 / 4));
  // TDM IN CTRL0 config, reg_tdmin_in_bit_skew 3
  // (same as TDM2 for frame_sync_sclks_offset=1).
  ASSERT_EQ(0x3003'001F, GetMmioValue(0x300 / 4));
  // SCLK CTRL, clk in/out enabled, 24 sdiv, 1 lrduty, 16 lrdiv (1 channel).
  ASSERT_EQ(0xc180'001f, GetMmioValue(0x040 / 4));
  // SCLK CTRL, no clk_inv (sclk_on_raising=true)
  ASSERT_EQ(0x0000'0000, GetMmioValue(0x044 / 4));

  // Custom with sclk_on_raising=false, 2 channels.
  SetDaiFormatFrameFormat(1, fuchsia_hardware_audio::DaiFrameFormat::WithFrameFormatCustom(
                                 fuchsia_hardware_audio::DaiFrameFormatCustom(true, false, 1, 1)));
  // TDM OUT CTRL0 config, reg_tdm_init_bitnum (bitoffset) 2
  // (same as TDM2 for frame_sync_sclks_offset=1).
  ASSERT_EQ(0x3001'003F, GetMmioValue(0x500 / 4));
  // TDM IN CTRL0 config, reg_tdmin_in_bit_skew 3
  // (same as TDM2 for frame_sync_sclks_offset=1).
  ASSERT_EQ(0x3003'001F, GetMmioValue(0x300 / 4));
  // SCLK CTRL, clk in/out enabled, 24 sdiv, 1 lrduty, 32 lrdiv (2x16 bits channels).
  ASSERT_EQ(0xc180'003f, GetMmioValue(0x040 / 4));
  // SCLK CTRL, clk_inv ph0 (sclk_on_raising=false).
  ASSERT_EQ(0x0000'0001, GetMmioValue(0x044 / 4));

  SetDaiFormatFrameRate(1, 8'000);
  EXPECT_EQ(0x8400'003b, GetMmioValue(0x001));  // MCLK CTRL, div 60.

  SetDaiFormatFrameRate(1, 16'000);
  EXPECT_EQ(0x8400'001d, GetMmioValue(0x001));  // MCLK CTRL, div 30.

  SetDaiFormatFrameRate(1, 32'000);
  EXPECT_EQ(0x8400'000e, GetMmioValue(0x001));  // MCLK CTRL, div 15.

  SetDaiFormatFrameRate(1, 48'000);
  EXPECT_EQ(0x8400'0009, GetMmioValue(0x001));  // MCLK CTRL, div 10.

  SetDaiFormatFrameRate(1, 96'000);
  EXPECT_EQ(0x8400'0004, GetMmioValue(0x001));  // MCLK CTRL, div 5.

  SetDaiFormatFrameRate(2, 8'000);              // A change to id 2 does not affect id 1.
  SetDaiFormatFrameRate(3, 48'000);             // A change to id 3 does not affect id 1.
  EXPECT_EQ(0x8400'0004, GetMmioValue(0x001));  // MCLK CTRL, div 5.

  SetDaiFormatBitsPerSampleAndBitsPerSlot(1, 32, 32);
  // TDM OUT CTRL0 config, bitoffset 2, 2 slots, 32 bits per slot.
  ASSERT_EQ(0x3001'003F, GetMmioValue(0x500 / 4));
  // TDM OUT CTRL1 FRDDR with 32 bits per sample.
  ASSERT_EQ(0x0000'1f40, GetMmioValue(0x504 / 4));
  // SCLK CTRL, clk in/out enabled, 24 sdiv, 32 lrduty, 32 lrdiv.
  ASSERT_EQ(0xc180'7c3f, GetMmioValue(0x040 / 4));

  SetDaiFormatBitsPerSampleAndBitsPerSlot(1, 16, 16);
  // TDM OUT CTRL0 config, bitoffset 2, 2 slots, 16 bits per slot.
  ASSERT_EQ(0x3001'002F, GetMmioValue(0x500 / 4));
  // TDM OUT CTRL1 FRDDR with 16 bits per sample.
  ASSERT_EQ(0x0000'0f20, GetMmioValue(0x504 / 4));
  // SCLK CTRL, clk in/out enabled, 24 sdiv, 16 lrduty, 16 lrdiv.
  ASSERT_EQ(0xc180'3c1f, GetMmioValue(0x040 / 4));

  SetDaiFormatBitsPerSampleAndBitsPerSlot(1, 16, 32);
  // TDM OUT CTRL0 config, bitoffset 2, 2 slots, 32 bits per slot.
  ASSERT_EQ(0x3001'003F, GetMmioValue(0x500 / 4));
  // TDM OUT CTRL1 FRDDR with 16 bits per sample.
  ASSERT_EQ(0x0000'0f20, GetMmioValue(0x504 / 4));
  // SCLK CTRL, clk in/out enabled, 24 sdiv, 32 lrduty, 32 lrdiv.
  ASSERT_EQ(0xc180'7c3f, GetMmioValue(0x040 / 4));
}

TEST_F(AmlG12CompositeTest, GetRingBufferFormats) {
  {
    auto ring_buffer_formats_result = client()->GetRingBufferFormats(kInvalidBufferId0);
    ASSERT_FALSE(ring_buffer_formats_result.is_ok());
  }
  {
    auto ring_buffer_formats_result = client()->GetRingBufferFormats(kInvalidBufferId1);
    ASSERT_FALSE(ring_buffer_formats_result.is_ok());
  }
  {
    auto ring_buffer_formats_result = client()->GetRingBufferFormats(kEngineARingBufferIdOutput);
    ASSERT_TRUE(ring_buffer_formats_result.is_ok());
    // There is at least one entry reported.
    ASSERT_GE(1, ring_buffer_formats_result->ring_buffer_formats().size());
    // Only 2 bytes per sample supported.
    auto& first_format = ring_buffer_formats_result->ring_buffer_formats()[0];
    ASSERT_TRUE(first_format.pcm_supported_formats().has_value());
    auto& first_pcm_format = first_format.pcm_supported_formats().value();
    ASSERT_TRUE(first_pcm_format.bytes_per_sample());
    ASSERT_EQ(1, first_pcm_format.bytes_per_sample()->size());
    ASSERT_EQ(2, (*first_pcm_format.bytes_per_sample())[0]);
    ASSERT_EQ(1, first_pcm_format.valid_bits_per_sample()->size());
    ASSERT_EQ(16, (*first_pcm_format.valid_bits_per_sample())[0]);
  }
}

TEST_F(AmlG12CompositeTest, CreateRingBuffer) {
  {
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_audio::RingBuffer>();
    fuchsia_hardware_audio::CompositeCreateRingBufferRequest request(
        kInvalidBufferId0, GetDefaultRingBufferFormat(), std::move(endpoints->server));
    auto ring_buffer_result = client()->CreateRingBuffer(std::move(request));
    ASSERT_FALSE(ring_buffer_result.is_ok());
  }
  {
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_audio::RingBuffer>();
    fuchsia_hardware_audio::CompositeCreateRingBufferRequest request(
        kInvalidBufferId1, GetDefaultRingBufferFormat(), std::move(endpoints->server));
    auto ring_buffer_result = client()->CreateRingBuffer(std::move(request));
    ASSERT_FALSE(ring_buffer_result.is_ok());
  }
  // Any Ring Buffer field not in the supported ones returns an error.
  {
    auto pcm_format = GetDefaultRingBufferFormat().pcm_format().value();
    pcm_format.number_of_channels(123);  // Bad number of channels.
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_audio::RingBuffer>();
    fuchsia_hardware_audio::CompositeCreateRingBufferRequest request(
        kEngineARingBufferIdOutput,
        fuchsia_hardware_audio::Format2::WithPcmFormat(std::move(pcm_format)),
        std::move(endpoints->server));
    auto ring_buffer_result = client()->CreateRingBuffer(std::move(request));
    ASSERT_FALSE(ring_buffer_result.is_ok());
  }
  {
    auto pcm_format = GetDefaultRingBufferFormat().pcm_format().value();
    pcm_format.sample_format(fuchsia_hardware_audio::SampleFormat::kPcmFloat);  // Bad format.
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_audio::RingBuffer>();
    fuchsia_hardware_audio::CompositeCreateRingBufferRequest request(
        kEngineARingBufferIdOutput,
        fuchsia_hardware_audio::Format2::WithPcmFormat(std::move(pcm_format)),
        std::move(endpoints->server));
    auto ring_buffer_result = client()->CreateRingBuffer(std::move(request));
    ASSERT_FALSE(ring_buffer_result.is_ok());
  }
  {
    auto pcm_format = GetDefaultRingBufferFormat().pcm_format().value();
    pcm_format.bytes_per_sample(123);  // Bad bytes per sample.
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_audio::RingBuffer>();
    fuchsia_hardware_audio::CompositeCreateRingBufferRequest request(
        kEngineARingBufferIdOutput,
        fuchsia_hardware_audio::Format2::WithPcmFormat(std::move(pcm_format)),
        std::move(endpoints->server));
    auto ring_buffer_result = client()->CreateRingBuffer(std::move(request));
    ASSERT_FALSE(ring_buffer_result.is_ok());
  }
  {
    auto pcm_format = GetDefaultRingBufferFormat().pcm_format().value();
    pcm_format.valid_bits_per_sample(123);  // Bad valid bits per sample.
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_audio::RingBuffer>();
    fuchsia_hardware_audio::CompositeCreateRingBufferRequest request(
        kEngineARingBufferIdOutput,
        fuchsia_hardware_audio::Format2::WithPcmFormat(std::move(pcm_format)),
        std::move(endpoints->server));
    auto ring_buffer_result = client()->CreateRingBuffer(std::move(request));
    ASSERT_FALSE(ring_buffer_result.is_ok());
  }
  {
    auto pcm_format = GetDefaultRingBufferFormat().pcm_format().value();
    pcm_format.frame_rate(123);  // Bad frame rate.
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_audio::RingBuffer>();
    fuchsia_hardware_audio::CompositeCreateRingBufferRequest request(
        kEngineARingBufferIdOutput,
        fuchsia_hardware_audio::Format2::WithPcmFormat(std::move(pcm_format)),
        std::move(endpoints->server));
    auto ring_buffer_result = client()->CreateRingBuffer(std::move(request));
    ASSERT_FALSE(ring_buffer_result.is_ok());
  }
}

class AmlG12CompositeRingBufferTest : public AmlG12CompositeTest {
 protected:
  void SetUp() override { AmlG12CompositeTest::SetUp(); }

  fidl::SyncClient<fuchsia_hardware_audio::RingBuffer> GetRingBufferClient(
      fuchsia_hardware_audio::ElementId id) {
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_audio::RingBuffer>();
    fuchsia_hardware_audio::CompositeCreateRingBufferRequest request(
        id, GetDefaultRingBufferFormat(), std::move(endpoints->server));
    auto ring_buffer_result = client()->CreateRingBuffer(std::move(request));
    ZX_ASSERT(ring_buffer_result.is_ok());
    return fidl::SyncClient<fuchsia_hardware_audio::RingBuffer>(std::move(endpoints->client));
  }

  fidl::SyncClient<fuchsia_hardware_audio::RingBuffer> GetRingBufferClientReadyForSetActiveChannels(
      fuchsia_hardware_audio::ElementId id, uint32_t number_of_channels) {
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_audio::RingBuffer>();
    auto pcm_format = GetDefaultRingBufferFormat().pcm_format().value();
    pcm_format.number_of_channels(number_of_channels);
    fuchsia_hardware_audio::Format2 format =
        fuchsia_hardware_audio::Format2::WithPcmFormat(std::move(pcm_format));
    fuchsia_hardware_audio::CompositeCreateRingBufferRequest request(id, std::move(format),
                                                                     std::move(endpoints->server));
    auto ring_buffer_result = client()->CreateRingBuffer(std::move(request));
    ZX_ASSERT(ring_buffer_result.is_ok());
    auto ring_buffer_client =
        fidl::SyncClient<fuchsia_hardware_audio::RingBuffer>(std::move(endpoints->client));

    constexpr uint32_t kMinFrames = 8192;
    auto get_vmo_result =
        ring_buffer_client->GetVmo(fuchsia_hardware_audio::RingBufferGetVmoRequest(kMinFrames, 0));
    ZX_ASSERT(get_vmo_result.is_ok());

    return ring_buffer_client;
  }

  void TestProperties(fuchsia_hardware_audio::ElementId id) {
    auto client = GetRingBufferClient(id);
    fidl::Result result = client->GetProperties();
    ASSERT_TRUE(result.is_ok());
    ASSERT_TRUE(result->properties().needs_cache_flush_or_invalidate().has_value());
    ASSERT_TRUE(*result->properties().needs_cache_flush_or_invalidate());
    ASSERT_TRUE(result->properties().driver_transfer_bytes().has_value());
  }

  void TestGetVmo(fuchsia_hardware_audio::ElementId id) {
    auto client = GetRingBufferClient(id);
    constexpr uint32_t kMinFrames = 1;
    auto get_vmo_result =
        client->GetVmo(fuchsia_hardware_audio::RingBufferGetVmoRequest(kMinFrames, 0));
    ASSERT_TRUE(get_vmo_result.is_ok());
    ASSERT_TRUE(get_vmo_result->ring_buffer().is_valid());
  }

  void TestGetVmoSize(fuchsia_hardware_audio::ElementId id) {
    auto client = GetRingBufferClient(id);
    constexpr uint32_t kMinFrames = 100;
    auto get_vmo_result =
        client->GetVmo(fuchsia_hardware_audio::RingBufferGetVmoRequest(kMinFrames, 0));
    ASSERT_TRUE(get_vmo_result.is_ok());
    ASSERT_TRUE(get_vmo_result->ring_buffer().is_valid());

    ASSERT_GE(get_vmo_result->num_frames(), kMinFrames);

    uint64_t vmo_size;
    ASSERT_EQ(ZX_OK, get_vmo_result->ring_buffer().get_size(&vmo_size));
    fuchsia_hardware_audio::Format2 format = GetDefaultRingBufferFormat();
    auto& pcm_format = format.pcm_format().value();
    uint32_t frame_size = pcm_format.number_of_channels() * pcm_format.bytes_per_sample();
    ASSERT_GE(vmo_size, kMinFrames * frame_size);
  }

  void TestGetVmoMultipleTimes(fuchsia_hardware_audio::ElementId id) {
    auto client = GetRingBufferClient(id);
    constexpr uint32_t kMinFrames = 1;
    auto get_vmo_result0 =
        client->GetVmo(fuchsia_hardware_audio::RingBufferGetVmoRequest(kMinFrames, 0));
    ASSERT_TRUE(get_vmo_result0.is_ok());
    ASSERT_TRUE(get_vmo_result0->ring_buffer().is_valid());

    auto get_vmo_result1 =
        client->GetVmo(fuchsia_hardware_audio::RingBufferGetVmoRequest(kMinFrames, 0));
    ASSERT_FALSE(get_vmo_result1.is_ok());
  }

  void TestStartRingBufferNoVmo(fuchsia_hardware_audio::ElementId id) {
    auto client = GetRingBufferClient(id);
    auto start_result = client->Start();
    ASSERT_FALSE(start_result.is_ok());  // Not ok to start a ring buffer with no VMO.
  }

  void TestStopRingBufferNoVmo(fuchsia_hardware_audio::ElementId id) {
    auto client = GetRingBufferClient(id);
    auto stop_result = client->Stop();
    ASSERT_FALSE(stop_result.is_ok());  // Not ok to stop a ring buffer with no VMO.
  }

  void TestCreationAndStartStop(fuchsia_hardware_audio::ElementId id) {
    auto client = GetRingBufferClient(id);
    constexpr uint32_t kMinFrames = 8192;
    auto get_vmo_result =
        client->GetVmo(fuchsia_hardware_audio::RingBufferGetVmoRequest(kMinFrames, 0));
    ASSERT_TRUE(get_vmo_result.is_ok());

    auto start_result = client->Start();
    ASSERT_TRUE(start_result.is_ok());

    auto stop_result = client->Stop();
    ASSERT_TRUE(stop_result.is_ok());
  }

  void TestStartStartedRingBuffer(fuchsia_hardware_audio::ElementId id) {
    auto client = GetRingBufferClient(id);
    constexpr uint32_t kMinFrames = 8192;
    auto get_vmo_result =
        client->GetVmo(fuchsia_hardware_audio::RingBufferGetVmoRequest(kMinFrames, 0));
    ASSERT_TRUE(get_vmo_result.is_ok());

    auto start_result0 = client->Start();
    ASSERT_TRUE(start_result0.is_ok());

    auto start_result1 = client->Start();
    ASSERT_FALSE(start_result1.is_ok());  // Not ok to start a started ring buffer.
  }

  void TestStopStoppedRingBuffer(fuchsia_hardware_audio::ElementId id) {
    auto client = GetRingBufferClient(id);
    constexpr uint32_t kMinFrames = 8192;
    auto get_vmo_result =
        client->GetVmo(fuchsia_hardware_audio::RingBufferGetVmoRequest(kMinFrames, 0));
    ASSERT_TRUE(get_vmo_result.is_ok());

    auto start_result = client->Start();
    ASSERT_TRUE(start_result.is_ok());

    auto stop_result0 = client->Stop();
    ASSERT_TRUE(stop_result0.is_ok());

    auto stop_result1 = client->Stop();
    ASSERT_TRUE(stop_result1.is_ok());  // Ok to stop a stopped ring buffer.
  }

  void TestGetDelay(fuchsia_hardware_audio::ElementId id) {
    auto client = GetRingBufferClient(id);
    auto delay_result = client->WatchDelayInfo();
    ASSERT_TRUE(delay_result.is_ok());
    ASSERT_TRUE(delay_result->delay_info().internal_delay().has_value());  // Some delay reported.
  }

  void TestInvalidActiveChannelsFor1Channel(fuchsia_hardware_audio::ElementId id) {
    auto client = GetRingBufferClientReadyForSetActiveChannels(id, 1);

    {
      // Bit 0 is a valid channel.
      auto set_active_channels_result = client->SetActiveChannels(1);
      ASSERT_TRUE(set_active_channels_result.is_ok());
    }
    {
      // Bit 1 is not a valid channel.
      auto set_active_channels_result = client->SetActiveChannels(2);
      ASSERT_FALSE(set_active_channels_result.is_ok());
    }
    {
      // Bit 2 is not a valid channel.
      auto set_active_channels_result = client->SetActiveChannels(4);
      ASSERT_FALSE(set_active_channels_result.is_ok());
    }
  }

  void TestInvalidActiveChannelsFor2Channels(fuchsia_hardware_audio::ElementId id) {
    auto client = GetRingBufferClientReadyForSetActiveChannels(id, 2);

    {
      // Bit 0 is a valid channel.
      auto set_active_channels_result = client->SetActiveChannels(1);
      ASSERT_TRUE(set_active_channels_result.is_ok());
    }
    {
      // Bit 1 is a valid channel.
      auto set_active_channels_result = client->SetActiveChannels(2);
      ASSERT_TRUE(set_active_channels_result.is_ok());
    }
    {
      // Bit 2 is not a valid channel.
      auto set_active_channels_result = client->SetActiveChannels(4);
      ASSERT_FALSE(set_active_channels_result.is_ok());
    }
    {
      // Bit 3 is not a valid channel.
      auto set_active_channels_result = client->SetActiveChannels(8);
      ASSERT_FALSE(set_active_channels_result.is_ok());
    }
  }
};

TEST_F(AmlG12CompositeRingBufferTest, RingBufferProperties) {
  for (const fuchsia_hardware_audio::ElementId& id : kAllValidRingBufferIds) {
    TestProperties(id);
  }
}

TEST_F(AmlG12CompositeRingBufferTest, RingBufferGetVmo) {
  for (const fuchsia_hardware_audio::ElementId& id : kAllValidRingBufferIds) {
    TestGetVmo(id);
  }
}

TEST_F(AmlG12CompositeRingBufferTest, RingBufferGetVmoSize) {
  for (const fuchsia_hardware_audio::ElementId& id : kAllValidRingBufferIds) {
    TestGetVmoSize(id);
  }
}

TEST_F(AmlG12CompositeRingBufferTest, RingBufferGetVmoMultipleTimes) {
  for (const fuchsia_hardware_audio::ElementId& id : kAllValidRingBufferIds) {
    TestGetVmoMultipleTimes(id);
  }
}

TEST_F(AmlG12CompositeRingBufferTest, RingBuffersStartStop) {
  for (const fuchsia_hardware_audio::ElementId& id : kAllValidRingBufferIds) {
    TestCreationAndStartStop(id);
  }
}

TEST_F(AmlG12CompositeRingBufferTest, RingBuffersClientCloseChannel) {
  for (const fuchsia_hardware_audio::ElementId& id : kAllValidRingBufferIds) {
    auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_audio::RingBuffer>();
    fuchsia_hardware_audio::CompositeCreateRingBufferRequest request(
        id, GetDefaultRingBufferFormat(), std::move(endpoints->server));
    auto ring_buffer_result = client()->CreateRingBuffer(std::move(request));
    ASSERT_TRUE(ring_buffer_result.is_ok());

    endpoints->client.TakeChannel().reset();
  }
}

TEST_F(AmlG12CompositeRingBufferTest, RingBufferStartStartedRingBuffer) {
  for (const fuchsia_hardware_audio::ElementId& id : kAllValidRingBufferIds) {
    TestStartStartedRingBuffer(id);
  }
}

TEST_F(AmlG12CompositeRingBufferTest, RingBufferStopStoppedRingBuffer) {
  for (const fuchsia_hardware_audio::ElementId& id : kAllValidRingBufferIds) {
    TestStopStoppedRingBuffer(id);
  }
}

TEST_F(AmlG12CompositeRingBufferTest, RingBufferGetDelay) {
  for (const fuchsia_hardware_audio::ElementId& id : kAllValidRingBufferIds) {
    TestGetDelay(id);
  }
}

TEST_F(AmlG12CompositeRingBufferTest, DefaultPowerState) {
  auto client0 = GetRingBufferClient(kEngineARingBufferIdOutput);
  TestPowerState(true);  // Default to on.
  ASSERT_TRUE(client0->SetActiveChannels(0).is_ok());
  TestPowerState(false);  // Was set to off.

  auto client1 = GetRingBufferClient(kEngineARingBufferIdOutput);
  TestPowerState(true);  // Another ring buffer request, then back to on.
}

TEST_F(AmlG12CompositeRingBufferTest, PowerSuspendResumeOneRingBuffer) {
  // Clear active channels for one ring buffer.
  auto client0 = GetRingBufferClientReadyForSetActiveChannels(kAllValidRingBufferIds[0], 2);
  auto result0 = client0->SetActiveChannels(0);
  ASSERT_TRUE(result0.is_ok());
  TestPowerState(false);

  // Set one active channel in one ring buffer.
  auto client1 = GetRingBufferClientReadyForSetActiveChannels(kAllValidRingBufferIds[0], 2);
  auto result1 = client1->SetActiveChannels(1);
  ASSERT_TRUE(result1.is_ok());
  TestPowerState(true);
}

TEST_F(AmlG12CompositeRingBufferTest, PowerSuspendResumMultipleRingBuffers) {
  auto client0 = GetRingBufferClientReadyForSetActiveChannels(kAllValidRingBufferIds[0], 2);
  auto client1 = GetRingBufferClientReadyForSetActiveChannels(kAllValidRingBufferIds[1], 2);
  auto client2 = GetRingBufferClientReadyForSetActiveChannels(kAllValidRingBufferIds[2], 2);

  // After disabling one ring buffer, power is still enabled.
  ASSERT_TRUE(client0->SetActiveChannels(0).is_ok());
  TestPowerState(true);

  // After all ring buffers created are disabled, power is disabled.
  ASSERT_TRUE(client1->SetActiveChannels(0).is_ok());
  ASSERT_TRUE(client2->SetActiveChannels(0).is_ok());
  TestPowerState(false);

  // After enabling a channel in a different ring buffer, power is enabled.
  ASSERT_TRUE(client2->SetActiveChannels(1).is_ok());
  TestPowerState(true);
}

TEST_F(AmlG12CompositeRingBufferTest, InvalidSetActiveChannels) {
  for (auto& id : kAllValidRingBufferIds) {
    TestInvalidActiveChannelsFor1Channel(id);
    TestInvalidActiveChannelsFor2Channels(id);
  }
}

// Calling GetVmo while powered-down should still succeed.
// To set the stream's memory range, we program DMA registers; this requires being powered-up.
// So if needed, we power-up the stream beforehand and restore the original power state afterward.
TEST_F(AmlG12CompositeRingBufferTest, GetVmoWhilePowerStopped) {
  // Create the RingBuffer client, but don't retrieve a VMO yet.
  auto client = GetRingBufferClient(kAllValidRingBufferIds[0]);
  // Setting active channels to 0 tells this endpoint to power-down.
  ASSERT_TRUE(client->SetActiveChannels(0).is_ok());
  // Verify that we are indeed powered-down.
  TestPowerState(false);

  // We verify this temporary power-up by observing an increase in enable-count.
  auto initial_enable_count = GetFakeClockGateEnableCount();

  constexpr uint32_t kMinFrames = 8192;
  auto vmo = client->GetVmo(fuchsia_hardware_audio::RingBufferGetVmoRequest(kMinFrames, 0));
  ASSERT_TRUE(vmo.is_ok());

  // Verify that SoC power was temporarily enabled during GetVmo...
  EXPECT_GT(GetFakeClockGateEnableCount(), initial_enable_count);
  // ...and then turned back off afterward.
  TestPowerState(false);
}

}  // namespace audio::aml_g12
