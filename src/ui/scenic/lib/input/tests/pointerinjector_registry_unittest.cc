// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/pointerinjector_registry.h"

#include <fidl/fuchsia.ui.pointerinjector/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_ref_pair.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/lib/utils/helpers.h"

using fuchsia_ui_pointerinjector::DeviceType;
using fuchsia_ui_pointerinjector::DispatchPolicy;
using fuchsia_ui_views::ViewRef;
// Unit tests for the PointerinjectorRegistry class.

namespace input::test {

namespace {

// clang-format off
static constexpr std::array<float, 9> kIdentityMatrix = {
  1, 0, 0, // first column
  0, 1, 0, // second column
  0, 0, 1, // third column
};
// clang-format on

std::vector<fuchsia_ui_pointerinjector::Event> EventsTemplate() {
  fuchsia_ui_pointerinjector::Event event;
  event.timestamp(1);
  fuchsia_ui_pointerinjector::PointerSample pointer;
  pointer.pointer_id(1);
  pointer.phase(fuchsia_ui_pointerinjector::EventPhase::kAdd);
  pointer.position_in_viewport(std::array<float, 2>{1.f, 1.f});
  event.data(fuchsia_ui_pointerinjector::Data::WithPointerSample(std::move(pointer)));

  std::vector<fuchsia_ui_pointerinjector::Event> events;
  events.emplace_back(std::move(event));
  return events;
}

class DeviceEventHandler : public fidl::AsyncEventHandler<fuchsia_ui_pointerinjector::Device> {
 public:
  bool error_fired = false;
  void on_fidl_error(fidl::UnbindInfo error) override { error_fired = true; }
};

}  // namespace

class PointerinjectorRegistryTest : public gtest::TestLoopFixture {
 public:
  PointerinjectorRegistryTest()
      : snapshot_holder_(std::make_shared<view_tree::SnapshotHolder>()),
        registry_(
            dispatcher(), snapshot_holder_,
            /*inject_touch_exclusive=*/[](auto...) {},
            /*inject_touch_hit_tested=*/[](auto...) {},
            /*inject_mouse_exclusive=*/[](auto...) {},
            /*inject_mouse_hit_tested=*/[](auto...) {},
            /*cancel_mouse_stream=*/[](auto...) {}, inspect::Node()) {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Registry>::Create();
    registry_.Bind(std::move(server_end));
    registry_client_.Bind(std::move(client_end), dispatcher());
  }

 protected:
  struct ScenePair {
    scenic::cpp::ViewRefPair parent;
    scenic::cpp::ViewRefPair child;

    ScenePair() : parent(scenic::cpp::ViewRefPair::New()), child(scenic::cpp::ViewRefPair::New()) {}
  };

  fuchsia_ui_pointerinjector::Config ConfigTemplate(const ViewRef& context_view_ref,
                                                    const ViewRef& target_view_ref) {
    fuchsia_ui_pointerinjector::Config config;
    config.device_id(1);
    config.device_type(DeviceType::kTouch);
    config.dispatch_policy(DispatchPolicy::kExclusiveTarget);
    {
      fuchsia_ui_pointerinjector::Viewport viewport;
      viewport.extents(std::array<std::array<float, 2>, 2>{{{0, 0}, {10, 10}}});
      viewport.viewport_to_context_transform(kIdentityMatrix);
      config.viewport(std::move(viewport));
    }
    config.context(
        fuchsia_ui_pointerinjector::Context::WithView(scenic::cpp::CloneViewRef(context_view_ref)));
    config.target(
        fuchsia_ui_pointerinjector::Target::WithView(scenic::cpp::CloneViewRef(target_view_ref)));
    return config;
  }

  ScenePair SetupSceneWithParentAndChildViews() {
    ScenePair scene_pair;
    const zx_koid_t parent_koid = utils::ExtractKoid(scene_pair.parent.view_ref);
    const zx_koid_t child_koid = utils::ExtractKoid(scene_pair.child.view_ref);

    auto snapshot = std::make_shared<view_tree::Snapshot>();
    snapshot->sequence_number = next_sequence_number_++;
    snapshot->root = parent_koid;
    snapshot->view_tree[parent_koid] = {.children = {child_koid}};
    snapshot->view_tree[child_koid] = {.parent = parent_koid};
    snapshot_holder_->SetSnapshot(snapshot);

    return scene_pair;
  }

