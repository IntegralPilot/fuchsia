// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/ui/app/cpp/fidl.h>
#include <fuchsia/web/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_token_pair.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <string>

#include "src/ui/tests/integration_graphics_tests/web-pixel-tests/chromium_pixel_client/chromium_pixel_client_config.h"

namespace {

fuchsia::mem::Buffer BufferFromString(const std::string& script) {
  fuchsia::mem::Buffer buffer;
  uint64_t num_bytes = script.size();

  zx::vmo vmo;
  zx_status_t status = zx::vmo::create(num_bytes, 0u, &vmo);
  FX_CHECK(status >= 0);

  status = vmo.write(script.data(), 0, num_bytes);
  FX_CHECK(status >= 0);
  buffer.vmo = std::move(vmo);
  buffer.size = num_bytes;

  return buffer;
}

std::string StringFromBuffer(const fuchsia::mem::Buffer& buffer) {
  size_t num_bytes = buffer.size;
  std::string str(num_bytes, 'x');
  buffer.vmo.read(str.data(), 0, num_bytes);
  return str;
}

class NavListener : public fuchsia::web::NavigationEventListener {
 public:
  // |fuchsia::web::NavigationEventListener|
  void OnNavigationStateChanged(fuchsia::web::NavigationState nav_state,
                                OnNavigationStateChangedCallback send_ack) override {
    if (nav_state.has_is_main_document_loaded()) {
      FX_LOGS(INFO) << "nav_state.is_main_document_loaded = "
                    << nav_state.is_main_document_loaded();
      is_main_document_loaded_ = nav_state.is_main_document_loaded();
    }
    if (nav_state.has_title()) {
      FX_LOGS(INFO) << "nav_state.title = " << nav_state.title();
      loaded_title_ = true;
      if (nav_state.title() == "window_resized") {
        window_resized_ = true;
      }
    }

    send_ack();
  }
  bool loaded_title_ = false;
  bool is_main_document_loaded_ = false;
  bool window_resized_ = false;
};

// Implements a simple web app.
class WebApp final : public fuchsia::ui::app::ViewProvider {
 public:
  WebApp()
      : loop_(&kAsyncLoopConfigAttachToCurrentThread),
        context_(sys::ComponentContext::CreateAndServeOutgoingDirectory()),
        view_provider_binding_(this),
        chromium_pixel_client_config_(
            chromium_pixel_client_config::Config::TakeFromStartupHandle()) {
    FX_LOGS(INFO) << "Starting web client";
    SetupWebEngine();
    SetupViewProvider();
  }

  void Run() {
    // Set up navigation affordances.
    FX_LOGS(INFO) << "Loading web app";
    fuchsia::web::NavigationControllerPtr navigation_controller;
    NavListener navigation_event_listener;
    fidl::Binding<fuchsia::web::NavigationEventListener> navigation_event_listener_binding(
        &navigation_event_listener);
    web_frame_->SetNavigationEventListener(navigation_event_listener_binding.NewBinding());
    web_frame_->GetNavigationController(navigation_controller.NewRequest());

    // Read HTML from structured config file.
    FX_CHECK(!chromium_pixel_client_config_.html().empty());

    // Load the web page.
    FX_LOGS(INFO) << "Loading web page";
    navigation_controller->LoadUrl(
        chromium_pixel_client_config_.html(), fuchsia::web::LoadUrlParams(), [this](auto result) {
          if (result.is_err()) {
            FX_LOGS(FATAL) << "Error while loading URL: " << static_cast<uint32_t>(result.err());
          } else {
            FX_LOGS(INFO) << "Loaded " << chromium_pixel_client_config_.html();
          }
        });

    // Wait for navigation loaded |chromium_pixel_client_config.html()| page then inject JS code, to
    // avoid inject JS to wrong page.
    RunLoopUntil([&navigation_event_listener] {
      return navigation_event_listener.loaded_title_ &&
             navigation_event_listener.is_main_document_loaded_;
    });

    FX_LOGS(INFO) << "Running javascript to inject html: " << chromium_pixel_client_config_.html();
    bool is_js_loaded = false;
    web_frame_->ExecuteJavaScript({"*"}, BufferFromString(kAppCode), [&is_js_loaded](auto result) {
      if (result.is_err()) {
        FX_LOGS(FATAL) << "Error while executing JavaScript: "
                       << static_cast<uint32_t>(result.err());
      } else {
        is_js_loaded = true;
        FX_LOGS(INFO) << "Injected html";
      }
    });
    RunLoopUntil([&] { return is_js_loaded; });

    // Wait for the window resized message.
    fuchsia::web::MessagePortPtr message_port;
    bool is_port_registered = false;
    bool window_resized = false;
    SendMessageToWebPage(message_port.NewRequest(), "REGISTER_PORT");
    message_port->ReceiveMessage([&is_port_registered, &window_resized](auto web_message) {
      auto message = StringFromBuffer(web_message.data());
      // JS already saw window has size, don't wait for resize.
      if (message == "PORT_REGISTERED WINDOW_RESIZED") {
        window_resized = true;
      } else {
        FX_CHECK(message == "PORT_REGISTERED") << "Expected PORT_REGISTERED but got " << message;
      }
      is_port_registered = true;
    });
    RunLoopUntil([&] { return is_port_registered; });

    if (!window_resized) {
      RunLoopUntil(
          [&navigation_event_listener] { return navigation_event_listener.window_resized_; });
    }

    loop_.Run();
  }

