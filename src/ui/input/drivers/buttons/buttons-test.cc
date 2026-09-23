// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "buttons.h"

#include <fidl/fuchsia.hardware.gpio/cpp/wire_test_base.h>
#include <fidl/fuchsia.input.report/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/test_base.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <set>

#include <gtest/gtest.h>

#include "src/devices/gpio/testing/fake-gpio/fake-gpio.h"
#include "src/lib/testing/predicates/status.h"

namespace {

const std::vector<fuchsia_buttons::GpioButtonConfig> kButtonsDirect = {{{
    .type = fuchsia_buttons::GpioButtonType::WithDirect({}),
    .gpio_a_index = 0,
    .id = fuchsia_buttons::GpioButtonId::kVolumeUp,
}}};

const std::vector<fuchsia_buttons::GpioConfig> kGpiosDirect = {
    {{.type = fuchsia_buttons::GpioType::WithInterrupt({}), .flags = {{}}}},
};

const std::vector<fuchsia_buttons::GpioButtonConfig> kButtonsMultiple = {
    {{
        .type = fuchsia_buttons::GpioButtonType::WithDirect({}),
        .gpio_a_index = 0,
        .id = fuchsia_buttons::GpioButtonId::kVolumeUp,
    }},
    {{
        .type = fuchsia_buttons::GpioButtonType::WithDirect({}),
        .gpio_a_index = 1,
        .id = fuchsia_buttons::GpioButtonId::kMicMute,
    }},
    {{
        .type = fuchsia_buttons::GpioButtonType::WithDirect({}),
        .gpio_a_index = 2,
        .id = fuchsia_buttons::GpioButtonId::kCamMute,
    }},
};

const std::vector<fuchsia_buttons::GpioConfig> kGpiosMultiple = {
    {{.type = fuchsia_buttons::GpioType::WithInterrupt({}), .flags = {{}}}},
    {{.type = fuchsia_buttons::GpioType::WithInterrupt({}), .flags = {{}}}},
    {{.type = fuchsia_buttons::GpioType::WithInterrupt({}), .flags = {{}}}},
};

const std::vector<fuchsia_buttons::GpioConfig> kGpiosMultipleOnePolled = {
    {{.type = fuchsia_buttons::GpioType::WithInterrupt({}), .flags = {{}}}},
    {{.type = fuchsia_buttons::GpioType::WithPoll({{.period = zx::msec(20).get()}}),
      .flags = {{}}}},
    {{.type = fuchsia_buttons::GpioType::WithInterrupt({}), .flags = {{}}}},
};

const std::vector<fuchsia_buttons::GpioButtonConfig> kButtonsMatrix = {
    {{
        .type = fuchsia_buttons::GpioButtonType::WithMatrix({{
            .gpio_b_index = 2,
        }}),
        .gpio_a_index = 0,
        .id = fuchsia_buttons::GpioButtonId::kVolumeUp,
    }},
    {{
        .type = fuchsia_buttons::GpioButtonType::WithMatrix({{
            .gpio_b_index = 2,
        }}),
        .gpio_a_index = 1,
        .id = fuchsia_buttons::GpioButtonId::kKeyA,
    }},
    {{
        .type = fuchsia_buttons::GpioButtonType::WithMatrix({{
            .gpio_b_index = 3,
        }}),
        .gpio_a_index = 0,
        .id = fuchsia_buttons::GpioButtonId::kKeyM,
    }},
    {{
        .type = fuchsia_buttons::GpioButtonType::WithMatrix({{
            .gpio_b_index = 3,
        }}),
        .gpio_a_index = 1,
        .id = fuchsia_buttons::GpioButtonId::kPlayPause,
    }},
};

const std::vector<fuchsia_buttons::GpioConfig> kGpiosMatrix = {
    {{.type = fuchsia_buttons::GpioType::WithInterrupt({}), .flags = {{}}}},
    {{.type = fuchsia_buttons::GpioType::WithInterrupt({}), .flags = {{}}}},
    {{.type = fuchsia_buttons::GpioType::WithMatrixOutput({{.output_value = 0}}), .flags = {{}}}},
    {{.type = fuchsia_buttons::GpioType::WithMatrixOutput({{.output_value = 0}}), .flags = {{}}}},
};

const std::vector<fuchsia_buttons::GpioButtonConfig> kButtonsDuplicate = {
    {{
        .type = fuchsia_buttons::GpioButtonType::WithDirect({}),
        .gpio_a_index = 0,
        .id = fuchsia_buttons::GpioButtonId::kVolumeUp,
    }},
    {{
        .type = fuchsia_buttons::GpioButtonType::WithDirect({}),
        .gpio_a_index = 1,
        .id = fuchsia_buttons::GpioButtonId::kVolumeDown,
    }},
    {{
        .type = fuchsia_buttons::GpioButtonType::WithDirect({}),
        .gpio_a_index = 2,
        .id = fuchsia_buttons::GpioButtonId::kFdr,
    }},
};

const std::vector<fuchsia_buttons::GpioConfig> kGpiosDuplicate = {
    {{.type = fuchsia_buttons::GpioType::WithInterrupt({}), .flags = {{}}}},
    {{.type = fuchsia_buttons::GpioType::WithInterrupt({}), .flags = {{}}}},
    {{.type = fuchsia_buttons::GpioType::WithInterrupt({}), .flags = {{}}}},
};

}  // namespace