  std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder_;
  scenic_impl::input::PointerinjectorRegistry registry_;
  fidl::Client<fuchsia_ui_pointerinjector::Registry> registry_client_;

 protected:
  uint64_t next_sequence_number_ = 1;

 private:
  utils::ScopedThreadDispatcherSetter dispatcher_setter_{dispatcher(), dispatcher()};
};

class PointerinjectorRegistryTestP : public PointerinjectorRegistryTest,
                                     public testing::WithParamInterface<bool> {
 public:
  bool use_inject_events() const { return GetParam(); }

  void Inject(fidl::Client<fuchsia_ui_pointerinjector::Device>& injector,
              std::vector<fuchsia_ui_pointerinjector::Event> events,
              std::function<void()> callback) {
    if (use_inject_events()) {
      fuchsia_ui_pointerinjector::DeviceInjectRequest request;
      request.events(std::move(events));
      auto result = injector->InjectEvents(std::move(request));
      EXPECT_TRUE(result.is_ok());
    } else {
      fuchsia_ui_pointerinjector::DeviceInjectRequest request;
      request.events(std::move(events));
      injector->Inject(std::move(request)).Then([callback = std::move(callback)](auto& result) {
        if (callback) {
          callback();
        }
      });
    }
  }
};

INSTANTIATE_TEST_SUITE_P(PointerinjectorRegistryTest, PointerinjectorRegistryTestP,
                         testing::Bool());

TEST_F(PointerinjectorRegistryTest, RegisterAttemptWithCorrectArguments_ShouldSucceed) {
  const auto [parent, child] = SetupSceneWithParentAndChildViews();

  DeviceEventHandler event_handler;
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
  bool register_callback_fired = false;
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  injector.Bind(std::move(client_end), dispatcher(), &event_handler);
  {
    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });
  }

  RunLoopUntilIdle();

  EXPECT_TRUE(register_callback_fired);
  EXPECT_FALSE(event_handler.error_fired);
}

TEST_F(PointerinjectorRegistryTest, RegisterAttemptWithBadDeviceConfig_ShouldFail) {
  const auto [parent, child] = SetupSceneWithParentAndChildViews();

  {  // No device id.
    DeviceEventHandler event_handler;
    fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);

    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
    config.device_id().reset();

    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });

    RunLoopUntilIdle();

    EXPECT_TRUE(register_callback_fired);
    EXPECT_TRUE(event_handler.error_fired);
  }

  {  // No device type.
    DeviceEventHandler event_handler;
    fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);

    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
    config.device_type().reset();

    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });

    RunLoopUntilIdle();

    EXPECT_TRUE(register_callback_fired);
    EXPECT_TRUE(event_handler.error_fired);
  }

  {  // Wrong device type.
    DeviceEventHandler event_handler;
    fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);

    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
    // Set to not TOUCH.
    config.device_type(static_cast<DeviceType>(12421));

    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });

    RunLoopUntilIdle();

    EXPECT_FALSE(register_callback_fired);
    EXPECT_TRUE(event_handler.error_fired);
  }
}

