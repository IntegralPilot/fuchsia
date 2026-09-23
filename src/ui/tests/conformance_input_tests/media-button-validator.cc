// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.input.report/cpp/fidl.h>
#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <fidl/fuchsia.ui.focus/cpp/fidl.h>
#include <fidl/fuchsia.ui.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.input/cpp/natural_ostream.h>
#include <fidl/fuchsia.ui.input3/cpp/fidl.h>
#include <fidl/fuchsia.ui.policy/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.conformance/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.scene/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>

#include <sstream>
#include <string>
#include <utility>
#include <vector>

#include <src/ui/testing/util/fidl_cpp_helpers.h>
#include <src/ui/tests/conformance_input_tests/conformance-test-base.h>
#include <zxtest/zxtest.h>

namespace ui_conformance_testing {

const std::string PUPPET_UNDER_TEST_FACTORY_SERVICE = "/svc/puppet-under-test-factory-service";

namespace futi = fuchsia_ui_test_input;
namespace fir = fuchsia_input_report;
namespace fui = fuchsia_ui_input;
namespace fup = fuchsia_ui_policy;
namespace futc = fuchsia_ui_test_conformance;

class ButtonsListener final : public fidl::Server<fup::MediaButtonsListener> {
 public:
  // |MediaButtonsListener|
  void OnEvent(OnEventRequest& request, OnEventCompleter::Sync& completer) override {
    events_received_.push_back(std::move(request.event()));
    completer.Reply();
  }

  void OnMediaButtonsEvent(OnMediaButtonsEventRequest& request,
                           OnMediaButtonsEventCompleter::Sync& completer) override {
    ZX_PANIC("Not Implemented");
  }

  fidl::ClientEnd<fup::MediaButtonsListener> ServeAndGetClientEnd(async_dispatcher_t* dispatcher) {
    auto [client_end, server_end] = fidl::Endpoints<fup::MediaButtonsListener>::Create();
    binding_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
    return std::move(client_end);
  }

  const std::vector<fui::MediaButtonsEvent>& events_received() const { return events_received_; }

  void clear_events() { events_received_.clear(); }

 private:
  fidl::ServerBindingGroup<fup::MediaButtonsListener> binding_;
  std::vector<fui::MediaButtonsEvent> events_received_;
};

class MediaButtonConformanceTest : public ui_conformance_test_base::ConformanceTest {
 public:
  void SetUp() override {
    ui_conformance_test_base::ConformanceTest::SetUp();

    fuchsia_ui_views::ViewCreationToken root_view_token;
    {
      FX_LOGS(INFO) << "Creating root view token";

      auto controller = ConnectSyncIntoRealm<fuchsia_ui_test_scene::Controller>();

      fuchsia_ui_test_scene::ControllerPresentClientViewRequest req;
      auto [view_token, viewport_token] = scenic::cpp::ViewCreationTokenPair::New();
      req.viewport_creation_token(std::move(viewport_token));
      ZX_ASSERT_OK(controller->PresentClientView(std::move(req)));
      root_view_token = std::move(view_token);
    }

    {
      FX_LOGS(INFO) << "Create puppet under test";
      auto puppet_factory_connect =
          component::Connect<futc::PuppetFactory>(PUPPET_UNDER_TEST_FACTORY_SERVICE);
      ZX_ASSERT_OK(puppet_factory_connect);

      puppet_factory_ = fidl::SyncClient(std::move(puppet_factory_connect.value()));

      auto flatland = ConnectIntoRealm<fuchsia_ui_composition::Flatland>();
      auto keyboard = ConnectIntoRealm<fuchsia_ui_input3::Keyboard>();
      auto [puppet_client_end, puppet_server_end] = fidl::Endpoints<futc::Puppet>::Create();
      puppet_ = fidl::SyncClient(std::move(puppet_client_end));

      futc::PuppetCreationArgs creation_args;
      creation_args.server_end(std::move(puppet_server_end));
      creation_args.view_token(std::move(root_view_token));
      creation_args.flatland_client(std::move(flatland));
      creation_args.keyboard_client(std::move(keyboard));
      creation_args.device_pixel_ratio(DevicePixelRatio());

      auto res = puppet_factory_->Create(std::move(creation_args));
      ZX_ASSERT_OK(res);
      ASSERT_EQ(res.value().result(), futc::Result::kSuccess);
    }

    // Register fake media button.
    {
      FX_LOGS(INFO) << "Connecting to input registry";
      auto input_registry = ConnectSyncIntoRealm<futi::Registry>();

      FX_LOGS(INFO) << "Registering fake media button device";
      auto [client_end, server_end] = fidl::Endpoints<futi::MediaButtonsDevice>::Create();
      media_buttons_ = fidl::SyncClient(std::move(client_end));

      futi::RegistryRegisterMediaButtonsDeviceAndGetDeviceInfoRequest request;
      request.device(std::move(server_end));
      auto res = input_registry->RegisterMediaButtonsDeviceAndGetDeviceInfo(std::move(request));
      ZX_ASSERT_OK(res);

      device_id_ = res->device_id().value();
    }
  }

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
  void SimulatePress(fuchsia_input::ConsumerControlButton button) {
    futi::MediaButtonsDeviceSimulateButtonPressRequest request;
    request.button(button);
    ZX_ASSERT_OK(media_buttons_->SimulateButtonPress(request));
  }

