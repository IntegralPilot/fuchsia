// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/web/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include <gtest/gtest.h>

#include "lib/sys/component/cpp/testing/realm_builder_types.h"
#include "src/chromium/web_runner_tests/test_server.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

// This file contains a subset of adapted Chromium Fuchsia tests to make sure
// nothing broke on the import boundary.
//
// See also: https://chromium.googlesource.com/chromium/src/+/HEAD/fuchsia
namespace {
using namespace component_testing;

class MockNavigationEventListener : public fuchsia::web::NavigationEventListener {
 public:
  // |fuchsia::web::NavigationEventListener|
  void OnNavigationStateChanged(fuchsia::web::NavigationState change,
                                OnNavigationStateChangedCallback callback) override {
    if (on_navigation_state_changed_) {
      on_navigation_state_changed_(std::move(change));
    }

    callback();
  }

  void set_on_navigation_state_changed(fit::function<void(fuchsia::web::NavigationState)> fn) {
    on_navigation_state_changed_ = std::move(fn);
  }

 private:
  fit::function<void(fuchsia::web::NavigationState)> on_navigation_state_changed_;
};

class ChromiumAppTest : public gtest::RealLoopFixture,
                        public ::testing::WithParamInterface<fuchsia::web::ContextFeatureFlags> {
 protected:
  ChromiumAppTest() : context_(sys::ComponentContext::CreateAndServeOutgoingDirectory()) {
    auto realm_builder = RealmBuilder::Create();
    realm_builder.AddChild("context_provider",
                           "fuchsia-pkg://fuchsia.com/web_engine#meta/context_provider.cm");

    // Capabilities that must be given to ContextProvider.
    realm_builder.AddRoute(
        Route{.capabilities = {Protocol{"fuchsia.logger.LogSink"},
                               Protocol{"fuchsia.feedback.ComponentDataRegister"},
                               Protocol{"fuchsia.feedback.CrashReportingProductRegister"},
                               Directory{
                                   .name = "tzdata-icu",
                                   .rights = fuchsia::io::R_STAR_DIR,
                               },
                               Directory{
                                   .name = "root-ssl-certificates",
                                   .rights = fuchsia::io::R_STAR_DIR,
                               }},
              .source = ParentRef(),
              .targets = {ChildRef{"context_provider"}}});

    // Expose the ContextProvider capabilities to the test.
    realm_builder.AddRoute(Route{.capabilities = {Protocol{"fuchsia.web.ContextProvider"}},
                                 .source = ChildRef{"context_provider"},
                                 .targets = {ParentRef()}});

    realm_ = std::make_unique<RealmRoot>(realm_builder.Build(dispatcher()));
    auto incoming_service_clone = context_->svc()->CloneChannel();
    auto web_context_provider = realm_->component().Connect<fuchsia::web::ContextProvider>();
    web_context_provider.set_error_handler([](zx_status_t status) {
      FX_LOGS(ERROR) << "web_context_provider: " << zx_status_get_string(status);
    });
    FX_CHECK(incoming_service_clone.is_valid());

    fuchsia::web::CreateContextParams params;
    params.set_service_directory(std::move(incoming_service_clone));
    params.set_features(fuchsia::web::ContextFeatureFlags::NETWORK | GetParam());
    web_context_provider->Create(std::move(params), web_context_.NewRequest());
    web_context_.set_error_handler([](zx_status_t status) {
      FX_LOGS(ERROR) << "web_context_: " << zx_status_get_string(status);
    });
    web_context_->CreateFrame(web_frame_.NewRequest());
    web_frame_.set_error_handler([](zx_status_t status) {
      FX_LOGS(ERROR) << "web_frame_: " << zx_status_get_string(status);
    });
  }
  fuchsia::web::Frame* web_frame() const { return web_frame_.get(); }

  void Navigate(const std::string& url) {
    // By creating a new NavigationController for each Navigate() call, we
    // implicitly ensure that any preceding calls to the Frame must have executed
    // before LoadUrl() is handled.
    fuchsia::web::NavigationControllerPtr navigation;
    web_frame_->GetNavigationController(navigation.NewRequest());
    navigation->LoadUrl(url, fuchsia::web::LoadUrlParams(),
                        [](fuchsia::web::NavigationController_LoadUrl_Result) {});
  }

 private:
  std::unique_ptr<sys::ComponentContext> context_;
  std::unique_ptr<RealmRoot> realm_;
  fuchsia::web::ContextPtr web_context_;
  fuchsia::web::FramePtr web_frame_;
};

// This test ensures that we can interact with the fuchsia.web FIDL.
//
// See also
// https://chromium.googlesource.com/chromium/src/+/HEAD/fuchsia/engine/browser/context_impl_browsertest.cc
TEST_P(ChromiumAppTest, CreateAndNavigate) {
  MockNavigationEventListener navigation_event_listener;
  fidl::Binding<fuchsia::web::NavigationEventListener> navigation_event_listener_binding(
      &navigation_event_listener);
  web_frame()->SetNavigationEventListener(navigation_event_listener_binding.NewBinding());
  navigation_event_listener_binding.set_error_handler([](zx_status_t status) {
    FAIL() << "navigation_event_listener_binding: " << zx_status_get_string(status);
  });

  std::string observed_url;
  std::string observed_title;

  navigation_event_listener.set_on_navigation_state_changed(
      [this, &navigation_event_listener, &observed_url,
       &observed_title](fuchsia::web::NavigationState change) {
        if (change.has_url()) {
          observed_url = change.url();
        }
        if (change.has_title()) {
          observed_title = change.title();
        }

        if (change.has_page_type()) {
          EXPECT_EQ(change.page_type(), fuchsia::web::PageType::NORMAL);
        }

        if (change.has_is_main_document_loaded() && change.is_main_document_loaded()) {
          navigation_event_listener.set_on_navigation_state_changed(nullptr);
          QuitLoop();
        }
      });

  web_runner_tests::TestServer server;
  FX_CHECK(server.FindAndBindPort());

  const std::string url = fxl::StringPrintf("http://localhost:%d/foo.html", server.port());
  Navigate(url);

  ASSERT_TRUE(server.Accept());

  const std::string expected_prefix = "GET /foo.html HTTP";
  std::string buf(expected_prefix.size(), 0);
  ASSERT_TRUE(server.Read(&buf));
  EXPECT_EQ(expected_prefix, buf.substr(0, expected_prefix.size()));
  FX_CHECK(server.WriteContent(R"(<!doctype html>
      <html>
        <head>
          <title>Test title!</title>
        </head>
      </html>)"));

  EXPECT_FALSE(RunLoopWithTimeout(zx::sec(5))) << "Timed out waiting for navigation events";

  EXPECT_EQ(url, observed_url);
  EXPECT_EQ("Test title!", observed_title);
}

INSTANTIATE_TEST_SUITE_P(ContextFeatureFlags, ChromiumAppTest,
                         ::testing::Values(fuchsia::web::ContextFeatureFlags(),
                                           fuchsia::web::ContextFeatureFlags::HEADLESS,
                                           fuchsia::web::ContextFeatureFlags::VULKAN));

}  // namespace