TEST_F(PointerinjectorRegistryTest, RegisterAttemptWithBadContextOrTarget_ShouldFail) {
  const auto [parent, child] = SetupSceneWithParentAndChildViews();

  {  // No context.
    DeviceEventHandler event_handler;
    fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);

    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
    config.context().reset();

    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });

    RunLoopUntilIdle();

    EXPECT_TRUE(register_callback_fired);
    EXPECT_TRUE(event_handler.error_fired);
  }

  {  // No target.
    DeviceEventHandler event_handler;
    fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);

    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
    config.target().reset();

    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });

    RunLoopUntilIdle();

    EXPECT_TRUE(register_callback_fired);
    EXPECT_TRUE(event_handler.error_fired);
  }

  {  // Context equals target.
    DeviceEventHandler event_handler;
    fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);

    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, parent.view_ref);

    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });

    RunLoopUntilIdle();

    EXPECT_TRUE(register_callback_fired);
    EXPECT_TRUE(event_handler.error_fired);
  }

  {  // Context is descendant of target.
    DeviceEventHandler event_handler;
    fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);

    // Swap context and target.
    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(child.view_ref, parent.view_ref);

    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });

    RunLoopUntilIdle();

    EXPECT_TRUE(register_callback_fired);
    EXPECT_TRUE(event_handler.error_fired);
  }

  {  // Context is unregistered.
    DeviceEventHandler event_handler;
    fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);

    auto unregistered = scenic::cpp::ViewRefPair::New();
    fuchsia_ui_pointerinjector::Config config =
        ConfigTemplate(unregistered.view_ref, child.view_ref);

    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });

    RunLoopUntilIdle();

    EXPECT_TRUE(register_callback_fired);
    EXPECT_TRUE(event_handler.error_fired);
  }

  {  // Target is unregistered.
    DeviceEventHandler event_handler;
    fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);

    auto unregistered = scenic::cpp::ViewRefPair::New();
    fuchsia_ui_pointerinjector::Config config =
        ConfigTemplate(parent.view_ref, unregistered.view_ref);

    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });

    RunLoopUntilIdle();

    EXPECT_TRUE(register_callback_fired);
    EXPECT_TRUE(event_handler.error_fired);
  }

  {  // Context is detached from scene.
    DeviceEventHandler event_handler;
    fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);

    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);

    // Empty the scene.
    auto empty_snapshot = std::make_shared<view_tree::Snapshot>();
    empty_snapshot->sequence_number = next_sequence_number_++;
    snapshot_holder_->SetSnapshot(empty_snapshot);

    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });

    RunLoopUntilIdle();

    EXPECT_TRUE(register_callback_fired);
    EXPECT_TRUE(event_handler.error_fired);
  }
}

TEST_F(PointerinjectorRegistryTest, RegisterAttemptWithBadDispatchPolicy_ShouldFail) {
  const auto [parent, child] = SetupSceneWithParentAndChildViews();

  {  // No dispatch policy.
    DeviceEventHandler event_handler;
    fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);

    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
    config.dispatch_policy().reset();

    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });

    RunLoopUntilIdle();

    EXPECT_TRUE(register_callback_fired);
    EXPECT_TRUE(event_handler.error_fired);
  }

  {  // Unsupported dispatch policy.
    DeviceEventHandler event_handler;
    fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);

    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
    config.dispatch_policy(static_cast<DispatchPolicy>(6323));

    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });

    RunLoopUntilIdle();

    EXPECT_FALSE(register_callback_fired);
    EXPECT_TRUE(event_handler.error_fired);
  }
}

TEST_F(PointerinjectorRegistryTest, ChannelDying_ShouldNotCrash) {
  const auto [parent, child] = SetupSceneWithParentAndChildViews();

  {
    DeviceEventHandler event_handler;
    fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);

    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });

    RunLoopUntilIdle();

    EXPECT_TRUE(register_callback_fired);
    EXPECT_FALSE(event_handler.error_fired);
  }  // |injector| goes out of scope.

  RunLoopUntilIdle();
}

TEST_F(PointerinjectorRegistryTest, MultipleRegistrations_ShouldSucceed) {
  const auto [parent, child] = SetupSceneWithParentAndChildViews();

  DeviceEventHandler event_handler;
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
  {
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);
    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });
    RunLoopUntilIdle();
    EXPECT_TRUE(register_callback_fired);
    EXPECT_FALSE(event_handler.error_fired);
  }

  DeviceEventHandler event_handler2;
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector2;
  {
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector2.Bind(std::move(client_end), dispatcher(), &event_handler2);

    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
    registry_client_->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });
    RunLoopUntilIdle();
    EXPECT_TRUE(register_callback_fired);
    EXPECT_FALSE(event_handler2.error_fired);
  }
}

