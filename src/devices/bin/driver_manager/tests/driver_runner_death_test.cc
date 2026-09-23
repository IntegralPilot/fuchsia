// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/tests/driver_runner_test_fixture.h"

namespace driver_runner {

//   BEGIN DEATH TESTS
//                  _________-----_____
//        _____------           __      ----_
// ___----             ___------              \
//    ----________        ----                 \
//                -----__    |             _____)
//                     __-                /     \
//         _______-----    ___--          \    /)\
//   ------_______      ---____            \__/  /
//                -----__    \ --    _          /\
//                       --__--__     \_____/   \_/\
//                               ----|   /          |
//                                   |  |___________|
//                                   |  | ((_(_)| )_)
//                                   |  \_((_(_)|/(_)
//                                   \             (
//                                    \_____________)
//
// These tests test the allowlist for the fuchsia.device/Controller
// interface.  They first test the interface with an class name that
// is on the allowlist, then with a classname that is not on the allowlist
// to make sure it fails.

const char* kAllowedClassName = "driver_runner_test";
const char* kDisallowedClassName = "Not_on_allowlist";

const char* kAllowedChildName = "node-1";
const char* kBannedChildName = "node-0";

// This type of test creates two children, one with an allowed class name
// and the other without.
class DriverRunnerDeathTest : public DriverRunnerTestBase {
 public:
  void SetUp() override {
    SetupDriverRunner();
    root_driver_ = StartRootDriver();
    ASSERT_EQ(ZX_OK, root_driver_.status_value());
    allowed_child_ =
        root_driver_->driver->AddChild(kAllowedChildName, true, false, kAllowedClassName);
    banned_child_ =
        root_driver_->driver->AddChild(kBannedChildName, true, false, kDisallowedClassName);
    EXPECT_TRUE(RunLoopUntilIdle());
    allowed_controller_ = ConnectToDeviceController(kAllowedChildName);
    banned_controller_ = ConnectToDeviceController(kBannedChildName);
  }

