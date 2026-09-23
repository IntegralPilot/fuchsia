// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/shutdown/node_removal_tracker.h"

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

struct NodeBank {
  NodeBank(driver_manager::NodeRemovalTracker *tracker) : tracker_(tracker) {}
  void AddNode(driver_manager::Collection collection, driver_manager::NodeState state) {
    if (state == driver_manager::NodeState::kDestroyed) {
      return;
    }
    ids_.insert(tracker_->RegisterNode(driver_manager::NodeInfo{
        .name = "node",
        .driver_url = "driver",
        .collection = collection,
        .state = state,
    }));
  }

  void NotifyRemovalComplete() {
    for (driver_manager::NodeId id : ids_) {
      tracker_->Notify(id, driver_manager::NodeState::kDestroyed);
    }
  }

  std::set<driver_manager::NodeId> ids_;
  driver_manager::NodeRemovalTracker *tracker_;
};

class NodeRemovalTrackerTest : public gtest::TestLoopFixture {};

TEST_F(NodeRemovalTrackerTest, RegisterOneNode) {
  driver_manager::NodeRemovalTracker tracker(dispatcher());
  driver_manager::NodeId id = tracker.RegisterNode(driver_manager::NodeInfo{
      .name = "node",
      .driver_url = "driver",
      .collection = driver_manager::Collection::kBoot,
      .state = driver_manager::NodeState::kRunning,
  });
  int package_callbacks = 0;
  int all_callbacks = 0;
  tracker.set_pkg_callback([&package_callbacks]() { package_callbacks++; });
  tracker.set_all_callback([&all_callbacks]() { all_callbacks++; });
  tracker.FinishEnumeration();
  tracker.Notify(id, driver_manager::NodeState::kDestroyed);

  EXPECT_EQ(package_callbacks, 1);
  EXPECT_EQ(all_callbacks, 1);
}

TEST_F(NodeRemovalTrackerTest, DoNotRegisterDestroyedNode) {
  driver_manager::NodeRemovalTracker tracker(dispatcher());
  NodeBank node_bank(&tracker);
  node_bank.AddNode(driver_manager::Collection::kBoot, driver_manager::NodeState::kDestroyed);
  int package_callbacks = 0;
  int all_callbacks = 0;
  tracker.set_pkg_callback([&package_callbacks]() { package_callbacks++; });
  tracker.set_all_callback([&all_callbacks]() { all_callbacks++; });

  EXPECT_EQ(package_callbacks, 0);
  EXPECT_EQ(all_callbacks, 0);
  node_bank.NotifyRemovalComplete();

  EXPECT_EQ(package_callbacks, 0);
  EXPECT_EQ(all_callbacks, 0);

  tracker.FinishEnumeration();
  EXPECT_EQ(package_callbacks, 1);
  EXPECT_EQ(all_callbacks, 1);
}

TEST_F(NodeRemovalTrackerTest, RegisterManyNodes) {
  driver_manager::NodeRemovalTracker tracker(dispatcher());
  NodeBank node_bank(&tracker);
  node_bank.AddNode(driver_manager::Collection::kBoot, driver_manager::NodeState::kRunning);
  node_bank.AddNode(driver_manager::Collection::kBoot, driver_manager::NodeState::kRunning);
  node_bank.AddNode(driver_manager::Collection::kPackage, driver_manager::NodeState::kRunning);
  node_bank.AddNode(driver_manager::Collection::kPackage, driver_manager::NodeState::kRunning);
  int package_callbacks = 0;
  int all_callbacks = 0;
  tracker.set_pkg_callback([&package_callbacks]() { package_callbacks++; });
  tracker.set_all_callback([&all_callbacks]() { all_callbacks++; });
  tracker.FinishEnumeration();
  EXPECT_EQ(package_callbacks, 0);
  EXPECT_EQ(all_callbacks, 0);
  node_bank.NotifyRemovalComplete();

  EXPECT_EQ(package_callbacks, 1);
  EXPECT_EQ(all_callbacks, 1);
}