TEST_P(PointerinjectorRegistryTestP,
       TouchDeviceAndExclusivePolicy_ShouldTriggerExclusiveTouchInjectFunc) {
  bool exclusive_touch_used = false;
  bool hit_tested_touch_used = false;
  bool exclusive_mouse_used = false;
  bool hit_tested_mouse_used = false;
  scenic_impl::input::PointerinjectorRegistry registry(
      dispatcher(), snapshot_holder_,
      /*inject_touch_exclusive*/ [&exclusive_touch_used](auto...) { exclusive_touch_used = true; },
      /*inject_touch_hit_tested*/
      [&hit_tested_touch_used](auto...) { hit_tested_touch_used = true; },
      /*inject_mouse_exclusive*/ [&exclusive_mouse_used](auto...) { exclusive_mouse_used = true; },
      /*inject_mouse_hit_tested*/
      [&hit_tested_mouse_used](auto...) { hit_tested_mouse_used = true; },
      /*cancel_mouse_stream=*/[](auto...) {});
  auto [reg_client_end, reg_server_end] =
      fidl::Endpoints<fuchsia_ui_pointerinjector::Registry>::Create();
  registry.Bind(std::move(reg_server_end));
  fidl::Client registry_client(std::move(reg_client_end), dispatcher());

  const auto [parent, child] = SetupSceneWithParentAndChildViews();

  DeviceEventHandler event_handler;
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
  bool register_callback_fired = false;
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  injector.Bind(std::move(client_end), dispatcher(), &event_handler);
  fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
  config.device_type(DeviceType::kTouch);
  config.dispatch_policy(DispatchPolicy::kExclusiveTarget);
  registry_client->Register({std::move(config), std::move(server_end)})
      .Then([&register_callback_fired](auto& result) {
        if (result.is_ok()) {
          register_callback_fired = true;
        }
      });
  RunLoopUntilIdle();
  EXPECT_TRUE(register_callback_fired);
  EXPECT_FALSE(event_handler.error_fired);

  Inject(injector, EventsTemplate(), [] {});
  RunLoopUntilIdle();
  EXPECT_TRUE(exclusive_touch_used);
  EXPECT_FALSE(hit_tested_touch_used);
  EXPECT_FALSE(exclusive_mouse_used);
  EXPECT_FALSE(hit_tested_mouse_used);
}

TEST_P(PointerinjectorRegistryTestP,
       TouchDeviceAndHitTestPolicy_ShouldTriggerHitTestedTouchInjectFunc) {
  bool exclusive_touch_used = false;
  bool hit_tested_touch_used = false;
  bool exclusive_mouse_used = false;
  bool hit_tested_mouse_used = false;
  scenic_impl::input::PointerinjectorRegistry registry(
      dispatcher(), snapshot_holder_,
      /*inject_touch_exclusive*/ [&exclusive_touch_used](auto...) { exclusive_touch_used = true; },
      /*inject_touch_hit_tested*/
      [&hit_tested_touch_used](auto...) { hit_tested_touch_used = true; },
      /*inject_mouse_exclusive*/ [&exclusive_mouse_used](auto...) { exclusive_mouse_used = true; },
      /*inject_mouse_hit_tested*/
      [&hit_tested_mouse_used](auto...) { hit_tested_mouse_used = true; },
      /*cancel_mouse_stream=*/[](auto...) {});
  auto [reg_client_end, reg_server_end] =
      fidl::Endpoints<fuchsia_ui_pointerinjector::Registry>::Create();
  registry.Bind(std::move(reg_server_end));
  fidl::Client registry_client(std::move(reg_client_end), dispatcher());

  const auto [parent, child] = SetupSceneWithParentAndChildViews();

  DeviceEventHandler event_handler;
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
  bool register_callback_fired = false;
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  injector.Bind(std::move(client_end), dispatcher(), &event_handler);
  fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
  config.device_type(DeviceType::kTouch);
  config.dispatch_policy(DispatchPolicy::kTopHitAndAncestorsInTarget);
  registry_client->Register({std::move(config), std::move(server_end)})
      .Then([&register_callback_fired](auto& result) {
        if (result.is_ok()) {
          register_callback_fired = true;
        }
      });
  RunLoopUntilIdle();
  EXPECT_TRUE(register_callback_fired);
  EXPECT_FALSE(event_handler.error_fired);

  Inject(injector, EventsTemplate(), [] {});
  RunLoopUntilIdle();
  EXPECT_FALSE(exclusive_touch_used);
  EXPECT_TRUE(hit_tested_touch_used);
  EXPECT_FALSE(exclusive_mouse_used);
  EXPECT_FALSE(hit_tested_mouse_used);
}

