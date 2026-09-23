// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The contents of this web application are heavily borrowed from prior work
// such as mouse-input-chromium.cc, virtual-keyboard-test.cc and other
// similar efforts.

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

// Implements a simple web app, which responds to tap and keyboard events.
class WebApp : public integration_tests::WebAppBase {
 public:
  WebApp() {
    Setup("text-input-chromium", kAppCode,
          fuchsia_web::ContextFeatureFlags::kVulkan | fuchsia_web::ContextFeatureFlags::kNetwork |
              fuchsia_web::ContextFeatureFlags::kKeyboard);
  }

  void RunLoopForKeyEvents() {
    FX_LOGS(INFO) << "Wait for input_ready";
    RunLoopUntil([&] { return cpp23::contains(nav_listener_.title_, "input_ready"); });

    auto test_app_status_listener_connect =
        component::Connect<fuchsia_ui_test_input::TestAppStatusListener>();
    ZX_ASSERT_OK(test_app_status_listener_connect);
    fidl::SyncClient test_app_status_listener(std::move(test_app_status_listener_connect.value()));
    ZX_ASSERT_OK(test_app_status_listener->ReportStatus(
        {fuchsia_ui_test_input::TestAppStatus::kHandlersRegistered}));

    FX_LOGS(INFO) << "Wait for text_input_focused";
    RunLoopUntil([&] { return cpp23::contains(nav_listener_.title_, "text_input_focused"); });

    ZX_ASSERT_OK(test_app_status_listener->ReportStatus(
        {fuchsia_ui_test_input::TestAppStatus::kElementFocused}));

    // Watch for any changes in the text area, and forward repeatedly to the
    // response listener in the test fixture.
    auto keyboard_input_listener_connect =
        component::Connect<fuchsia_ui_test_input::KeyboardInputListener>();
    ZX_ASSERT_OK(keyboard_input_listener_connect);
    fidl::SyncClient keyboard_input_listener(std::move(keyboard_input_listener_connect.value()));
    for (;;) {
      // This WebMessage comes from the Javascript code (below).
      std::optional<fuchsia_web::WebMessage> received;
      out_message_port_->ReceiveMessage().Then([&received](auto& res) {
        ZX_ASSERT_OK(res);
        received = std::move(res->message());
      });
      RunLoopUntil([&received] { return received.has_value(); });

      // Forward the message to the test fixture.
      auto str = integration_tests::StringFromBuffer(received.value().data().value());
      ZX_ASSERT_OK(keyboard_input_listener->ReportTextInput({{str}}));
    }
  }

 private:
  // The application code that will be loaded up.
  static constexpr auto kAppCode = R"JS(
    let port;

    // Report a window resize event by changing the document title.
    window.onresize = function(event) {
      if (window.innerWidth != 0) {
        console.info('size: ', window.innerWidth, window.innerHeight);
        document.title = [document.title, 'window_resized'].join(' ');
      }
    };

    // Registers a port for sending messages between the web engine and this
    // web app.
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
      } else {
        console.error('received unexpected message: ' + event.data);
      }
    };

    function sendMessageEvent(messageObj) {
      let message = JSON.stringify(messageObj);
      console.info('sendMessageEvent: ' + message);
      port.postMessage(message);
    }

    const headHtml = `
    <style>
      body {
        height: 100%;
        width: 100%;
        background-color: #000077; /* dark blue */
        color: white;
      }
      #text-input {
        height: 100%;
        width: 100%;
        background-color: #ca2c92; /* royal fuchsia */
        font-size: 36pt;
      }
    </style>
    `;

    // Installs a large text field. The text field occupies most of the
    // screen for easy navigation.
    document.head.innerHTML += headHtml;
    document.body.innerHTML = `<textarea id="text-input" rows="3" cols="20"></textarea>`;

    /** @type HTMLInputElement */
    let input_element = document.querySelector("#text-input");

    // Every time a keyup event happens on input, relay the key to the web app.
    // "keyup" is selected instead of "keydown" because "keydown" will show us
    // the *previous* state of the text area.
    input_element.addEventListener("keyup", function (e) {
      sendMessageEvent({
        text: input_element.value,
      });
    });

    // Sends a signal that the text area is focused, when that happens. The
    // easiest way to do that is to change the document title. There is a
    // navigation listener which will get notified of the title change.
    input_element.addEventListener('focus', function (e) {
      document.title = [document.title, 'text_input_focused'].join(' ');
    });

    window.addEventListener('message', receiveMessage, false);

    waitForVisible("text-input").then((element) => {
      document.title = [document.title, 'input_ready'].join(' ');
    });

    console.info('JS loaded');
  )JS";
};

}  // namespace

int main(int argc, const char** argv) { WebApp().RunLoopForKeyEvents(); }