namespace buttons {

static constexpr size_t kMaxGpioServers = 4;

enum MetadataVersion : uint8_t {
  kMetadataSingleButtonDirect = 0,
  kMetadataMultiple,
  kMetadataDuplicate,
  kMetadataMatrix,
  kMetadataPolled,
};

class LocalFakeGpio : public fake_gpio::FakeGpio {
 public:
  LocalFakeGpio() = default;
  void SetExpectedInterruptOptions(fuchsia_hardware_gpio::InterruptOptions options) {
    expected_interrupt_options_ = options;
  }
  void SetExpectedInterruptMode(fuchsia_hardware_gpio::InterruptMode mode) {
    expected_interrupt_mode_ = mode;
  }

 private:
  void ConfigureInterrupt(ConfigureInterruptRequestView request,
                          ConfigureInterruptCompleter::Sync& completer) override {
    ASSERT_TRUE(request->config.has_mode());
    if (check_interrupt_mode_) {
      EXPECT_EQ(request->config.mode(), expected_interrupt_mode_);
    }
    FakeGpio::ConfigureInterrupt(request, completer);
  }
  void GetInterrupt(GetInterruptRequestView request,
                    GetInterruptCompleter::Sync& completer) override {
    EXPECT_EQ(request->options, expected_interrupt_options_);
    check_interrupt_mode_ = false;
    FakeGpio::GetInterrupt(request, completer);
  }
  fuchsia_hardware_gpio::InterruptOptions expected_interrupt_options_;
  fuchsia_hardware_gpio::InterruptMode expected_interrupt_mode_ =
      fuchsia_hardware_gpio::InterruptMode::kEdgeHigh;
  bool check_interrupt_mode_ = true;
};

class FakeSystemActivityGovernor
    : public fidl::testing::TestBase<fuchsia_power_system::ActivityGovernor> {
 public:
  fidl::ProtocolHandler<fuchsia_power_system::ActivityGovernor> CreateHandler() {
    return bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

  zx::eventpair AcquireWakeLease() {
    has_wake_lease_been_taken_ = true;
    zx::eventpair wake_lease_remote, wake_lease_local;
    zx::eventpair::create(0, &wake_lease_local, &wake_lease_remote);
    wake_leases_.push_back(std::move(wake_lease_local));
    return wake_lease_remote;
  }

  void AcquireWakeLease(AcquireWakeLeaseRequest& request,
                        AcquireWakeLeaseCompleter::Sync& completer) override {
    completer.Reply(fit::ok(AcquireWakeLease()));
  }

  void RegisterSuspendBlocker(RegisterSuspendBlockerRequest& request,
                              RegisterSuspendBlockerCompleter::Sync& completer) override {
    suspend_blocker_client_.Bind(std::move(request.suspend_blocker().value()),
                                 fdf::Dispatcher::GetCurrent()->async_dispatcher());
    suspend_blocker_client_->BeforeSuspend().Then([this](auto unused) { suspend_started_ = true; });

    // Use a fake lease token. There's no need to keep our handle alive.
    zx::eventpair lease_token, peer;
    zx::eventpair::create(0, &lease_token, &peer);
    fuchsia_power_system::ActivityGovernorRegisterSuspendBlockerResponse response;
    response.token() = std::move(lease_token);

    completer.Reply(fit::ok(std::move(response)));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    fdf::error("unexpected call to {}", name);
  }

  size_t HasWakeLeaseBeenTaken() const { return has_wake_lease_been_taken_; }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_system::ActivityGovernor> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  std::vector<zx::eventpair> wake_leases_;
  bool suspend_started_ = false;
  fidl::Client<fuchsia_power_system::SuspendBlocker> suspend_blocker_client_;
  fidl::ServerBindingGroup<fuchsia_power_system::ActivityGovernor> bindings_;
  bool has_wake_lease_been_taken_ = false;
};

class ButtonsTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    // Serve fake GPIO servers.
    EXPECT_LE(gpios_.size(), kMaxGpioServers);
    for (size_t i = 0; i < gpios_.size(); ++i) {
      EXPECT_OK(zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL,
                                      &fake_gpio_interrupts_[i]));
      zx::interrupt interrupt;
      EXPECT_OK(fake_gpio_interrupts_[i].duplicate(ZX_RIGHT_SAME_RIGHTS, &interrupt));
      fake_gpio_servers_[i].SetInterrupt(zx::ok(std::move(interrupt)));
      auto gpio_handler = fake_gpio_servers_[i].CreateInstanceHandler();
      auto result = to_driver_vfs.AddService<fuchsia_hardware_gpio::Service>(
          std::move(gpio_handler), buttons_names_[i].c_str());
      if (result.is_error()) {
        return result;
      }

      fake_gpio_servers_[i].SetDefaultReadResponse(zx::ok(uint8_t{0u}));
    }

    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
          pdev_.GetInstanceHandler(fdf::Dispatcher::GetCurrent()->async_dispatcher()), "pdev");
      if (result.is_error()) {
        return result.take_error();
      }
    }

    // Serve fake sag.
    if (serve_sag_) {
      return to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_power_system::ActivityGovernor>(
          fake_sag_.CreateHandler());
    }

    return zx::ok();
  }

  void Init(MetadataVersion metadata_version, bool serve_sag) {
    serve_sag_ = serve_sag;

    // Serve metadata.
    std::vector<fuchsia_buttons::GpioButtonConfig> buttons;
    switch (metadata_version) {
      case kMetadataSingleButtonDirect: {
        buttons_names_ = {"volume-up"};
        buttons = kButtonsDirect;
        gpios_ = std::span(kGpiosDirect);
      } break;
      case kMetadataMultiple: {
        buttons_names_ = {"volume-up", "mic-mute", "cam-mute"};
        buttons = kButtonsMultiple;
        gpios_ = std::span(kGpiosMultiple);
      } break;
      case kMetadataDuplicate: {
        buttons_names_ = {"volume-up", "volume-down", "volume-both"};
        buttons = kButtonsDuplicate;
        gpios_ = std::span(kGpiosDuplicate);
      } break;
      case kMetadataMatrix: {
        buttons_names_ = {"volume-up", "key-a", "key-m", "play-pause"};
        buttons = kButtonsMatrix;
        gpios_ = std::span(kGpiosMatrix);
      } break;
      case kMetadataPolled: {
        buttons_names_ = {"volume-up", "mic-mute", "cam-mute"};
        buttons = kButtonsMultiple;
        gpios_ = std::span(kGpiosMultipleOnePolled);
      } break;
      default:
        ASSERT_TRUE(0);
    }

    std::vector<fuchsia_buttons::GpioConfig> gpios(gpios_.begin(), gpios_.end());
    const fuchsia_buttons::GpioButtonsMetadata metadata{
        {.buttons = buttons, .gpios = std::move(gpios)}};
    ASSERT_OK(
        pdev_.AddFidlMetadata(fuchsia_buttons::GpioButtonsMetadata::kSerializableName, metadata));
  }

  void SetGpioReadResponse(size_t gpio_index, uint8_t read_data) {
    fake_gpio_servers_[gpio_index].PushReadResponse(zx::ok(read_data));
  }

  void SetDefaultGpioReadResponse(size_t gpio_index, uint8_t read_data) {
    fake_gpio_servers_[gpio_index].SetDefaultReadResponse(zx::ok(read_data));
  }

  void SetExpectedInterruptOptions(fuchsia_hardware_gpio::InterruptOptions options) {
    fake_gpio_servers_[0].SetExpectedInterruptOptions(options);
  }

  void SetExpectedInterruptMode(fuchsia_hardware_gpio::InterruptMode mode) {
    fake_gpio_servers_[0].SetExpectedInterruptMode(mode);
  }

  zx::interrupt fake_gpio_interrupts_[kMaxGpioServers];
  LocalFakeGpio fake_gpio_servers_[kMaxGpioServers]{};
  FakeSystemActivityGovernor fake_sag_;

 private:
  std::vector<std::string> buttons_names_;
  // Must be of static lifetime.
  cpp20::span<const fuchsia_buttons::GpioConfig> gpios_;
  fdf_fake::FakePDev pdev_;
  bool serve_sag_;
};