TEST_F(PointerinjectorRegistryTest, MouseDevice_CanRegisterMouseWithoutButtons) {
  scenic_impl::input::PointerinjectorRegistry registry(
      dispatcher(), snapshot_holder_,
      /*inject_touch_exclusive*/ [](auto...) {},
      /*inject_touch_hit_tested*/ [](auto...) {},
      /*inject_mouse_exclusive*/ [](const auto&...) {},
      /*inject_mouse_hit_tested*/ [](const auto&...) {},
      /*cancel_mouse_stream=*/[](auto...) {});
  auto [reg_client_end, reg_server_end] =
      fidl::Endpoints<fuchsia_ui_pointerinjector::Registry>::Create();
  registry.Bind(std::move(reg_server_end));
  fidl::Client registry_client(std::move(reg_client_end), dispatcher());

  const auto [parent, child] = SetupSceneWithParentAndChildViews();

  DeviceEventHandler event_handler;
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
  bool register_callback_fired = false;
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  injector.Bind(std::move(client_end), dispatcher(), &event_handler);
  fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
  config.device_type(DeviceType::kMouse);
  config.dispatch_policy(DispatchPolicy::kExclusiveTarget);
  config.buttons().reset();
  registry_client->Register({std::move(config), std::move(server_end)})
      .Then([&register_callback_fired](auto& result) {
        if (result.is_ok()) {
          register_callback_fired = true;
        }
      });
  RunLoopUntilIdle();
  EXPECT_TRUE(register_callback_fired);
  EXPECT_FALSE(event_handler.error_fired);
}

TEST_P(PointerinjectorRegistryTestP,
       MouseDeviceAndExclusivePolicy_ShouldTriggerExclusiveMouseInjectFunc) {
  bool exclusive_touch_used = false;
  bool hit_tested_touch_used = false;
  bool exclusive_mouse_used = false;
  bool hit_tested_mouse_used = false;
  scenic_impl::input::PointerinjectorRegistry registry(
      dispatcher(), snapshot_holder_,
      /*inject_touch_exclusive*/ [&exclusive_touch_used](auto...) { exclusive_touch_used = true; },
      /*inject_touch_hit_tested*/
      [&hit_tested_touch_used](auto...) { hit_tested_touch_used = true; },
      /*inject_mouse_exclusive*/ [&exclusive_mouse_used](auto...) { exclusive_mouse_used = true; },
      /*inject_mouse_hit_tested*/
      [&hit_tested_mouse_used](auto...) { hit_tested_mouse_used = true; },
      /*cancel_mouse_stream=*/[](auto...) {});
  auto [reg_client_end, reg_server_end] =
      fidl::Endpoints<fuchsia_ui_pointerinjector::Registry>::Create();
  registry.Bind(std::move(reg_server_end));
  fidl::Client registry_client(std::move(reg_client_end), dispatcher());

  const auto [parent, child] = SetupSceneWithParentAndChildViews();

  DeviceEventHandler event_handler;
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
  bool register_callback_fired = false;
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  injector.Bind(std::move(client_end), dispatcher(), &event_handler);
  fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
  config.device_type(DeviceType::kMouse);
  config.dispatch_policy(DispatchPolicy::kExclusiveTarget);
  config.buttons(std::vector<uint8_t>{0});
  registry_client->Register({std::move(config), std::move(server_end)})
      .Then([&register_callback_fired](auto& result) {
        if (result.is_ok()) {
          register_callback_fired = true;
        }
      });
  RunLoopUntilIdle();
  EXPECT_TRUE(register_callback_fired);
  EXPECT_FALSE(event_handler.error_fired);

  Inject(injector, EventsTemplate(), [] {});
  RunLoopUntilIdle();
  EXPECT_FALSE(exclusive_touch_used);
  EXPECT_FALSE(hit_tested_touch_used);
  EXPECT_TRUE(exclusive_mouse_used);
  EXPECT_FALSE(hit_tested_mouse_used);
}