 protected:
  zx::result<StartDriverResult> root_driver_;
  std::shared_ptr<CreatedChild> allowed_child_, banned_child_;
  fidl::WireClient<fuchsia_device::Controller> allowed_controller_, banned_controller_;
};

void TryConnectToController(fidl::WireClient<fuchsia_device::Controller>& controller,
                            async::TestLoop& loop) {
  auto controller_endpoints = fidl::Endpoints<fuchsia_device::Controller>::Create();
  fidl::OneWayStatus result =
      controller->ConnectToController(std::move(controller_endpoints.server));
  ASSERT_TRUE(loop.RunUntilIdle());
  ASSERT_EQ(result.status(), ZX_OK);
}

// Start the root driver, add a child node, and verify that the child node's device controller is
// reachable.
TEST_F(DriverRunnerDeathTest, AllowlistCausesConnectToControllerToFail) {
  TryConnectToController(allowed_controller_, test_loop());

  ASSERT_DEATH(TryConnectToController(banned_controller_, test_loop()),
               "Undeclared DEVFS_USAGE detected");
}

void TryConnectToDeviceFidl(fidl::WireClient<fuchsia_device::Controller>& controller,
                            async::TestLoop& loop) {
  auto controller_endpoints = fidl::Endpoints<fuchsia_device::Controller>::Create();
  fidl::OneWayStatus result =
      controller->ConnectToDeviceFidl(controller_endpoints.server.TakeChannel());
  ASSERT_TRUE(loop.RunUntilIdle());
  ASSERT_EQ(result.status(), ZX_OK);
}

// This just verifies that the call was able to be made and now blocked by the allowlist.  It does
// not check that the device actually connected an interface.
TEST_F(DriverRunnerDeathTest, AllowlistCausesConnectToDeviceFidlToFail) {
  TryConnectToDeviceFidl(allowed_controller_, test_loop());

  ASSERT_DEATH(TryConnectToDeviceFidl(banned_controller_, test_loop()),
               "Undeclared DEVFS_USAGE detected");
}

void TryBind(fidl::WireClient<fuchsia_device::Controller>& controller, async::TestLoop& loop) {
  auto controller_endpoints = fidl::Endpoints<fuchsia_device::Controller>::Create();
  controller->Bind(fidl::StringView::FromExternal(second_driver_url))
      .Then([](fidl::WireUnownedResult<fuchsia_device::Controller::Bind>& reply) {
        ASSERT_EQ(reply.status(), ZX_OK);
      });
  ASSERT_TRUE(loop.RunUntilIdle());
}

TEST_F(DriverRunnerDeathTest, AllowlistCausesBindToFail) {
  PrepareRealmForDriverComponentStart("dev.node-1", second_driver_url);
  driver_index().set_match_callback([](auto args) -> zx::result<FakeDriverIndex::MatchResult> {
    EXPECT_EQ(args.driver_url_suffix().get(), second_driver_url);
    return zx::ok(FakeDriverIndex::MatchResult{
        .url = second_driver_url,
    });
  });

  TryBind(allowed_controller_, test_loop());

  ASSERT_DEATH(TryBind(banned_controller_, test_loop()), "Undeclared DEVFS_USAGE detected");
}

void TryRebind(fidl::WireClient<fuchsia_device::Controller>& controller, async::TestLoop& loop) {
  auto controller_endpoints = fidl::Endpoints<fuchsia_device::Controller>::Create();
  controller->Rebind(fidl::StringView::FromExternal(second_driver_url))
      .Then([](fidl::WireUnownedResult<fuchsia_device::Controller::Rebind>& reply) {
        ASSERT_EQ(reply.status(), ZX_OK);
      });

  ASSERT_TRUE(loop.RunUntilIdle());
}

TEST_F(DriverRunnerDeathTest, AllowlistCausesRebindToFail) {
  PrepareRealmForDriverComponentStart("dev.node-1", second_driver_url);
  driver_index().set_match_callback([](auto args) -> zx::result<FakeDriverIndex::MatchResult> {
    EXPECT_EQ(args.driver_url_suffix().get(), second_driver_url);
    return zx::ok(FakeDriverIndex::MatchResult{
        .url = second_driver_url,
    });
  });

  TryRebind(allowed_controller_, test_loop());

  ASSERT_DEATH(TryRebind(banned_controller_, test_loop()), "Undeclared DEVFS_USAGE detected");
}

void TryScheduleUnbind(fidl::WireClient<fuchsia_device::Controller>& controller,
                       async::TestLoop& loop) {
  auto controller_endpoints = fidl::Endpoints<fuchsia_device::Controller>::Create();
  controller->ScheduleUnbind().Then(
      [](fidl::WireUnownedResult<fuchsia_device::Controller::ScheduleUnbind>& reply) {
        ASSERT_EQ(reply.status(), ZX_OK);
      });
  ASSERT_TRUE(loop.RunUntilIdle());
}

TEST_F(DriverRunnerDeathTest, AllowlistCausesScheduleUnbindToFail) {
  TryScheduleUnbind(allowed_controller_, test_loop());

  ASSERT_DEATH(TryScheduleUnbind(banned_controller_, test_loop()),
               "Undeclared DEVFS_USAGE detected");
}

void TryUnbindChildren(fidl::WireClient<fuchsia_device::Controller>& controller,
                       async::TestLoop& loop) {
  auto controller_endpoints = fidl::Endpoints<fuchsia_device::Controller>::Create();
  controller->UnbindChildren().Then(
      [](fidl::WireUnownedResult<fuchsia_device::Controller::UnbindChildren>& reply) {
        ASSERT_EQ(reply.status(), ZX_OK);
      });
  ASSERT_TRUE(loop.RunUntilIdle());
}

TEST_F(DriverRunnerDeathTest, AllowlistCausesUnbindChildrenToFail) {
  TryUnbindChildren(allowed_controller_, test_loop());

  ASSERT_DEATH(TryUnbindChildren(banned_controller_, test_loop()),
               "Undeclared DEVFS_USAGE detected");
}
}  // namespace driver_runner