class ButtonsTestConfig final {
 public:
  using DriverType = buttons::Buttons;
  using EnvironmentType = ButtonsTestEnvironment;
};

class ButtonsTest : public ::testing::Test {
 public:
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  zx::result<> Init(MetadataVersion metadata_version, bool suspend_enabled, bool serve_sag = true) {
    driver_test().RunInEnvironmentTypeContext(
        [metadata_version, serve_sag](ButtonsTestEnvironment& env) {
          env.Init(metadata_version, serve_sag);
        });

    return driver_test().StartDriverWithCustomStartArgs([&](fdf::DriverStartArgs& start_args) {
      buttons_config::Config fake_config;
      fake_config.suspend_enabled() = suspend_enabled;
      start_args.config(fake_config.ToVmo());
    });
  }

  fidl::ClientEnd<fuchsia_input_report::InputDevice> GetClient() {
    // Connect to InputDevice.
    zx::result connect_result =
        driver_test().ConnectThroughDevfs<fuchsia_input_report::InputDevice>("buttons");
    EXPECT_EQ(ZX_OK, connect_result.status_value());
    return std::move(connect_result.value());
  }

  fdf_testing::BackgroundDriverTest<ButtonsTestConfig>& driver_test() { return driver_test_; }

  class SyncReaderV2EventHandler
      : public fidl::WireSyncEventHandler<fuchsia_input_report::InputReportsReaderV2> {
   public:
    using Callback = fit::function<void(
        fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*)>;

    explicit SyncReaderV2EventHandler(Callback callback = nullptr)
        : callback_(std::move(callback)) {}

    void OnInputReports(fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*
                            event) override {
      last_report_stamp = event->last_report_stamp;
      if (callback_) {
        callback_(event);
      }
    }

    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_input_report::InputReportsReaderV2> metadata) override {}