TEST_P(PointerinjectorRegistryTestP,
       MouseDeviceAndHitTestPolicy_ShouldTriggerHitTestedMouseInjectFunc) {
  bool exclusive_touch_used = false;
  bool hit_tested_touch_used = false;
  bool exclusive_mouse_used = false;
  bool hit_tested_mouse_used = false;
  scenic_impl::input::PointerinjectorRegistry registry(
      dispatcher(), snapshot_holder_,
      /*inject_touch_exclusive*/ [&exclusive_touch_used](auto...) { exclusive_touch_used = true; },
      /*inject_touch_hit_tested*/
      [&hit_tested_touch_used](auto...) { hit_tested_touch_used = true; },
      /*inject_mouse_exclusive*/ [&exclusive_mouse_used](auto...) { exclusive_mouse_used = true; },
      /*inject_mouse_hit_tested*/
      [&hit_tested_mouse_used](auto...) { hit_tested_mouse_used = true; },
      /*cancel_mouse_stream=*/[](auto...) {});
  auto [reg_client_end, reg_server_end] =
      fidl::Endpoints<fuchsia_ui_pointerinjector::Registry>::Create();
  registry.Bind(std::move(reg_server_end));
  fidl::Client registry_client(std::move(reg_client_end), dispatcher());

  const auto [parent, child] = SetupSceneWithParentAndChildViews();

  DeviceEventHandler event_handler;
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
  bool register_callback_fired = false;
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  injector.Bind(std::move(client_end), dispatcher(), &event_handler);
  fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
  config.device_type(DeviceType::kMouse);
  config.dispatch_policy(DispatchPolicy::kMouseHoverAndLatchInTarget);
  config.buttons(std::vector<uint8_t>{0});
  registry_client->Register({std::move(config), std::move(server_end)})
      .Then([&register_callback_fired](auto& result) {
        if (result.is_ok()) {
          register_callback_fired = true;
        }
      });
  RunLoopUntilIdle();
  EXPECT_TRUE(register_callback_fired);
  EXPECT_FALSE(event_handler.error_fired);

  Inject(injector, EventsTemplate(), [] {});
  RunLoopUntilIdle();
  EXPECT_FALSE(exclusive_touch_used);
  EXPECT_FALSE(hit_tested_touch_used);
  EXPECT_FALSE(exclusive_mouse_used);
  EXPECT_TRUE(hit_tested_mouse_used);
}

TEST_P(PointerinjectorRegistryTestP,
       MouseInjectorChannelDying_ShouldTriggerCancelMouseStreamCallback) {
  uint32_t cancel_mouse_stream_count = 0;
  scenic_impl::input::PointerinjectorRegistry registry(
      dispatcher(), snapshot_holder_,
      /*inject_touch_exclusive*/ [](auto...) {},
      /*inject_touch_hit_tested*/ [](auto...) {},
      /*inject_mouse_exclusive*/ [](auto...) {},
      /*inject_mouse_hit_tested*/ [](auto...) {},
      /*cancel_mouse_stream=*/
      [&cancel_mouse_stream_count](auto...) { cancel_mouse_stream_count++; });
  auto [reg_client_end, reg_server_end] =
      fidl::Endpoints<fuchsia_ui_pointerinjector::Registry>::Create();
  registry.Bind(std::move(reg_server_end));
  fidl::Client registry_client(std::move(reg_client_end), dispatcher());

  const auto [parent, child] = SetupSceneWithParentAndChildViews();

  {
    DeviceEventHandler event_handler;
    fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
    bool register_callback_fired = false;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector.Bind(std::move(client_end), dispatcher(), &event_handler);
    fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
    config.device_type(DeviceType::kMouse);
    config.dispatch_policy(DispatchPolicy::kMouseHoverAndLatchInTarget);
    config.buttons(std::vector<uint8_t>{0});
    registry_client->Register({std::move(config), std::move(server_end)})
        .Then([&register_callback_fired](auto& result) {
          if (result.is_ok()) {
            register_callback_fired = true;
          }
        });
    RunLoopUntilIdle();
    EXPECT_TRUE(register_callback_fired);
    EXPECT_FALSE(event_handler.error_fired);
    EXPECT_EQ(cancel_mouse_stream_count, 0u);

    // Begin two streams.
    Inject(injector, EventsTemplate(), [] {});
    {
      auto events = EventsTemplate();
      events.back().data()->pointer_sample()->pointer_id(2);
      Inject(injector, std::move(events), [] {});
    }
    RunLoopUntilIdle();
    EXPECT_EQ(cancel_mouse_stream_count, 0u);
  }  // injector goes out of scope.

  RunLoopUntilIdle();
  // We get a cancel call for the ongoing stream.
  EXPECT_EQ(cancel_mouse_stream_count, 2u);
}

