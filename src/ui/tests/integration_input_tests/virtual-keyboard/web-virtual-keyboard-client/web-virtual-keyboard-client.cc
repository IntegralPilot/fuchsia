// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <fidl/test.virtualkeyboard/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/stdcompat/string_view.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <src/ui/testing/util/fidl_cpp_helpers.h>
#include <src/ui/tests/integration_input_tests/web-test-base/web-app-base.h>

#include "fidl/test.virtualkeyboard/cpp/markers.h"
#include "fidl/test.virtualkeyboard/cpp/natural_types.h"

namespace {

// Implements a simple web app, which responds to touch events.
class WebApp : public integration_tests::WebAppBase {
 public:
  WebApp() {
    Setup("web-virtual-keyboard", kAppCode,
          fuchsia_web::ContextFeatureFlags::kVulkan | fuchsia_web::ContextFeatureFlags::kNetwork |
              fuchsia_web::ContextFeatureFlags::kKeyboard |
              fuchsia_web::ContextFeatureFlags::kVirtualKeyboard);
  }

  void Run() {
    FX_LOGS(INFO) << "Wait for input_ready";
    RunLoopUntil([&] { return cpp23::contains(nav_listener_.title_, "input_ready"); });

    FX_LOGS(INFO) << "Requesting input position";
    auto [input_position_port_client_end, input_position_port_server_end] =
        fidl::Endpoints<fuchsia_web::MessagePort>::Create();
    SendMessageToWebPage(std::move(input_position_port_server_end), "GET_INPUT_POSITION");

    FX_LOGS(INFO) << "Waiting for input position";
    std::optional<rapidjson::Document> input_position;
    auto input_position_port =
        fidl::Client(std::move(input_position_port_client_end), dispatcher());

    input_position_port->ReceiveMessage().Then([&input_position](auto& res) {
      ZX_ASSERT_OK(res);
      input_position = integration_tests::JsonFromBuffer(res->message().data().value());
    });
    RunLoopUntil([&] { return input_position.has_value(); });

    // Validate structure of input position.
    FX_LOGS(INFO) << "Return input position to test fixture";
    const auto& input_pos = input_position.value();
    for (const auto& element : {"left", "right", "top", "bottom"}) {
      FX_CHECK(input_pos.HasMember(element)) << "HasMember failed for " << element;
      // Apparently sometimes these values can be floating points too.
      FX_CHECK(input_pos[element].IsNumber()) << "IsNumber failed for " << element;
    }

    // Relay position to parent.
    auto position_listener_connect =
        component::Connect<test_virtualkeyboard::InputPositionListener>();
    ZX_ASSERT_OK(position_listener_connect);
    fidl::SyncClient position_listener(std::move(position_listener_connect.value()));

    test_virtualkeyboard::InputPositionListenerNotifyRequest req;
    req.bounding_box().x0() = static_cast<uint32_t>(input_pos["left"].GetFloat());
    req.bounding_box().y0() = static_cast<uint32_t>(input_pos["top"].GetFloat());
    req.bounding_box().x1() = static_cast<uint32_t>(input_pos["right"].GetFloat());
    req.bounding_box().y1() = static_cast<uint32_t>(input_pos["bottom"].GetFloat());

    ZX_ASSERT_OK(position_listener->Notify(req));

    auto test_app_status_listener_connect =
        component::Connect<fuchsia_ui_test_input::TestAppStatusListener>();
    ZX_ASSERT_OK(test_app_status_listener_connect);
    fidl::SyncClient test_app_status_listener(std::move(test_app_status_listener_connect.value()));
    ZX_ASSERT_OK(test_app_status_listener->ReportStatus(
        {fuchsia_ui_test_input::TestAppStatus::kHandlersRegistered}));

    RunLoop();
  }

 private:
  // The application code that will be loaded up.
  static constexpr auto kAppCode = R"JS(
    console.info('injecting body');
    // Create a page with a single input box.
    // * When the user taps inside the input box (and the keyboard is currently hidden),
    //   web-engine should request the virtual keyboard be made visible.
    // * When the user taps outside the input box (and the keyboard is currently visible),
    //   web-engine should request the virtual keyboard me made hidden.
    document.write('<html><body><input id="textbox" /></body></html>');
    document.body.style.backgroundColor='#ff00ff';
    document.body.onclick = function(event) {
      document.body.style.backgroundColor='#40e0d0';
      let touch_event = JSON.stringify({
        x: event.screenX,
        y: event.screenY,
      });
      console.info('Got touch event ', touch_event);
    };

    // Report a window resize event by changing the document title.
    window.onresize = function(event) {
      if (window.innerWidth != 0) {
        console.info('size: ', window.innerWidth, window.innerHeight);
        document.title = [document.title, 'window_resized'].join(' ');
      }
    };

    let port;

    function receiveMessage(event) {
      if (event.data == "REGISTER_PORT") {
        console.log("received REGISTER_PORT");
        port = event.ports[0];
        if (window.innerWidth != 0) {
          // If the window was resized before JS loaded, notify the test
          // fixture so that it skips waiting for the resize to happen.
          port.postMessage('PORT_REGISTERED WINDOW_RESIZED');
        } else {
          port.postMessage('PORT_REGISTERED');
        }
      } else if (event.data == "GET_INPUT_POSITION") {
        let message = JSON.stringify(document.getElementById('textbox').getBoundingClientRect());
        console.info('sending input position ', message);
        event.ports[0].postMessage(message);
      } else {
        console.error('ignoring unexpected message: ' + event.data);
      }
    };
    window.addEventListener('message', receiveMessage, false);

    waitForVisible("textbox").then((element) => {
      document.title = [document.title, 'input_ready'].join(' ');
    });
    )JS";
};

}  // namespace

int main(int argc, const char** argv) { WebApp().Run(); }