    uint64_t last_report_stamp = 0;

   private:
    Callback callback_;
  };

  zx_status_t ReadAndAcknowledgeOneEvent(
      fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2>& reader,
      fit::function<
          void(fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*)>
          callback) {
    SyncReaderV2EventHandler handler(std::move(callback));
    fidl::Status result = reader.HandleOneEvent(handler);
    if (!result.ok()) {
      return result.status();
    }
    return reader->AcknowledgeReports(handler.last_report_stamp).status();
  }

  void DrainInitialReport(
      fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2>& reader) {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;
          ASSERT_EQ(1U, reports.size());
          const fuchsia_input_report::wire::InputReport& report = reports[0];

          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_consumer_control());
          const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
              report.consumer_control();

          ASSERT_TRUE(consumer_control.has_pressed_buttons());
        });
    ASSERT_OK(status);
  }

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> GetReader() {
    fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2> endpoints =
        fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
    fidl::ClientEnd<fuchsia_input_report::InputDevice> client = GetClient();
    fidl::WireResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2> result =
        fidl::WireCall(client)->GetInputReportsReaderV2(std::move(endpoints.server), 10);
    ZX_ASSERT(result.ok());
    fidl::WireResult<fuchsia_input_report::InputDevice::GetDescriptor> descriptor_result =
        fidl::WireCall(client)->GetDescriptor();
    ZX_ASSERT(descriptor_result.ok());
    fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader(
        std::move(endpoints.client));
    ZX_ASSERT(reader.is_valid());

    DrainInitialReport(reader);

    return reader;
  }

 private:
  fdf_testing::BackgroundDriverTest<ButtonsTestConfig> driver_test_;
};

class ParameterizedButtonsTest : public ButtonsTest, public ::testing::WithParamInterface<bool> {};

INSTANTIATE_TEST_SUITE_P(ParameterizedButtonsTest, ParameterizedButtonsTest,
                         ::testing::Values(true, false));

TEST_P(ParameterizedButtonsTest, DirectButtonInit) {
  auto result = Init(kMetadataSingleButtonDirect, /* suspend_enabled */ GetParam());
  ASSERT_TRUE(result.is_ok());
}

TEST_P(ParameterizedButtonsTest, DirectButtonPush) {
  auto result = Init(kMetadataSingleButtonDirect, /* suspend_enabled */ GetParam());
  ASSERT_TRUE(result.is_ok());

  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.fake_gpio_interrupts_[0].trigger(0, zx::clock::get_boot());
  });
}

