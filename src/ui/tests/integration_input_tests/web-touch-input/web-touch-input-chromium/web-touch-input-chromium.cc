// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/stdcompat/string_view.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <string>
#include <utility>

#include <src/ui/testing/util/fidl_cpp_helpers.h>
#include <src/ui/tests/integration_input_tests/web-test-base/web-app-base.h>

namespace {

// Implements a simple web app, which responds to touch events.
class WebApp : public integration_tests::WebAppBase {
 public:
  WebApp() { Setup("web-touch-input-chromium", kAppCode); }

  void RunLoopForTouchReponse() {
    FX_LOGS(INFO) << "Wait for input_ready";
    RunLoopUntil([&] { return cpp23::contains(nav_listener_.title_, "input_ready"); });

    auto test_app_status_listener_connect =
        component::Connect<fuchsia_ui_test_input::TestAppStatusListener>();
    ZX_ASSERT_OK(test_app_status_listener_connect);
    fidl::SyncClient test_app_status_listener(std::move(test_app_status_listener_connect.value()));
    ZX_ASSERT_OK(test_app_status_listener->ReportStatus(
        {fuchsia_ui_test_input::TestAppStatus::kHandlersRegistered}));

    auto touch_input_listener_connect =
        component::Connect<fuchsia_ui_test_input::TouchInputListener>();
    ZX_ASSERT_OK(touch_input_listener_connect);
    fidl::SyncClient touch_input_listener(std::move(touch_input_listener_connect.value()));

    while (true) {
      bool got_touch_event = false;
      FX_LOGS(INFO) << "Waiting for tap response message";

      out_message_port_->ReceiveMessage().Then(
          [&touch_input_listener, &got_touch_event](auto& res) {
            ZX_ASSERT_OK(res);

            rapidjson::Document tap_resp =
                integration_tests::JsonFromBuffer(res->message().data().value());

            // Validate structure of touch response.
            FX_CHECK(tap_resp.HasMember("epoch_msec"));
            FX_CHECK(tap_resp.HasMember("x"));
            FX_CHECK(tap_resp.HasMember("y"));
            FX_CHECK(tap_resp.HasMember("device_pixel_ratio"));
            FX_CHECK(tap_resp["epoch_msec"].IsInt64());
            FX_CHECK(tap_resp["x"].IsInt());
            FX_CHECK(tap_resp["y"].IsInt());
            FX_CHECK(tap_resp["device_pixel_ratio"].IsNumber());

            FX_LOGS(INFO) << "Got touch response message at (" << tap_resp["x"].GetInt() << ", "
                          << tap_resp["y"].GetInt() << ")";

            // Relay response to parent.
            fuchsia_ui_test_input::TouchInputListenerReportTouchInputRequest request({
                .local_x = tap_resp["x"].GetInt(),
                .local_y = tap_resp["y"].GetInt(),
                .time_received = tap_resp["epoch_msec"].GetInt64() * 1000 * 1000,
                .device_pixel_ratio = tap_resp["device_pixel_ratio"].GetDouble(),
                .component_name = "web-touch-input-chromium",
            });
            ZX_ASSERT_OK(touch_input_listener->ReportTouchInput(request));

            got_touch_event = true;
          });

      RunLoopUntil([&]() { return got_touch_event; });
    }
  }

 private:
  static constexpr auto kAppCode = R"JS(
    let port;
    // Add element for touching.
    document.body.innerHTML = '<div id="click-target" style="width: 100%; height: 100%; background-color: #ff00ff;"></div>';
    let target = document.getElementById("click-target");

    target.onclick = function(event) {
      target.style.backgroundColor='#40e0d0';
      console.assert(port != null);
      let response = JSON.stringify({
        epoch_msec: Date.now(),
        x: event.screenX,
        y: event.screenY,
        device_pixel_ratio: window.devicePixelRatio,
      });
      console.info('Reporting touch event ', response);
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

    waitForVisible("click-target").then((element) => {
      document.title = [document.title, 'input_ready'].join(' ');
    });
    )JS";
};
}  // namespace

int main(int argc, const char** argv) { WebApp().RunLoopForTouchReponse(); }
