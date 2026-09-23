// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <string>
#include <utility>

#include <src/ui/testing/util/fidl_cpp_helpers.h>
#include <src/ui/tests/integration_input_tests/web-test-base/web-app-base.h>

namespace {

std::vector<fuchsia_ui_test_input::MouseButton> GetPressedButtons(int buttons) {
  std::vector<fuchsia_ui_test_input::MouseButton> pressed_buttons;

  if (buttons & 0x1) {
    pressed_buttons.push_back(fuchsia_ui_test_input::MouseButton::kFirst);
  }
  if (buttons & 0x1 >> 1) {
    pressed_buttons.push_back(fuchsia_ui_test_input::MouseButton::kSecond);
  }
  if (buttons & 0x1 >> 2) {
    pressed_buttons.push_back(fuchsia_ui_test_input::MouseButton::kThird);
  }

  return pressed_buttons;
}

fuchsia_ui_test_input::MouseEventPhase GetPhase(const std::string& type) {
  if (type == "add") {
    return fuchsia_ui_test_input::MouseEventPhase::kAdd;
  }
  if (type == "hover") {
    return fuchsia_ui_test_input::MouseEventPhase::kHover;
  }
  if (type == "mousedown") {
    return fuchsia_ui_test_input::MouseEventPhase::kDown;
  }
  if (type == "mousemove") {
    return fuchsia_ui_test_input::MouseEventPhase::kMove;
  }
  if (type == "mouseup") {
    return fuchsia_ui_test_input::MouseEventPhase::kUp;
  }
  if (type == "wheel") {
    return fuchsia_ui_test_input::MouseEventPhase::kWheel;
  }
  FX_LOGS(FATAL) << "Invalid mouse event type: " << type;
  exit(1);
}

// Implements a simple web app, which responds to mouse events.
class WebApp : public integration_tests::WebAppBase {
 public:
  WebApp() { Setup("mouse-input-chromium", kAppCode); }

  void RunLoopForMouseReponse() {
    auto test_app_status_listener_connect =
        component::Connect<fuchsia_ui_test_input::TestAppStatusListener>();
    ZX_ASSERT_OK(test_app_status_listener_connect);
    fidl::SyncClient test_app_status_listener(std::move(test_app_status_listener_connect.value()));
    ZX_ASSERT_OK(test_app_status_listener->ReportStatus(
        {fuchsia_ui_test_input::TestAppStatus::kHandlersRegistered}));

    auto mouse_input_listener_connect =
        component::Connect<fuchsia_ui_test_input::MouseInputListener>();
    ZX_ASSERT_OK(mouse_input_listener_connect);
    fidl::SyncClient mouse_input_listener(std::move(mouse_input_listener_connect.value()));

    while (true) {
      bool got_mouse_event = false;
      FX_LOGS(INFO) << "Waiting for mouse response message";

      out_message_port_->ReceiveMessage().Then(
          [&mouse_input_listener, &got_mouse_event](auto& res) {
            ZX_ASSERT_OK(res);

            rapidjson::Document mouse_resp =
                integration_tests::JsonFromBuffer(res->message().data().value());
            // Validate structure of mouse response.
            FX_CHECK(mouse_resp.HasMember("type"));
            FX_CHECK(mouse_resp.HasMember("epoch_msec"));
            FX_CHECK(mouse_resp.HasMember("x"));
            FX_CHECK(mouse_resp.HasMember("y"));
            FX_CHECK(mouse_resp.HasMember("device_scale_factor"));
            FX_CHECK(mouse_resp.HasMember("buttons"));
            FX_CHECK(mouse_resp["type"].IsString());
            FX_CHECK(mouse_resp["epoch_msec"].IsInt64());
            FX_CHECK(mouse_resp["x"].IsNumber());
            FX_CHECK(mouse_resp["y"].IsNumber());
            FX_CHECK(mouse_resp["device_scale_factor"].IsNumber());
            FX_CHECK(mouse_resp["buttons"].IsNumber());

            // Relay response to parent.
            fuchsia_ui_test_input::MouseInputListenerReportMouseInputRequest request({
                .local_x = mouse_resp["x"].GetDouble(),
                .local_y = mouse_resp["y"].GetDouble(),
                .time_received = mouse_resp["epoch_msec"].GetInt64() * 1000 * 1000,
                .component_name = "mouse-input-chromium",
                .buttons = GetPressedButtons(mouse_resp["buttons"].GetInt()),
                .phase = GetPhase(mouse_resp["type"].GetString()),
                .device_pixel_ratio = mouse_resp["device_scale_factor"].GetDouble(),
            });

            if (mouse_resp.HasMember("wheel_h")) {
              FX_CHECK(mouse_resp["wheel_h"].IsNumber());
              request.wheel_x_physical_pixel() = mouse_resp["wheel_h"].GetDouble();
            }
            if (mouse_resp.HasMember("wheel_v")) {
              FX_CHECK(mouse_resp["wheel_v"].IsNumber());
              request.wheel_y_physical_pixel() = mouse_resp["wheel_v"].GetDouble();
            }

            FX_LOGS(INFO) << "Got mouse response message " << mouse_resp["type"].GetString();

            ZX_ASSERT_OK(mouse_input_listener->ReportMouseInput(request));

            got_mouse_event = true;
          });
      RunLoopUntil([&]() { return got_mouse_event; });
    }
  }