TEST_P(ParameterizedButtonsTest, DirectButtonPushReleaseReport) {
  bool suspend_enabled = GetParam();
  auto result = Init(kMetadataSingleButtonDirect, suspend_enabled);
  ASSERT_TRUE(result.is_ok());

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader = GetReader();

  // Push.
  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.SetDefaultGpioReadResponse(0, 1);

    env.fake_gpio_interrupts_[0].trigger(0, zx::clock::get_boot());
  });

  {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

          ASSERT_EQ(reports.size(), 1U);
          const fuchsia_input_report::wire::InputReport& report = reports[0];

          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_consumer_control());
          const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
              report.consumer_control();

          ASSERT_TRUE(consumer_control.has_pressed_buttons());
          ASSERT_EQ(consumer_control.pressed_buttons().size(), 1U);
          EXPECT_EQ(consumer_control.pressed_buttons()[0],
                    fuchsia_input_report::wire::ConsumerControlButton::kVolumeUp);
        });
    ASSERT_OK(status);

    driver_test().RunInEnvironmentTypeContext([suspend_enabled](ButtonsTestEnvironment& env) {
      EXPECT_EQ(env.fake_sag_.HasWakeLeaseBeenTaken(), suspend_enabled);
    });
  }

  // Release.
  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.SetDefaultGpioReadResponse(0, 0);
    env.fake_gpio_interrupts_[0].trigger(0, zx::clock::get_boot());
  });

  {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

          ASSERT_EQ(reports.size(), 1U);
          const fuchsia_input_report::wire::InputReport& report = reports[0];

          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_consumer_control());
          const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
              report.consumer_control();

          ASSERT_TRUE(consumer_control.has_pressed_buttons());
          ASSERT_EQ(consumer_control.pressed_buttons().size(), 0U);
        });
    ASSERT_OK(status);

    driver_test().RunInEnvironmentTypeContext([suspend_enabled](ButtonsTestEnvironment& env) {
      EXPECT_EQ(env.fake_sag_.HasWakeLeaseBeenTaken(), suspend_enabled);
    });
  }
}

TEST_P(ParameterizedButtonsTest, DirectButtonPushReleaseReportWithoutPower) {
  bool suspend_enabled = GetParam();
  auto result = Init(kMetadataSingleButtonDirect, suspend_enabled, /* serve_sag= */ false);
  ASSERT_TRUE(result.is_ok());

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader = GetReader();

  // Push.
  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.SetDefaultGpioReadResponse(0, 1);

    env.fake_gpio_interrupts_[0].trigger(0, zx::clock::get_boot());
  });

  {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

          ASSERT_EQ(reports.size(), 1U);
          const fuchsia_input_report::wire::InputReport& report = reports[0];

          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_consumer_control());
          const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
              report.consumer_control();

          ASSERT_TRUE(consumer_control.has_pressed_buttons());
          ASSERT_EQ(consumer_control.pressed_buttons().size(), 1U);
          EXPECT_EQ(consumer_control.pressed_buttons()[0],
                    fuchsia_input_report::wire::ConsumerControlButton::kVolumeUp);
        });
    ASSERT_OK(status);
  }

  // Release.
  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.SetDefaultGpioReadResponse(0, 0);
    env.fake_gpio_interrupts_[0].trigger(0, zx::clock::get_boot());
  });

  {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

          ASSERT_EQ(reports.size(), 1U);
          const fuchsia_input_report::wire::InputReport& report = reports[0];

          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_consumer_control());
          const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
              report.consumer_control();

          ASSERT_TRUE(consumer_control.has_pressed_buttons());
          ASSERT_EQ(consumer_control.pressed_buttons().size(), 0U);
        });
    ASSERT_OK(status);
  }
}

TEST_P(ParameterizedButtonsTest, DirectButtonPushReleasePush) {
  auto result = Init(kMetadataSingleButtonDirect, /* suspend_enabled */ GetParam());
  ASSERT_TRUE(result.is_ok());

  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.SetGpioReadResponse(0, 0);
    env.fake_gpio_interrupts_[0].trigger(0, zx::clock::get_boot());

    env.SetGpioReadResponse(0, 1);
    env.fake_gpio_interrupts_[0].trigger(0, zx::clock::get_boot());

    env.SetGpioReadResponse(0, 0);
    env.fake_gpio_interrupts_[0].trigger(0, zx::clock::get_boot());
  });
}

TEST_P(ParameterizedButtonsTest, DirectButtonFlaky) {
  auto init_result = Init(kMetadataSingleButtonDirect, /* suspend_enabled */ GetParam());
  ASSERT_TRUE(init_result.is_ok());

  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.SetGpioReadResponse(0, 1);
    env.SetGpioReadResponse(0, 0);
    env.SetDefaultGpioReadResponse(0, 1);  // Stabilizes.

    env.fake_gpio_interrupts_[0].trigger(0, zx::clock::get_boot());
  });

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader = GetReader();
  zx_status_t status = ReadAndAcknowledgeOneEvent(
      reader,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1U);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_consumer_control());
        const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
            report.consumer_control();

        ASSERT_TRUE(consumer_control.has_pressed_buttons());
        ASSERT_EQ(consumer_control.pressed_buttons().size(), 1U);
        EXPECT_EQ(consumer_control.pressed_buttons()[0],
                  fuchsia_input_report::wire::ConsumerControlButton::kVolumeUp);
      });
  ASSERT_OK(status);
}

