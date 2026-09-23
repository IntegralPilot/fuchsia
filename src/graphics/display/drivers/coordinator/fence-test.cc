// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/fence.h"

#include <lib/async-testing/test_loop.h>
#include <lib/async/default.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/zx/event.h>
#include <lib/zx/result.h>

#include <vector>

#include <fbl/ref_ptr.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace display_coordinator {

class FakeFenceListener : public FenceListener {
 public:
  FakeFenceListener() = default;

  FakeFenceListener(const FakeFenceListener&) = delete;
  FakeFenceListener& operator=(const FakeFenceListener&) = delete;

  ~FakeFenceListener() override = default;

  // `FenceListener`:
  void OnFenceSignaled(Fence& fence) override { signaled_fences_.push_back(&fence); }

  std::vector<Fence*>& signaled_fences() { return signaled_fences_; }

 private:
  std::vector<Fence*> signaled_fences_;
};

class FenceTest : public testing::Test {
 public:
  void SetUp() override {
    static constexpr display::EventId kEventId(1);

    zx::event event;
    ASSERT_OK(zx::event::create(/*options=*/0, &event));

    fence_ = fbl::AdoptRef(
        new Fence(&fence_listener_, driver_dispatcher_->borrow(), kEventId, std::move(event)));
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;
  fdf_testing::DriverRuntime driver_runtime_;

  fdf::UnownedSynchronizedDispatcher driver_dispatcher_ = driver_runtime_.GetForegroundDispatcher();

  fbl::RefPtr<Fence> fence_;
  FakeFenceListener fence_listener_;
};

TEST_F(FenceTest, WaitOnce) {
  ASSERT_OK(fence_->Wait());

  fence_->Signal();
  driver_runtime_.RunUntilIdle();

  EXPECT_THAT(fence_listener_.signaled_fences(), ::testing::ElementsAre(fence_.get()));
}

TEST_F(FenceTest, WaitWhileWaiting) {
  ASSERT_OK(fence_->Wait());
  ASSERT_OK(fence_->Wait());

  fence_->Signal();
  driver_runtime_.RunUntilIdle();

  EXPECT_THAT(fence_listener_.signaled_fences(), ::testing::ElementsAre(fence_.get()));
}

TEST_F(FenceTest, WaitAfterSignaled) {
  ASSERT_OK(fence_->Wait());

  fence_->Signal();
  driver_runtime_.RunUntilIdle();

  ASSERT_THAT(fence_listener_.signaled_fences(), ::testing::ElementsAre(fence_.get()));
  fence_listener_.signaled_fences().clear();

  ASSERT_OK(fence_->Wait());

  fence_->Signal();
  driver_runtime_.RunUntilIdle();

  EXPECT_THAT(fence_listener_.signaled_fences(), ::testing::ElementsAre(fence_.get()));
}

}  // namespace display_coordinator
