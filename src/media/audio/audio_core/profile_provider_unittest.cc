// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/profile_provider.h"

#include <lib/fidl/cpp/binding.h>
#include <lib/sys/cpp/testing/component_context_provider.h>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/media/audio/audio_core/mix_profile_config.h"
#include "src/media/audio/audio_core/testing/fake_role_manager.h"

namespace media::audio {

class ProfileProviderTestWithoutRoleManager : public gtest::TestLoopFixture {
 protected:
  void SetUp() override {
    TestLoopFixture::SetUp();
    profile_provider_ = std::make_unique<ProfileProvider>(*context(), mix_profile_config_);
  }

  sys::ComponentContext* context() { return context_provider_.context(); }
  MixProfileConfig mix_profile_config_;
  std::unique_ptr<ProfileProvider> profile_provider_;
  sys::testing::ComponentContextProvider context_provider_;
};

TEST_F(ProfileProviderTestWithoutRoleManager, CallRegisterHandlerWithoutRoleManager) {
  bool called = false;
  zx::thread self;
  ASSERT_EQ(zx::thread::self()->duplicate(ZX_RIGHT_SAME_RIGHTS, &self), ZX_OK);
  profile_provider_->RegisterHandlerWithCapacity(
      std::move(self), "test", zx::msec(1).to_nsecs(), 0.125, [&called](uint64_t, uint64_t) {
        called = true;
        FAIL() << "Unexpected callback without ProfileProvider";
      });
  RunLoopUntilIdle();
  EXPECT_FALSE(called);
}

class ProfileProviderTest : public ProfileProviderTestWithoutRoleManager {
  void SetUp() override {
    ProfileProviderTestWithoutRoleManager::SetUp();

    auto svc = context_provider_.service_directory_provider();
    ASSERT_EQ(ZX_OK, svc->AddService(fake_role_manager_.GetHandler()));
  }

 protected:
  FakeRoleManager fake_role_manager_;
};

TEST_F(ProfileProviderTest, CallRegisterHandlerWithCapacity) {
  bool called = false;
  zx::thread self;
  ASSERT_EQ(zx::thread::self()->duplicate(ZX_RIGHT_SAME_RIGHTS, &self), ZX_OK);
  // Request 25% CPU with a 1 ms period, so 250us every 1ms.
  profile_provider_->RegisterHandlerWithCapacity(
      std::move(self), "test", zx::msec(1).to_nsecs(), 0.25,
      [&called](uint64_t period, uint64_t capacity) {
        ASSERT_EQ(period, static_cast<uint64_t>(zx::msec(1).get()));
        ASSERT_EQ(capacity, static_cast<uint64_t>(zx::usec(250).get()));
        called = true;
      });
  RunLoopUntilIdle();
  EXPECT_TRUE(called);
}

TEST_F(ProfileProviderTest, CallRegisterHandlerWithCapacityDefaultPeriod) {
  bool called = false;
  zx::thread self;
  ASSERT_EQ(zx::thread::self()->duplicate(ZX_RIGHT_SAME_RIGHTS, &self), ZX_OK);
  // Request 25% CPU with a default period, so 2,500us every `mix_profile_config_.period`.
  profile_provider_->RegisterHandlerWithCapacity(
      std::move(self), "test", 0, 0.25, [this, &called](uint64_t period, uint64_t capacity) {
        ASSERT_EQ(period, static_cast<uint64_t>(mix_profile_config_.period.get()));
        ASSERT_EQ(capacity, static_cast<uint64_t>(zx::usec(2500).get()));
        called = true;
      });
  RunLoopUntilIdle();
  EXPECT_TRUE(called);
}

TEST_F(ProfileProviderTest, CallUnregisterHandler) {
  bool called = false;
  zx::thread self;
  ASSERT_EQ(zx::thread::self()->duplicate(ZX_RIGHT_SAME_RIGHTS, &self), ZX_OK);
  profile_provider_->UnregisterHandler(std::move(self), "test", [&called]() { called = true; });
  RunLoopUntilIdle();
  EXPECT_TRUE(called);
}

}  // namespace media::audio