 private:
  static constexpr auto kAppCode = R"JS(
    let port;
    window.onresize = function(event) {
      if (window.innerWidth != 0) {
        console.info('size: ', window.innerWidth, window.innerHeight);
        document.title = 'window_resized';
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

  void SendMessageToWebPage(fidl::InterfaceRequest<fuchsia::web::MessagePort> message_port,
                            const std::string& message) {
    fuchsia::web::WebMessage web_message;
    web_message.set_data(BufferFromString(message));

    std::vector<fuchsia::web::OutgoingTransferable> outgoing;
    outgoing.emplace_back(
        fuchsia::web::OutgoingTransferable::WithMessagePort(std::move(message_port)));
    web_message.set_outgoing_transfer(std::move(outgoing));

    web_frame_->PostMessage(/*target_origin=*/"*", std::move(web_message),
                            [](auto result) { FX_CHECK(!result.is_err()); });
  }

  // TODO(https://fxbug.dev/42055424): Remove this function when async::Loop provides support for
  // RunLoopUntil().
  template <typename PredicateT>
  void RunLoopUntil(PredicateT predicate) {
    while (!predicate()) {
      loop_.Run(zx::time::infinite(), true);
    }
  }

  void SetupWebEngine() {
    auto web_context_provider = context_->svc()->Connect<fuchsia::web::ContextProvider>();
    auto incoming_service_clone = context_->svc()->CloneChannel();
    web_context_provider.set_error_handler([](zx_status_t status) {
      FX_LOGS(WARNING) << "web_context_provider: " << zx_status_get_string(status);
    });
    FX_CHECK(incoming_service_clone.is_valid());

    fuchsia::web::CreateContextParams params;
    fuchsia::web::ContextFeatureFlags features = fuchsia::web::ContextFeatureFlags::NETWORK;
    if (chromium_pixel_client_config_.use_vulkan()) {
      features = features | fuchsia::web::ContextFeatureFlags::VULKAN;
    }
    params.set_features(features);

    params.set_service_directory(std::move(incoming_service_clone));
    web_context_provider->Create(std::move(params), web_context_.NewRequest());
    web_context_.set_error_handler([](zx_status_t status) {
      FX_LOGS(WARNING) << "web_context_: " << zx_status_get_string(status);
    });
    web_context_->CreateFrame(web_frame_.NewRequest());
    web_frame_.set_error_handler([](zx_status_t status) {
      FX_LOGS(WARNING) << "web_frame_: " << zx_status_get_string(status);
    });
    web_frame_->SetJavaScriptLogLevel(fuchsia::web::ConsoleLogLevel::INFO);
  }

  void SetupViewProvider() {
    fidl::InterfaceRequestHandler<fuchsia::ui::app::ViewProvider> handler =
        [&](fidl::InterfaceRequest<fuchsia::ui::app::ViewProvider> request) {
          if (view_provider_binding_.is_bound()) {
            request.Close(ZX_ERR_ALREADY_BOUND);
            return;
          }
          view_provider_binding_.Bind(std::move(request));
        };
    context_->outgoing()->AddPublicService(std::move(handler));
  }

  // |fuchsia::ui::app::ViewProvider|
  void CreateView2(fuchsia::ui::app::CreateView2Args args) override {
    fuchsia::web::CreateView2Args args2;
    fuchsia::ui::views::ViewCreationToken token;
    args2.set_view_creation_token(std::move(*args.mutable_view_creation_token()));
    web_frame_->CreateView2(std::move(args2));
  }

  void CreateViewWithViewRef(zx::eventpair token,
                             fuchsia::ui::views::ViewRefControl view_ref_control,
                             fuchsia::ui::views::ViewRef view_ref) override {
    ZX_PANIC("Not Implemented");
  }
  async::Loop loop_;
  std::unique_ptr<sys::ComponentContext> context_;
  fidl::Binding<fuchsia::ui::app::ViewProvider> view_provider_binding_;
  chromium_pixel_client_config::Config chromium_pixel_client_config_;
  fuchsia::web::ContextPtr web_context_;
  fuchsia::web::FramePtr web_frame_;
};

}  // namespace

int main(int argc, const char** argv) { WebApp().Run(); }
