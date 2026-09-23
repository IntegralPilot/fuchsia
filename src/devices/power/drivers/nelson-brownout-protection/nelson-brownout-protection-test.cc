// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "nelson-brownout-protection.h"

#include <lib/async-default/include/lib/async/default.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/simple-codec/simple-codec-server.h>

#include <optional>

#include <zxtest/zxtest.h>

#include "src/devices/gpio/testing/fake-gpio/fake-gpio.h"
#include "src/devices/testing/mock-ddk/mock-device.h"

namespace brownout_protection {

namespace audio_fidl = ::fuchsia::hardware::audio;
namespace signal_fidl = ::fuchsia::hardware::audio::signalprocessing;

class FakeCodec final : public audio::SimpleCodecServer, public signal_fidl::SignalProcessing {
 public:
  FakeCodec(zx_device_t* parent) : SimpleCodecServer(parent) {}
  fuchsia_hardware_audio::CodecService::InstanceHandler GetInstanceHandler() {
    return fuchsia_hardware_audio::CodecService::InstanceHandler({
        .codec =
            [this](fidl::ServerEnd<fuchsia_hardware_audio::Codec> server_end) {
              this->CodecConnect(server_end.TakeChannel());
            },
    });
  }

  zx_status_t Shutdown() override { return ZX_OK; }

  // The test can check directly the state of AGL enablement in its thread.
  bool agl_enabled() { return agl_enabled_; }

 private:
  zx::result<audio::DriverIds> Initialize() override {
    return zx::ok(audio::DriverIds{.vendor_id = 0, .device_id = 0});
  }
  zx_status_t Reset() override { return ZX_ERR_NOT_SUPPORTED; }
  audio::Info GetInfo() override {
    return {
        .unique_id = {{1, 2, 3, 4}},
        .manufacturer = "test man",
        .product_name = "test prod",
    };
  }
  zx_status_t Stop() override { return ZX_ERR_NOT_SUPPORTED; }
  zx_status_t Start() override { return ZX_OK; }
  bool SupportsSignalProcessing() override { return true; }
  void SignalProcessingConnect(
      fidl::InterfaceRequest<signal_fidl::SignalProcessing> signal_processing) override {
    signal_processing_binding_.emplace(this, std::move(signal_processing), dispatcher());
  }
  void GetElements(GetElementsCallback callback) override {
    signal_fidl::Element pe;
    pe.set_id(kAglPeId);
    pe.set_type(signal_fidl::ElementType::AUTOMATIC_GAIN_LIMITER);
    pe.set_can_stop(true);
    std::vector<signal_fidl::Element> pes;
    pes.emplace_back(std::move(pe));
    signal_fidl::Reader_GetElements_Response response(std::move(pes));
    signal_fidl::Reader_GetElements_Result result;
    result.set_response(std::move(response));
    callback(std::move(result));
  }
  void SetElementState(uint64_t processing_element_id, signal_fidl::SettableElementState state,
                       SetElementStateCallback callback) override {
    ASSERT_EQ(processing_element_id, kAglPeId);
    ASSERT_TRUE(state.has_started());
    agl_enabled_ = state.started() && (!state.has_bypassed() || !state.bypassed());
    callback(signal_fidl::SignalProcessing_SetElementState_Result::WithResponse(
        signal_fidl::SignalProcessing_SetElementState_Response()));
  }
  void WatchElementState(uint64_t processing_element_id,
                         WatchElementStateCallback callback) override {}
  void GetTopologies(GetTopologiesCallback callback) override {
    callback(signal_fidl::Reader_GetTopologies_Result::WithErr(ZX_ERR_NOT_SUPPORTED));
  }
  void SetTopology(uint64_t topology_id, SetTopologyCallback callback) override {
    callback(signal_fidl::SignalProcessing_SetTopology_Result::WithErr(ZX_ERR_NOT_SUPPORTED));
  }
  void WatchTopology(WatchTopologyCallback callback) override {}
  audio::DaiSupportedFormats GetDaiFormats() override { return {}; }
  zx::result<audio::CodecFormatInfo> SetDaiFormat(const audio::DaiFormat& format) override {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  audio::GainFormat GetGainFormat() override { return {.min_gain = -103.0f}; }
  audio::GainState GetGainState() override { return gain_state; }
  void SetGainState(audio::GainState state) override { gain_state = state; }
  inspect::Inspector& inspect() { return SimpleCodecServer::inspect(); }

  void handle_unknown_method(uint64_t ordinal, bool method_has_response) override {}

 private:
  static constexpr uint64_t kAglPeId = 1;

  audio::GainState gain_state = {};
  // agl_enabled_ is accessed from different threads in SetAgl() and agl_enabled().
  std::atomic<bool> agl_enabled_ = false;
  std::optional<fidl::Binding<signal_fidl::SignalProcessing>> signal_processing_binding_;
};

class FakePowerSensor : public fidl::WireServer<fuchsia_hardware_power_sensor::Device> {
 public:
  void set_voltage(float voltage) { voltage_ = voltage; }

