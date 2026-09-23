// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/logger/cpp/fidl.h>
#include <fuchsia/tracing/provider/cpp/fidl.h>
#include <fuchsia/ui/app/cpp/fidl.h>
#include <fuchsia/ui/composition/cpp/fidl.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/syslog/cpp/macros.h>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/ui/testing/ui_test_manager/ui_test_manager.h"
#include "src/ui/testing/util/test_view.h"

namespace accessibility_test {
namespace {

using component_testing::ChildRef;
using component_testing::ParentRef;
using component_testing::Protocol;
using component_testing::Realm;
using component_testing::Route;

constexpr auto kViewProvider = "view-provider";

// This test verifies that a11y manager can fulfill its responsibility to create
// the accessibility view on behalf of the scene owner. `SceneManager`
// will only attach a client view if the accessibility view is attached to the
// display, so verifying that the client view renders is sufficient to guarantee
// that the a11y manager behaves correctly.
class AccessibilitySceneTest
    : public gtest::RealLoopFixture,
      public ::testing::WithParamInterface<ui_testing::UITestRealm::AccessibilityOwnerType> {
 public:
  AccessibilitySceneTest() = default;
  ~AccessibilitySceneTest() override = default;

  void SetUp() override {
    ui_testing::UITestRealm::Config config;
    config.accessibility_owner = GetParam();
    config.use_scene_owner = true;
    config.ui_to_client_services = {fuchsia::ui::composition::FlatlandFactory::Name_,
                                    fuchsia::ui::composition::Allocator::Name_};
    ui_test_manager_.emplace(std::move(config));

    FX_LOGS(INFO) << "AccessibilitySceneTest: Building realm";
    realm_ = ui_test_manager_->AddSubrealm();

    test_view_access_ = std::make_unique<ui_testing::TestViewAccess>();
    // Add a test view provider.
    realm_->AddLocalChild(kViewProvider, [d = dispatcher(), a = test_view_access_]() {
      return std::make_unique<ui_testing::TestView>(
          d, /* content = */ ui_testing::TestView::ContentType::DEFAULT, a);
    });
    realm_->AddRoute(Route{.capabilities = {Protocol{fuchsia::ui::app::ViewProvider::Name_}},
                           .source = ChildRef{kViewProvider},
                           .targets = {ParentRef()}});
    realm_->AddRoute(
        Route{.capabilities = {Protocol{fuchsia::ui::composition::FlatlandFactory::Name_}},
              .source = ParentRef(),
              .targets = {ChildRef{kViewProvider}}});

    ui_test_manager_->BuildRealm();
  }

  void TearDown() override {
    bool complete = false;
    ui_test_manager_->TeardownRealm(
        [&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  }

 protected:
  std::optional<ui_testing::UITestManager> ui_test_manager_;
  std::unique_ptr<sys::ServiceDirectory> realm_exposed_services_;
  std::shared_ptr<ui_testing::TestViewAccess> test_view_access_;
  std::optional<Realm> realm_;
};

// Run test with both the real and fake a11y components, because other tests
// will rely on the fake to vend `fuchsia.accessibility.scene.Provider`.
INSTANTIATE_TEST_SUITE_P(MagnificationPixelTestWithParams, AccessibilitySceneTest,
                         ::testing::Values(ui_testing::UITestRealm::AccessibilityOwnerType::FAKE,
                                           ui_testing::UITestRealm::AccessibilityOwnerType::REAL));

TEST_P(AccessibilitySceneTest, AccessibilityViewInserted) {
  EXPECT_FALSE(ui_test_manager_->ClientViewIsRendering());

  FX_LOGS(INFO) << "Requesting to attach client view";
  ui_test_manager_->InitializeScene();

  FX_LOGS(INFO) << "Waiting for client view to render";
  RunLoopUntil([this]() { return ui_test_manager_->ClientViewIsRendering(); });
}

}  // namespace
}  // namespace accessibility_test