TEST_P(ParameterizedButtonsTest, MatrixButtonInit) {
  auto result = Init(kMetadataMatrix, /* suspend_enabled */ GetParam());
  ASSERT_TRUE(result.is_ok());
}

TEST_P(ParameterizedButtonsTest, MatrixButtonPush) {
  auto init_result = Init(kMetadataMatrix, /* suspend_enabled */ GetParam());
  ASSERT_TRUE(init_result.is_ok());

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader = GetReader();

  // Initial reads.
  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.SetGpioReadResponse(0, 0);
    env.SetGpioReadResponse(0, 0);
    env.SetGpioReadResponse(1, 0);
    env.SetGpioReadResponse(1, 0);

    env.SetGpioReadResponse(0, 1);  // Read row. Matrix Scan for 0.
    env.SetGpioReadResponse(0, 0);  // Read row. Matrix Scan for 2.
    env.SetGpioReadResponse(1, 0);  // Read row. Matrix Scan for 1.
    env.SetGpioReadResponse(1, 0);  // Read row. Matrix Scan for 3.

    env.fake_gpio_interrupts_[0].trigger(0, zx::clock::get_boot());
  });

  zx_status_t status = ReadAndAcknowledgeOneEvent(
      reader,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1U);
        const fuchsia_input_report::wire::InputReport& report = reports[0];

        ASSERT_TRUE(report.has_event_time());
        ASSERT_TRUE(report.has_consumer_control());
        const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
            report.consumer_control();

        ASSERT_TRUE(consumer_control.has_pressed_buttons());
        ASSERT_EQ(consumer_control.pressed_buttons().size(), 1U);
        EXPECT_EQ(consumer_control.pressed_buttons()[0],
                  fuchsia_input_report::wire::ConsumerControlButton::kVolumeUp);
      });
  ASSERT_OK(status);

  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    auto gpio_2_states = env.fake_gpio_servers_[2].GetStateLog();
    const fuchsia_buttons::GpioConfig& gpio_2 = kGpiosMatrix[2];
    ASSERT_TRUE(gpio_2.type().has_value());
    ASSERT_TRUE(gpio_2.type()->matrix_output().has_value());
    const fuchsia_buttons::MatrixOutputGpio& gpio_2_matrix = gpio_2.type()->matrix_output().value();
    ASSERT_TRUE(gpio_2_matrix.output_value().has_value());
    const uint8_t gpio_2_matrix_output_value = gpio_2_matrix.output_value().value();
    ASSERT_GE(gpio_2_states.size(), 4U);
    ASSERT_EQ(fake_gpio::ReadSubState{},
              (gpio_2_states.end() - 4)->sub_state);  // Float column.
    ASSERT_EQ(fake_gpio::WriteSubState{.value = gpio_2_matrix_output_value},
              (gpio_2_states.end() - 3)->sub_state);  // Restore column.
    ASSERT_EQ(fake_gpio::ReadSubState{},
              (gpio_2_states.end() - 2)->sub_state);  // Float column.
    ASSERT_EQ(fake_gpio::WriteSubState{.value = gpio_2_matrix_output_value},
              (gpio_2_states.end() - 1)->sub_state);  // Restore column.

    auto gpio_3_states = env.fake_gpio_servers_[3].GetStateLog();
    const fuchsia_buttons::GpioConfig& gpio_3 = kGpiosMatrix[2];
    ASSERT_TRUE(gpio_3.type().has_value());
    ASSERT_TRUE(gpio_3.type()->matrix_output().has_value());
    const fuchsia_buttons::MatrixOutputGpio& gpio_3_matrix = gpio_3.type()->matrix_output().value();
    ASSERT_TRUE(gpio_3_matrix.output_value().has_value());
    const uint8_t gpio_3_matrix_output_value = gpio_3_matrix.output_value().value();
    ASSERT_GE(gpio_3_states.size(), 4U);
    ASSERT_EQ(fake_gpio::ReadSubState{},
              (gpio_3_states.end() - 4)->sub_state);  // Float column.
    ASSERT_EQ(fake_gpio::WriteSubState{.value = gpio_3_matrix_output_value},
              (gpio_3_states.end() - 3)->sub_state);  // Restore column.
    ASSERT_EQ(fake_gpio::ReadSubState{},
              (gpio_3_states.end() - 2)->sub_state);  // Float column.
    ASSERT_EQ(fake_gpio::WriteSubState{.value = gpio_3_matrix_output_value},
              (gpio_3_states.end() - 1)->sub_state);  // Restore column.
  });
}