  void GetPowerWatts(GetPowerWattsCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void GetVoltageVolts(GetVoltageVoltsCompleter::Sync& completer) override {
    completer.ReplySuccess(voltage_);
  }

  void GetSensorName(GetSensorNameCompleter::Sync& completer) override {}

  fuchsia_hardware_power_sensor::Service::InstanceHandler CreateInstanceHandler() {
    auto* dispatcher = async_get_default_dispatcher();
    Handler device_handler =
        [impl = this, dispatcher = dispatcher](
            ::fidl::ServerEnd<::fuchsia_hardware_power_sensor::Device> request) {
          impl->bindings_.AddBinding(dispatcher, std::move(request), impl,
                                     fidl::kIgnoreBindingClosure);
        };

    return fuchsia_hardware_power_sensor::Service::InstanceHandler(
        {.device = std::move(device_handler)});
  }

 private:
  std::atomic<float> voltage_ = 0.0f;
  fidl::ServerBindingGroup<fuchsia_hardware_power_sensor::Device> bindings_;
};

class NelsonBrownoutProtectionTest : public zxtest::Test {
 public:
  void SetUp() override {
    ASSERT_OK(incoming_namespace_loop_.StartThread("incoming_namespace"));

    SetupPowerSensorFragment();
    SetupCodecFragment();
    SetupAlertGpioFragment();
  }

 protected:
  std::shared_ptr<MockDevice> fake_parent() { return fake_parent_; }
  FakeCodec* codec() { return codec_; }
  async_patterns::TestDispatcherBound<FakePowerSensor>& power_sensor() { return power_sensor_; }
  zx::interrupt& alert_gpio_interrupt() { return alert_gpio_interrupt_; }
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio>& alert_gpio() { return alert_gpio_; }

 private:
  void SetupPowerSensorFragment() {
    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    auto power_handler = power_sensor_.SyncCall(&FakePowerSensor::CreateInstanceHandler);
    zx::result service_result = power_outgoing_.SyncCall(
        [handler = std::move(power_handler)](component::OutgoingDirectory* outgoing) mutable {
          return outgoing->AddService<fuchsia_hardware_power_sensor::Service>(std::move(handler));
        });
    ZX_ASSERT(service_result.is_ok());
    ZX_ASSERT(
        power_outgoing_.SyncCall(&component::OutgoingDirectory::Serve, std::move(endpoints.server))
            .is_ok());
    fake_parent_->AddFidlService(fuchsia_hardware_power_sensor::Service::Name,
                                 std::move(endpoints.client), "power-sensor");
  }

  void SetupCodecFragment() {
    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_OK(audio::SimpleCodecServer::CreateAndAddToDdk<FakeCodec>(fake_parent_.get()));
    auto* child_dev = fake_parent_->GetLatestChild();
    ASSERT_NOT_NULL(child_dev);
    codec_ = child_dev->GetDeviceContext<FakeCodec>();
    auto codec_handler = codec_->GetInstanceHandler();
    zx::result service_result = codec_outgoing_.SyncCall(
        [handler = std::move(codec_handler)](component::OutgoingDirectory* outgoing) mutable {
          return outgoing->AddService<fuchsia_hardware_audio::CodecService>(std::move(handler));
        });
    ZX_ASSERT(service_result.is_ok());
    ZX_ASSERT(
        codec_outgoing_.SyncCall(&component::OutgoingDirectory::Serve, std::move(endpoints.server))
            .is_ok());
    fake_parent_->AddFidlService(fuchsia_hardware_audio::CodecService::Name,
                                 std::move(endpoints.client), "codec");
  }