TEST_P(PointerinjectorRegistryTestP,
       MouseInjector_CancelEvent_ShouldTriggerCancelMouseStreamCallback) {
  uint32_t cancel_mouse_stream_count = 0;
  scenic_impl::input::PointerinjectorRegistry registry(
      dispatcher(), snapshot_holder_,
      /*inject_touch_exclusive*/ [](auto...) {},
      /*inject_touch_hit_tested*/ [](auto...) {},
      /*inject_mouse_exclusive*/ [](auto...) {},
      /*inject_mouse_hit_tested*/ [](auto...) {},
      /*cancel_mouse_stream=*/
      [&cancel_mouse_stream_count](auto...) { cancel_mouse_stream_count++; });
  auto [reg_client_end, reg_server_end] =
      fidl::Endpoints<fuchsia_ui_pointerinjector::Registry>::Create();
  registry.Bind(std::move(reg_server_end));
  fidl::Client registry_client(std::move(reg_client_end), dispatcher());

  const auto [parent, child] = SetupSceneWithParentAndChildViews();

  DeviceEventHandler event_handler;
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
  bool register_callback_fired = false;
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  injector.Bind(std::move(client_end), dispatcher(), &event_handler);
  fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
  config.device_type(DeviceType::kMouse);
  config.dispatch_policy(DispatchPolicy::kMouseHoverAndLatchInTarget);
  config.buttons(std::vector<uint8_t>{0});
  registry_client->Register({std::move(config), std::move(server_end)})
      .Then([&register_callback_fired](auto& result) {
        if (result.is_ok()) {
          register_callback_fired = true;
        }
      });
  RunLoopUntilIdle();
  EXPECT_TRUE(register_callback_fired);
  EXPECT_FALSE(event_handler.error_fired);
  EXPECT_EQ(cancel_mouse_stream_count, 0u);

  // Begin a stream.
  Inject(injector, EventsTemplate(), [] {});
  RunLoopUntilIdle();
  EXPECT_EQ(cancel_mouse_stream_count, 0u);

  {
    auto events = EventsTemplate();
    events.back().data()->pointer_sample()->phase(fuchsia_ui_pointerinjector::EventPhase::kCancel);
    Inject(injector, std::move(events), [] {});
    RunLoopUntilIdle();
    EXPECT_EQ(cancel_mouse_stream_count, 1u);
  }

  // Begin another stream.
  Inject(injector, EventsTemplate(), [] {});
  RunLoopUntilIdle();
  EXPECT_EQ(cancel_mouse_stream_count, 1u);

  {
    auto events = EventsTemplate();
    events.back().data()->pointer_sample()->phase(fuchsia_ui_pointerinjector::EventPhase::kRemove);
    Inject(injector, std::move(events), [] {});
    RunLoopUntilIdle();
    EXPECT_EQ(cancel_mouse_stream_count, 2u);
  }
}

// Parameterized tests for malformed viewport arguments.
// Use pairs of optional extents and matrices. Because test parameters must be copyable.
using ViewportPair = std::pair<std::optional<std::array<std::array<float, 2>, 2>>,
                               std::optional<std::array<float, 9>>>;