TEST_P(ParameterizedButtonsTest, DuplicateReports) {
  auto result = Init(kMetadataDuplicate, /* suspend_enabled */ GetParam());
  ASSERT_TRUE(result.is_ok());

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader = GetReader();

  // Holding FDR (VOL_UP and VOL_DOWN), then release VOL_UP, should only get one report
  // for the FDR and one report for the VOL_UP. When FDR is released, there is no
  // new report generated since the reported values do not change.

  // Push FDR (both VOL_UP and VOL_DOWN).
  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.SetGpioReadResponse(2, 1);
    env.SetGpioReadResponse(2, 1);

    // Report.
    env.SetGpioReadResponse(0, 1);
    env.SetGpioReadResponse(1, 1);
    env.SetGpioReadResponse(2, 1);

    // Release VOL_UP.
    env.SetGpioReadResponse(0, 0);
    env.SetGpioReadResponse(0, 0);

    // Report.
    env.SetGpioReadResponse(0, 0);
    env.SetGpioReadResponse(1, 1);
    env.SetGpioReadResponse(2, 0);

    // Release FDR (both VOL_UP and VOL_DOWN).
    env.SetGpioReadResponse(2, 0);
    env.SetGpioReadResponse(2, 0);

    // Report (same as before).
    env.SetGpioReadResponse(0, 0);
    env.SetGpioReadResponse(1, 1);
    env.SetGpioReadResponse(2, 0);

    env.fake_gpio_interrupts_[2].trigger(0, zx::clock::get_boot());
  });

  {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

          ASSERT_EQ(reports.size(), 1U);
          const fuchsia_input_report::wire::InputReport& report = reports[0];

          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_consumer_control());
          const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
              report.consumer_control();

          ASSERT_TRUE(consumer_control.has_pressed_buttons());
          ASSERT_EQ(consumer_control.pressed_buttons().size(), 3U);
          std::set<fuchsia_input_report::wire::ConsumerControlButton> pressed_buttons;
          for (const fuchsia_input_report::wire::ConsumerControlButton& button :
               consumer_control.pressed_buttons()) {
            pressed_buttons.insert(button);
          }
          const std::set<fuchsia_input_report::wire::ConsumerControlButton> expected_buttons = {
              fuchsia_input_report::wire::ConsumerControlButton::kVolumeUp,
              fuchsia_input_report::wire::ConsumerControlButton::kVolumeDown,
              fuchsia_input_report::wire::ConsumerControlButton::kFactoryReset};
          EXPECT_EQ(expected_buttons, pressed_buttons);
        });
    ASSERT_OK(status);
  }

  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.fake_gpio_interrupts_[0].trigger(0, zx::clock::get_boot());
  });

  {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

          ASSERT_EQ(reports.size(), 1U);
          const fuchsia_input_report::wire::InputReport& report = reports[0];

          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_consumer_control());
          const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
              report.consumer_control();

          ASSERT_TRUE(consumer_control.has_pressed_buttons());
          ASSERT_EQ(consumer_control.pressed_buttons().size(), 1U);
          EXPECT_EQ(consumer_control.pressed_buttons()[0],
                    fuchsia_input_report::wire::ConsumerControlButton::kVolumeDown);
        });
    ASSERT_OK(status);
  }

  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.fake_gpio_interrupts_[2].trigger(0, zx::clock::get_boot());
  });
}

TEST_P(ParameterizedButtonsTest, CamMute) {
  auto result = Init(kMetadataMultiple, /* suspend_enabled */ GetParam());
  ASSERT_TRUE(result.is_ok());

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader = GetReader();

  // Push camera mute.
  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.SetGpioReadResponse(2, 1);
    env.SetGpioReadResponse(2, 1);

    // Report.
    env.SetGpioReadResponse(0, 0);
    env.SetGpioReadResponse(1, 0);
    env.SetGpioReadResponse(2, 1);

    env.fake_gpio_interrupts_[2].trigger(0, zx::clock::get_boot());
  });

  {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

          ASSERT_EQ(reports.size(), 1U);
          const fuchsia_input_report::wire::InputReport& report = reports[0];

          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_consumer_control());
          const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
              report.consumer_control();

          ASSERT_TRUE(consumer_control.has_pressed_buttons());
          ASSERT_EQ(consumer_control.pressed_buttons().size(), 1U);
          EXPECT_EQ(consumer_control.pressed_buttons()[0],
                    fuchsia_input_report::wire::ConsumerControlButton::kCameraDisable);
        });
    ASSERT_OK(status);
  }

  // Release camera mute.
  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.SetGpioReadResponse(2, 0);
    env.SetGpioReadResponse(2, 0);

    // Report.
    env.SetGpioReadResponse(0, 0);
    env.SetGpioReadResponse(1, 0);
    env.SetGpioReadResponse(2, 0);

    env.fake_gpio_interrupts_[2].trigger(0, zx::clock::get_boot());
  });

  {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

          ASSERT_EQ(reports.size(), 1U);
          const fuchsia_input_report::wire::InputReport& report = reports[0];

          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_consumer_control());
          const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
              report.consumer_control();

          ASSERT_TRUE(consumer_control.has_pressed_buttons());
          ASSERT_EQ(consumer_control.pressed_buttons().size(), 0U);
        });
    ASSERT_OK(status);
  }
}