 private:
  static constexpr auto kAppCode = R"JS(
    let port;
    document.body.onmousemove = function(event) {
      console.assert(port != null);
      let response = JSON.stringify({
        type: event.type,
        epoch_msec: Date.now(),
        x: event.clientX,
        y: event.clientY,
        wheel_h: event.deltaX,
        wheel_v: event.deltaY,
        device_scale_factor: window.devicePixelRatio,
        buttons: event.buttons
      });
      console.info('Reporting mouse move event ', response);
      port.postMessage(response);
    };
    document.body.onmousedown = function(event) {
      console.assert(port != null);
      let response = JSON.stringify({
        type: event.type,
        epoch_msec: Date.now(),
        x: event.clientX,
        y: event.clientY,
        wheel_h: event.deltaX,
        wheel_v: event.deltaY,
        device_scale_factor: window.devicePixelRatio,
        buttons: event.buttons
      });
      console.info('Reporting mouse down event ', response);
      port.postMessage(response);
    };
    document.body.onmouseup = function(event) {
      console.assert(port != null);
      let response = JSON.stringify({
        type: event.type,
        epoch_msec: Date.now(),
        x: event.clientX,
        y: event.clientY,
        wheel_h: event.deltaX,
        wheel_v: event.deltaY,
        device_scale_factor: window.devicePixelRatio,
        buttons: event.buttons
      });
      console.info('Reporting mouse up event ', response);
      port.postMessage(response);
    };
    document.body.onwheel = function(event) {
      console.assert(port != null);
      let response = JSON.stringify({
        type: event.type,
        epoch_msec: Date.now(),
        x: event.clientX,
        y: event.clientY,
        wheel_h: event.deltaX,
        wheel_v: event.deltaY,
        device_scale_factor: window.devicePixelRatio,
        buttons: event.buttons
      });
      console.info('Reporting mouse wheel event ', response);
      port.postMessage(response);
    };
    window.onresize = function(event) {
      if (window.innerWidth != 0) {
        console.info('size: ', window.innerWidth, window.innerHeight);
        document.title = [document.title, 'window_resized'].join(' ');
      }
    };
    function receiveMessage(event) {
      if (event.data == "REGISTER_PORT") {
        console.log("received REGISTER_PORT");
        port = event.ports[0];
        if (window.innerWidth != 0 && window.innerHeight != 0) {
          console.info('size when REGISTER_PORT: ', window.innerWidth, window.innerHeight);
          port.postMessage('PORT_REGISTERED WINDOW_RESIZED');
        } else {
          port.postMessage('PORT_REGISTERED');
        }
      } else {
        console.error('received unexpected message: ' + event.data);
      }
    };
    window.addEventListener('message', receiveMessage, false);
    console.info('JS loaded');
  )JS";
};
}  // namespace

int main(int argc, const char** argv) { WebApp().RunLoopForMouseReponse(); }