// Make sure package callback is only called when package drivers stop
// and all callback is only called when all drivers stop
TEST_F(NodeRemovalTrackerTest, CallbacksCallOrder) {
  driver_manager::NodeRemovalTracker tracker(dispatcher());
  NodeBank boot_node_bank(&tracker), package_node_bank(&tracker);
  boot_node_bank.AddNode(driver_manager::Collection::kBoot, driver_manager::NodeState::kRunning);
  boot_node_bank.AddNode(driver_manager::Collection::kBoot, driver_manager::NodeState::kRunning);
  package_node_bank.AddNode(driver_manager::Collection::kPackage,
                            driver_manager::NodeState::kRunning);
  package_node_bank.AddNode(driver_manager::Collection::kPackage,
                            driver_manager::NodeState::kRunning);
  int package_callbacks = 0;
  int all_callbacks = 0;
  tracker.set_pkg_callback([&package_callbacks]() { package_callbacks++; });
  tracker.set_all_callback([&all_callbacks]() { all_callbacks++; });
  EXPECT_EQ(package_callbacks, 0);
  EXPECT_EQ(all_callbacks, 0);
  tracker.FinishEnumeration();

  package_node_bank.NotifyRemovalComplete();

  EXPECT_EQ(package_callbacks, 1);
  EXPECT_EQ(all_callbacks, 0);

  boot_node_bank.NotifyRemovalComplete();

  EXPECT_EQ(package_callbacks, 1);
  EXPECT_EQ(all_callbacks, 1);
}

// This tests verifies that set_all_callback can be called
// during the pkg_callback without causing a deadlock.
TEST_F(NodeRemovalTrackerTest, CallbackDeadlock) {
  driver_manager::NodeRemovalTracker tracker(dispatcher());
  driver_manager::NodeId id = tracker.RegisterNode(driver_manager::NodeInfo{
      .name = "node",
      .driver_url = "driver",
      .collection = driver_manager::Collection::kBoot,
      .state = driver_manager::NodeState::kRunning,
  });
  int package_callbacks = 0;
  int all_callbacks = 0;
  tracker.set_pkg_callback([&tracker, &package_callbacks, &all_callbacks]() {
    package_callbacks++;
    tracker.set_all_callback([&all_callbacks]() { all_callbacks++; });
  });
  tracker.FinishEnumeration();
  tracker.Notify(id, driver_manager::NodeState::kDestroyed);

  EXPECT_EQ(package_callbacks, 1);
  EXPECT_EQ(all_callbacks, 1);
}

// Make sure callbacks are not called until FinishEnumeration is called
TEST_F(NodeRemovalTrackerTest, FinishEnumeration) {
  driver_manager::NodeRemovalTracker tracker(dispatcher());
  NodeBank node_bank(&tracker);
  node_bank.AddNode(driver_manager::Collection::kBoot, driver_manager::NodeState::kRunning);
  node_bank.AddNode(driver_manager::Collection::kBoot, driver_manager::NodeState::kRunning);
  node_bank.AddNode(driver_manager::Collection::kPackage, driver_manager::NodeState::kRunning);
  node_bank.AddNode(driver_manager::Collection::kPackage, driver_manager::NodeState::kRunning);
  int package_callbacks = 0;
  int all_callbacks = 0;
  tracker.set_pkg_callback([&package_callbacks]() { package_callbacks++; });
  tracker.set_all_callback([&all_callbacks]() { all_callbacks++; });
  EXPECT_EQ(package_callbacks, 0);
  EXPECT_EQ(all_callbacks, 0);
  node_bank.NotifyRemovalComplete();

  EXPECT_EQ(package_callbacks, 0);
  EXPECT_EQ(all_callbacks, 0);
  tracker.FinishEnumeration();

  EXPECT_EQ(package_callbacks, 1);
  EXPECT_EQ(all_callbacks, 1);
}

TEST_F(NodeRemovalTrackerTest, PkgShutdownTimeoutCanceled) {
  driver_manager::NodeRemovalTracker tracker(dispatcher());
  NodeBank boot_node_bank(&tracker), package_node_bank(&tracker);
  boot_node_bank.AddNode(driver_manager::Collection::kBoot, driver_manager::NodeState::kRunning);
  package_node_bank.AddNode(driver_manager::Collection::kPackage,
                            driver_manager::NodeState::kRunning);

  int package_callbacks = 0;
  tracker.set_pkg_callback([&package_callbacks]() { package_callbacks++; });

  bool timed_out = false;
  tracker.SetOnRemovalTimeoutCallback([&timed_out]() { timed_out = true; });

  tracker.FinishEnumeration();

  // Advance time, but not enough to timeout.
  RunLoopFor(driver_manager::NodeRemovalTracker::kRemovalCheckDelay *
                 driver_manager::NodeRemovalTracker::kMaxRemovalCheckCount -
             zx::nsec(1));
  EXPECT_FALSE(timed_out);

  // Complete package removal.
  package_node_bank.NotifyRemovalComplete();
  EXPECT_EQ(package_callbacks, 1);

  // Advance time past the timeout.
  // If the timeout was canceled, it should not fire.
  RunLoopFor(zx::sec(1));
  EXPECT_FALSE(timed_out);
}