  void SimulatePress(std::vector<fuchsia_input::ConsumerControlButton> buttons) {
    futi::MediaButtonsDeviceSendButtonsStateRequest request;
    request.buttons(std::move(buttons));
    ZX_ASSERT_OK(media_buttons_->SendButtonsState(request));
  }
#else
  void SimulatePress(fuchsia_input::wire::ConsumerControlButton button) {
    futi::MediaButtonsDeviceSimulateButtonPressRequest request;
    request.button(button);
    ZX_ASSERT_OK(media_buttons_->SimulateButtonPress(request));
  }

  void SimulatePress(std::vector<fuchsia_input::wire::ConsumerControlButton> buttons) {
    futi::MediaButtonsDeviceSendButtonsStateRequest request;
    request.buttons(std::move(buttons));
    ZX_ASSERT_OK(media_buttons_->SendButtonsState(request));
  }
#endif

  fui::MediaButtonsEvent MakeMediaButtonsEvent(int8_t volume, bool mic_mute, bool pause,
                                               bool camera_disable, bool power, bool function) {
    fui::MediaButtonsEvent e({.volume = volume,
                              .mic_mute = mic_mute,
                              .pause = pause,
                              .camera_disable = camera_disable,
                              .power = power,
                              .function = function,
                              .device_id = device_id_});
    return e;
  }

  fui::MediaButtonsEvent MakeEmptyEvent() {
    return MakeMediaButtonsEvent(0, false, false, false, false, false);
  }

  fui::MediaButtonsEvent MakeVolumeUpEvent() {
    return MakeMediaButtonsEvent(1, false, false, false, false, false);
  }

  fui::MediaButtonsEvent MakeVolumeDownEvent() {
    return MakeMediaButtonsEvent(-1, false, false, false, false, false);
  }

  fui::MediaButtonsEvent MakePauseEvent() {
    return MakeMediaButtonsEvent(0, false, true, false, false, false);
  }

  fui::MediaButtonsEvent MakeMicMuteEvent() {
    return MakeMediaButtonsEvent(0, true, false, false, false, false);
  }

  fui::MediaButtonsEvent MakeCameraDisableEvent() {
    return MakeMediaButtonsEvent(0, false, false, true, false, false);
  }

  fui::MediaButtonsEvent MakePowerEvent() {
    return MakeMediaButtonsEvent(0, false, false, false, true, false);
  }

  fui::MediaButtonsEvent MakeFunctionEvent() {
    return MakeMediaButtonsEvent(0, false, false, false, false, true);
  }