TEST_P(ParameterizedButtonsTest, PollOneButton) {
  auto result = Init(kMetadataPolled, /* suspend_enabled */ GetParam());
  ASSERT_TRUE(result.is_ok());

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader = GetReader();

  // All gpios_ must have a default read value if polling is being used, as they are all ready
  // every poll period.
  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.SetDefaultGpioReadResponse(0, 1);

    env.fake_gpio_interrupts_[0].trigger(0, zx::clock::get_boot());
  });
  {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

          ASSERT_EQ(reports.size(), 1U);
          const fuchsia_input_report::wire::InputReport& report = reports[0];

          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_consumer_control());
          const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
              report.consumer_control();

          ASSERT_TRUE(consumer_control.has_pressed_buttons());
          ASSERT_EQ(consumer_control.pressed_buttons().size(), 1U);
          EXPECT_EQ(consumer_control.pressed_buttons()[0],
                    fuchsia_input_report::wire::ConsumerControlButton::kVolumeUp);
        });
    ASSERT_OK(status);
  }

  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.SetDefaultGpioReadResponse(1, 1);

    env.fake_gpio_interrupts_[0].trigger(0, zx::clock::get_boot());
    env.fake_gpio_interrupts_[1].trigger(0, zx::clock::get_boot());
  });

  {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

          ASSERT_EQ(reports.size(), 1U);
          const fuchsia_input_report::wire::InputReport& report = reports[0];

          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_consumer_control());
          const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
              report.consumer_control();

          ASSERT_TRUE(consumer_control.has_pressed_buttons());
          ASSERT_EQ(consumer_control.pressed_buttons().size(), 2U);
          std::set<fuchsia_input_report::wire::ConsumerControlButton> pressed_buttons;
          for (const fuchsia_input_report::wire::ConsumerControlButton& button :
               consumer_control.pressed_buttons()) {
            pressed_buttons.insert(button);
          }
          const std::set<fuchsia_input_report::wire::ConsumerControlButton> expected_buttons = {
              fuchsia_input_report::wire::ConsumerControlButton::kVolumeUp,
              fuchsia_input_report::wire::ConsumerControlButton::kMicMute};
          EXPECT_EQ(expected_buttons, pressed_buttons);
        });
    ASSERT_OK(status);
  }

  driver_test().RunInEnvironmentTypeContext([](ButtonsTestEnvironment& env) {
    env.SetDefaultGpioReadResponse(0, 0);
    env.fake_gpio_interrupts_[1].trigger(0, zx::clock::get_boot());
  });

  {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

          ASSERT_EQ(reports.size(), 1U);
          const fuchsia_input_report::wire::InputReport& report = reports[0];

          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_consumer_control());
          const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
              report.consumer_control();

          ASSERT_TRUE(consumer_control.has_pressed_buttons());
          ASSERT_EQ(consumer_control.pressed_buttons().size(), 1U);
          EXPECT_EQ(consumer_control.pressed_buttons()[0],
                    fuchsia_input_report::wire::ConsumerControlButton::kMicMute);
        });
    ASSERT_OK(status);
  }

  driver_test().RunInEnvironmentTypeContext(
      [](ButtonsTestEnvironment& env) { env.SetDefaultGpioReadResponse(1, 0); });

  {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

          ASSERT_EQ(reports.size(), 1U);
          const fuchsia_input_report::wire::InputReport& report = reports[0];

          ASSERT_TRUE(report.has_event_time());
          ASSERT_TRUE(report.has_consumer_control());
          const fuchsia_input_report::wire::ConsumerControlInputReport& consumer_control =
              report.consumer_control();

          ASSERT_TRUE(consumer_control.has_pressed_buttons());
          ASSERT_EQ(consumer_control.pressed_buttons().size(), 0U);
        });
    ASSERT_OK(status);
  }
}

}  // namespace buttons
