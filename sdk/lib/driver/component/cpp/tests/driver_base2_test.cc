// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/driver/testing/cpp/minimal_compat_environment.h>

#include <gtest/gtest.h>

namespace {

class TestDriver2 : public fdf::DriverBase2 {
 public:
  TestDriver2() : fdf::DriverBase2("test_driver2") { dispatcher_in_constructor_ = dispatcher(); }

  zx::result<> Start(fdf::DriverContext context) override { return zx::ok(); }

  async_dispatcher_t* dispatcher_in_constructor() const { return dispatcher_in_constructor_; }

 private:
  async_dispatcher_t* dispatcher_in_constructor_ = nullptr;
};

TEST(DriverBase2Test, DispatcherInConstructor) {
  fdf_testing::DriverRuntime runtime;

  EXPECT_NE(nullptr, fdf_dispatcher_get_current_dispatcher());

  TestDriver2 driver;
  EXPECT_NE(nullptr, driver.dispatcher_in_constructor());
  EXPECT_EQ(fdf_dispatcher_get_async_dispatcher(fdf_dispatcher_get_current_dispatcher()),
            driver.dispatcher_in_constructor());
}

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
class TestProvideResourceDriver2 : public fdf::DriverBase2 {
 public:
  TestProvideResourceDriver2() : fdf::DriverBase2("test_driver2") {}

  static DriverRegistration GetDriverRegistration() {
    return FUCHSIA_DRIVER_REGISTRATION_V1(
        fdf_internal::DriverServer2<TestProvideResourceDriver2>::initialize,
        fdf_internal::DriverServer2<TestProvideResourceDriver2>::destroy);
  }

  zx::result<> Start(fdf::DriverContext context) override { return zx::ok(); }

  void CreateResourceSync() {
    auto result = ProvideResource("my_resource", {}, {});
    ASSERT_EQ(ZX_OK, result.status_value());
  }
};

struct ProvideResourceFixtureConfig final {
  using DriverType = TestProvideResourceDriver2;
  using EnvironmentType = fdf_testing::MinimalCompatEnvironment;
};

TEST(DriverBase2Test, ProvideResource) {
  fdf_testing::ForegroundDriverTest<ProvideResourceFixtureConfig> driver_test;
  ASSERT_EQ(ZX_OK, driver_test.StartDriver().status_value());
  driver_test.driver()->CreateResourceSync();
  ASSERT_EQ(ZX_OK, driver_test.StopDriver().status_value());
}
#endif

}  // namespace