  void SetupAlertGpioFragment() {
    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_OK(
        zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &alert_gpio_interrupt_));
    zx::interrupt interrupt;
    ASSERT_OK(alert_gpio_interrupt_.duplicate(ZX_RIGHT_SAME_RIGHTS, &interrupt));
    alert_gpio_.SyncCall(&fake_gpio::FakeGpio::SetInterrupt, zx::ok(std::move(interrupt)));
    auto alert_gpio_handler = alert_gpio_.SyncCall(&fake_gpio::FakeGpio::CreateInstanceHandler);
    zx::result service_result = alert_gpio_outgoing_.SyncCall(
        [handler = std::move(alert_gpio_handler)](component::OutgoingDirectory* outgoing) mutable {
          return outgoing->AddService<fuchsia_hardware_gpio::Service>(std::move(handler));
        });
    ZX_ASSERT(service_result.is_ok());
    ZX_ASSERT(alert_gpio_outgoing_
                  .SyncCall(&component::OutgoingDirectory::Serve, std::move(endpoints.server))
                  .is_ok());
    fake_parent_->AddFidlService(fuchsia_hardware_gpio::Service::Name, std::move(endpoints.client),
                                 "alert-gpio");
  }

  std::shared_ptr<MockDevice> fake_parent_ = MockDevice::FakeRootParent();
  async::Loop incoming_namespace_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};

  zx::interrupt alert_gpio_interrupt_;
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio> alert_gpio_{
      incoming_namespace_loop_.dispatcher(), std::in_place};
  async_patterns::TestDispatcherBound<component::OutgoingDirectory> alert_gpio_outgoing_{
      incoming_namespace_loop_.dispatcher(), std::in_place, async_patterns::PassDispatcher};

  async_patterns::TestDispatcherBound<FakePowerSensor> power_sensor_{
      incoming_namespace_loop_.dispatcher(), std::in_place};
  async_patterns::TestDispatcherBound<component::OutgoingDirectory> power_outgoing_{
      incoming_namespace_loop_.dispatcher(), std::in_place, async_patterns::PassDispatcher};

  FakeCodec* codec_;
  async_patterns::TestDispatcherBound<component::OutgoingDirectory> codec_outgoing_{
      incoming_namespace_loop_.dispatcher(), std::in_place, async_patterns::PassDispatcher};
};

TEST_F(NelsonBrownoutProtectionTest, Test) {
  ASSERT_OK(NelsonBrownoutProtection::Create(nullptr, fake_parent().get(), zx::duration{0}));
  auto* child_dev2 = fake_parent()->GetLatestChild();
  ASSERT_NOT_NULL(child_dev2);
  child_dev2->InitOp();
  EXPECT_FALSE(codec()->agl_enabled());

  // Must be less than 11.5 to stay in the brownout state.
  power_sensor().SyncCall(&FakePowerSensor::set_voltage, 10.0f);

  alert_gpio_interrupt().trigger(0, zx::clock::get_boot());

  while (!codec()->agl_enabled()) {
  }

  // End the brownout state and make sure AGL gets disabled.
  power_sensor().SyncCall(&FakePowerSensor::set_voltage, 12.0f);

  while (codec()->agl_enabled()) {
  }

  std::vector alert_gpio_states = alert_gpio().SyncCall(&fake_gpio::FakeGpio::GetStateLog);
  ASSERT_GE(alert_gpio_states.size(), 1);
  ASSERT_EQ(fake_gpio::ReadSubState{}, alert_gpio_states[0].sub_state);
}

}  // namespace brownout_protection