static std::vector<ViewportPair> BadViewportTestData() {
  std::vector<ViewportPair> bad_viewports;
  {  // 0: No extents.
    ViewportPair pair;
    pair.second.emplace(kIdentityMatrix);
    bad_viewports.emplace_back(pair);
  }
  {  // 1: No viewport_to_context_transform.
    ViewportPair pair;
    pair.first = {{{/*min*/ 0, 0}, /*max*/ {10, 10}}};
    bad_viewports.emplace_back(pair);
  }
  {  // 2: Malformed extents: Min bigger than max.
    ViewportPair pair;
    pair.first = {{{/*min*/ -100, 100}, /*max*/ {100, -100}}};
    pair.second = kIdentityMatrix;
    bad_viewports.emplace_back(pair);
  }
  {  // 3: Malformed extents: Min equal to max.
    ViewportPair pair;
    pair.first = {{{/*min*/ 0, -100}, /*max*/ {0, 100}}};
    pair.second = kIdentityMatrix;
    bad_viewports.emplace_back(pair);
  }
  {  // 4: Malformed extents: Contains NaN
    ViewportPair pair;
    pair.first = {{{/*min*/ 0, 0}, /*max*/ {100, std::numeric_limits<float>::quiet_NaN()}}};
    pair.second = kIdentityMatrix;
    bad_viewports.emplace_back(pair);
  }
  {  // 5: Malformed extents: Contains Inf
    ViewportPair pair;
    pair.first = {{{/*min*/ 0, 0}, /*max*/ {100, std::numeric_limits<float>::infinity()}}};
    pair.second = kIdentityMatrix;
    bad_viewports.emplace_back(pair);
  }
  {  // 6: Malformed transform: Non-invertible matrix
    // clang-format off
    const std::array<float, 9> non_invertible_matrix = {
      1, 0, 0,
      1, 0, 0,
      0, 0, 1,
    };
    // clang-format on
    ViewportPair pair;
    pair.first = {{{/*min*/ 0, 0}, /*max*/ {10, 10}}};
    pair.second = non_invertible_matrix;
    bad_viewports.emplace_back(pair);
  }
  {  // 7: Malformed transform: Contains NaN
    // clang-format off
    const std::array<float, 9> nan_matrix = {
      1, std::numeric_limits<float>::quiet_NaN(), 0,
      0, 1, 0,
      0, 0, 1,
    };
    // clang-format on
    ViewportPair pair;
    pair.first = {{{/*min*/ 0, 0}, /*max*/ {10, 10}}};
    pair.second = nan_matrix;
    bad_viewports.emplace_back(pair);
  }
  {  // 8: Malformed transform: Contains Inf
    // clang-format off
    const std::array<float, 9> inf_matrix = {
      1, std::numeric_limits<float>::infinity(), 0,
      0, 1, 0,
      0, 0, 1,
    };
    // clang-format on
    ViewportPair pair;
    pair.first = {{{/*min*/ 0, 0}, /*max*/ {10, 10}}};
    pair.second = inf_matrix;
    bad_viewports.emplace_back(pair);
  }

  return bad_viewports;
}

class ParameterizedPointerinjectorRegistryTest : public PointerinjectorRegistryTest,
                                                 public testing::WithParamInterface<ViewportPair> {
};

INSTANTIATE_TEST_SUITE_P(RegisterAttemptWithBadViewport_ShouldFail,
                         ParameterizedPointerinjectorRegistryTest,
                         testing::ValuesIn(BadViewportTestData()));

TEST_P(ParameterizedPointerinjectorRegistryTest, RegisterAttemptWithBadViewport_ShouldFail) {
  const auto [parent, child] = SetupSceneWithParentAndChildViews();

  DeviceEventHandler event_handler;
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector;
  bool register_callback_fired = false;
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  injector.Bind(std::move(client_end), dispatcher(), &event_handler);

  fuchsia_ui_pointerinjector::Config config = ConfigTemplate(parent.view_ref, child.view_ref);
  {
    ViewportPair params = GetParam();
    fuchsia_ui_pointerinjector::Viewport viewport;
    if (params.first)
      viewport.extents(params.first.value());
    if (params.second)
      viewport.viewport_to_context_transform(params.second.value());
    config.viewport(std::move(viewport));
  }

  registry_client_->Register({std::move(config), std::move(server_end)})
      .Then([&register_callback_fired](auto& result) {
        if (result.is_ok()) {
          register_callback_fired = true;
        }
      });

  RunLoopUntilIdle();

  EXPECT_TRUE(register_callback_fired);
  EXPECT_TRUE(event_handler.error_fired);
}

}  // namespace input::test