 private:
  fidl::SyncClient<futi::MediaButtonsDevice> media_buttons_;
  fidl::SyncClient<futc::Puppet> puppet_;
  fidl::SyncClient<futc::PuppetFactory> puppet_factory_;
  uint32_t device_id_;
};

std::string ToString(const fui::MediaButtonsEvent& e) {
  std::stringstream ss;
  // ignore trace_flow_id in comparison
  fui::MediaButtonsEvent e_copy({
      .volume = e.volume().value_or(0),
      .mic_mute = e.mic_mute().value_or(false),
      .pause = e.pause().value_or(false),
      .camera_disable = e.camera_disable().value_or(false),
      .power = e.power().value_or(false),
      .function = e.function().value_or(false),
      .device_id = e.device_id().value_or(0),
  });
  ss << e_copy;
  return ss.str();
}

TEST_F(MediaButtonConformanceTest, SimplePress) {
  auto device_listener_registry = ConnectSyncIntoRealm<fup::DeviceListenerRegistry>();

  ButtonsListener listener;
  auto listener_client_end = listener.ServeAndGetClientEnd(dispatcher());
  ZX_ASSERT_OK(device_listener_registry->RegisterListener({std::move(listener_client_end)}));

  {
    SimulatePress(fuchsia_input::wire::ConsumerControlButton::kVolumeUp);
    FX_LOGS(INFO) << "wait for button VOLUME_UP";
    RunLoopUntil([&listener]() { return listener.events_received().size() > 1; });
    EXPECT_EQ(listener.events_received().size(), 2u);
    EXPECT_EQ(ToString(listener.events_received()[0]), ToString(MakeVolumeUpEvent()));
    EXPECT_EQ(ToString(listener.events_received()[1]), ToString(MakeEmptyEvent()));
    listener.clear_events();
  }

  {
    SimulatePress(fuchsia_input::wire::ConsumerControlButton::kVolumeDown);
    FX_LOGS(INFO) << "wait for button VOLUME_DOWN";
    RunLoopUntil([&listener]() { return listener.events_received().size() > 1; });
    EXPECT_EQ(listener.events_received().size(), 2u);
    EXPECT_EQ(ToString(listener.events_received()[0]), ToString(MakeVolumeDownEvent()));
    EXPECT_EQ(ToString(listener.events_received()[1]), ToString(MakeEmptyEvent()));
    listener.clear_events();
  }

  {
    SimulatePress(fuchsia_input::wire::ConsumerControlButton::kPause);
    FX_LOGS(INFO) << "wait for button PAUSE";
    RunLoopUntil([&listener]() { return listener.events_received().size() > 1; });
    EXPECT_EQ(listener.events_received().size(), 2u);
    EXPECT_EQ(ToString(listener.events_received()[0]), ToString(MakePauseEvent()));
    EXPECT_EQ(ToString(listener.events_received()[1]), ToString(MakeEmptyEvent()));
    listener.clear_events();
  }

  {
    SimulatePress(fuchsia_input::wire::ConsumerControlButton::kMicMute);
    FX_LOGS(INFO) << "wait for button MIC_MUTE";
    RunLoopUntil([&listener]() { return listener.events_received().size() > 1; });
    EXPECT_EQ(listener.events_received().size(), 2u);
    EXPECT_EQ(ToString(listener.events_received()[0]), ToString(MakeMicMuteEvent()));
    EXPECT_EQ(ToString(listener.events_received()[1]), ToString(MakeEmptyEvent()));
    listener.clear_events();
  }

  {
    SimulatePress(fuchsia_input::wire::ConsumerControlButton::kCameraDisable);
    FX_LOGS(INFO) << "wait for button CAMERA_DISABLE";
    RunLoopUntil([&listener]() { return listener.events_received().size() > 1; });
    EXPECT_EQ(listener.events_received().size(), 2u);
    EXPECT_EQ(ToString(listener.events_received()[0]), ToString(MakeCameraDisableEvent()));
    EXPECT_EQ(ToString(listener.events_received()[1]), ToString(MakeEmptyEvent()));
    listener.clear_events();
  }

  {
    SimulatePress(fuchsia_input::wire::ConsumerControlButton::kFunction);
    FX_LOGS(INFO) << "wait for button FUNCTION";
    RunLoopUntil([&listener]() { return listener.events_received().size() > 1; });
    EXPECT_EQ(listener.events_received().size(), 2u);
    EXPECT_EQ(ToString(listener.events_received()[0]), ToString(MakeFunctionEvent()));
    EXPECT_EQ(ToString(listener.events_received()[1]), ToString(MakeEmptyEvent()));
    listener.clear_events();
  }

  {
    SimulatePress(fuchsia_input::wire::ConsumerControlButton::kPower);
    FX_LOGS(INFO) << "wait for button POWER";
    RunLoopUntil([&listener]() { return listener.events_received().size() > 1; });
    EXPECT_EQ(listener.events_received().size(), 2u);
    EXPECT_EQ(ToString(listener.events_received()[0]), ToString(MakePowerEvent()));
    EXPECT_EQ(ToString(listener.events_received()[1]), ToString(MakeEmptyEvent()));
    listener.clear_events();
  }

  // The following button types are not yet supported.
  {
    SimulatePress(fuchsia_input::wire::ConsumerControlButton::kReboot);
    FX_LOGS(INFO) << "wait for button REBOOT";
    RunLoopUntil([&listener]() { return listener.events_received().size() > 1; });
    EXPECT_EQ(listener.events_received().size(), 2u);
    EXPECT_EQ(ToString(listener.events_received()[0]), ToString(MakeEmptyEvent()));
    EXPECT_EQ(ToString(listener.events_received()[1]), ToString(MakeEmptyEvent()));
    listener.clear_events();
  }

  {
    SimulatePress(fuchsia_input::wire::ConsumerControlButton::kFactoryReset);
    FX_LOGS(INFO) << "wait for button FACTORY_RESET";
    RunLoopUntil([&listener]() { return listener.events_received().size() > 1; });
    EXPECT_EQ(listener.events_received().size(), 2u);
    EXPECT_EQ(ToString(listener.events_received()[0]), ToString(MakeEmptyEvent()));
    EXPECT_EQ(ToString(listener.events_received()[1]), ToString(MakeEmptyEvent()));
    listener.clear_events();
  }
}

TEST_F(MediaButtonConformanceTest, MultiPress) {
  auto device_listener_registry = ConnectSyncIntoRealm<fup::DeviceListenerRegistry>();

  ButtonsListener listener;
  auto listener_client_end = listener.ServeAndGetClientEnd(dispatcher());
  ZX_ASSERT_OK(device_listener_registry->RegisterListener({std::move(listener_client_end)}));

  // press multi buttons.
  {
    auto buttons = std::vector<fuchsia_input::wire::ConsumerControlButton>{
        fuchsia_input::ConsumerControlButton::kCameraDisable,
        fuchsia_input::ConsumerControlButton::kMicMute,
        fuchsia_input::ConsumerControlButton::kPause,
        fuchsia_input::ConsumerControlButton::kVolumeUp,
    };
    SimulatePress(std::move(buttons));
    RunLoopUntil([&listener]() { return listener.events_received().size() > 0; });
    EXPECT_EQ(listener.events_received().size(), 1u);
    EXPECT_EQ(ToString(listener.events_received()[0]),
              ToString(MakeMediaButtonsEvent(1, true, true, true, false, false)));
    listener.clear_events();
  }

  // release multi buttons.
  {
    SimulatePress(std::vector<fuchsia_input::wire::ConsumerControlButton>());
    RunLoopUntil([&listener]() { return listener.events_received().size() > 0; });
    EXPECT_EQ(listener.events_received().size(), 1u);
    EXPECT_EQ(ToString(listener.events_received()[0]), ToString(MakeEmptyEvent()));
    listener.clear_events();
  }
}

TEST_F(MediaButtonConformanceTest, MultiListener) {
  auto device_listener_registry = ConnectSyncIntoRealm<fup::DeviceListenerRegistry>();

  ButtonsListener listener1;
  auto listener_client_end1 = listener1.ServeAndGetClientEnd(dispatcher());
  ZX_ASSERT_OK(device_listener_registry->RegisterListener({std::move(listener_client_end1)}));

  auto listener2 = std::make_unique<ButtonsListener>();
  auto listener_client_end2 = listener2->ServeAndGetClientEnd(dispatcher());
  ZX_ASSERT_OK(device_listener_registry->RegisterListener({std::move(listener_client_end2)}));

  // Both listener received events.
  {
    SimulatePress(fuchsia_input::wire::ConsumerControlButton::kVolumeUp);
    FX_LOGS(INFO) << "wait for button VOLUME_UP";
    RunLoopUntil([&listener1]() { return listener1.events_received().size() > 1; });
    RunLoopUntil([&listener2]() { return listener2->events_received().size() > 1; });
    EXPECT_EQ(listener1.events_received().size(), 2u);
    EXPECT_EQ(listener2->events_received().size(), 2u);
    EXPECT_EQ(ToString(listener1.events_received()[0]), ToString(MakeVolumeUpEvent()));
    EXPECT_EQ(ToString(listener1.events_received()[1]), ToString(MakeEmptyEvent()));
    EXPECT_EQ(ToString(listener2->events_received()[0]), ToString(MakeVolumeUpEvent()));
    EXPECT_EQ(ToString(listener2->events_received()[1]), ToString(MakeEmptyEvent()));
    listener1.clear_events();
    listener2->clear_events();
  }

  ButtonsListener listener3;
  auto listener_client_end3 = listener3.ServeAndGetClientEnd(dispatcher());
  ZX_ASSERT_OK(device_listener_registry->RegisterListener({std::move(listener_client_end3)}));

  // new listener receives the last event.
  {
    FX_LOGS(INFO) << "listener wait for the last button";
    RunLoopUntil([&listener3]() { return listener3.events_received().size() > 0; });
    EXPECT_EQ(ToString(listener3.events_received()[0]), ToString(MakeEmptyEvent()));
    listener3.clear_events();
  }

  // then new listener receive only new events.
  {
    SimulatePress(fuchsia_input::wire::ConsumerControlButton::kVolumeDown);
    FX_LOGS(INFO) << "wait for button VOLUME_DOWN";
    RunLoopUntil([&listener1]() { return listener1.events_received().size() > 1; });
    RunLoopUntil([&listener2]() { return listener2->events_received().size() > 1; });
    RunLoopUntil([&listener3]() { return listener3.events_received().size() > 1; });
    EXPECT_EQ(listener1.events_received().size(), 2u);
    EXPECT_EQ(listener2->events_received().size(), 2u);
    EXPECT_EQ(listener3.events_received().size(), 2u);
    EXPECT_EQ(ToString(listener1.events_received()[0]), ToString(MakeVolumeDownEvent()));
    EXPECT_EQ(ToString(listener1.events_received()[1]), ToString(MakeEmptyEvent()));
    EXPECT_EQ(ToString(listener2->events_received()[0]), ToString(MakeVolumeDownEvent()));
    EXPECT_EQ(ToString(listener2->events_received()[1]), ToString(MakeEmptyEvent()));
    EXPECT_EQ(ToString(listener3.events_received()[0]), ToString(MakeVolumeDownEvent()));
    EXPECT_EQ(ToString(listener3.events_received()[1]), ToString(MakeEmptyEvent()));
    listener1.clear_events();
    listener2->clear_events();
    listener3.clear_events();
  }

  // drop listener2, and verify other listeners still working.
  listener2 = {};
  {
    SimulatePress(fuchsia_input::wire::ConsumerControlButton::kPause);
    FX_LOGS(INFO) << "wait for button PAUSE";
    RunLoopUntil([&listener1]() { return listener1.events_received().size() > 1; });
    RunLoopUntil([&listener3]() { return listener3.events_received().size() > 1; });
    EXPECT_EQ(listener1.events_received().size(), 2u);
    EXPECT_EQ(listener3.events_received().size(), 2u);
    EXPECT_EQ(ToString(listener1.events_received()[0]), ToString(MakePauseEvent()));
    EXPECT_EQ(ToString(listener1.events_received()[1]), ToString(MakeEmptyEvent()));
    EXPECT_EQ(ToString(listener3.events_received()[0]), ToString(MakePauseEvent()));
    EXPECT_EQ(ToString(listener3.events_received()[1]), ToString(MakeEmptyEvent()));
    listener1.clear_events();
    listener3.clear_events();
  }
}

}  //  namespace ui_conformance_testing
