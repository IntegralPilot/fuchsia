// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/metadata/metadata-getter-dfv2.h"

#include <lib/component/incoming/cpp/constants.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace display {

namespace {

struct TestMetadata {
  uint32_t vendor_id;
  uint32_t platform_id;
  uint32_t device_id;
};

constexpr uint32_t kTestMetadataType = 'TEST';

constexpr TestMetadata kTestMetadata = {
    .vendor_id = 0x1a'2b'3c'4d,
    .platform_id = 0x5a'6b'7c'8d,
    .device_id = 0x9a'ab'bc'cd,
};

class Dfv2Driver : public fdf::DriverBase2 {
 public:
  static DriverRegistration GetDriverRegistration() {
    return FUCHSIA_DRIVER_REGISTRATION_V1(fdf_internal::DriverServer2<Dfv2Driver>::initialize,
                                          fdf_internal::DriverServer2<Dfv2Driver>::destroy);
  }

  Dfv2Driver() : fdf::DriverBase2("dfv2-driver") {}

  zx::result<> Start(fdf::DriverContext context) override {
    incoming_namespace_ = context.take_incoming();
    return zx::ok();
  }

  std::shared_ptr<fdf::Namespace> incoming_namespace() const { return incoming_namespace_; }

 private:
  std::shared_ptr<fdf::Namespace> incoming_namespace_;
};

class DriverTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    compat_server_.Initialize(component::kDefaultInstance);

    zx_status_t add_metadata_status =
        compat_server_.AddMetadata(kTestMetadataType, &kTestMetadata, sizeof(TestMetadata));
    if (add_metadata_status != ZX_OK) {
      return zx::error(add_metadata_status);
    }

    zx_status_t serve_status =
        compat_server_.Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(), &to_driver_vfs);
    if (serve_status != ZX_OK) {
      return zx::error(serve_status);
    }

    return zx::ok();
  }

 private:
  compat::DeviceServer compat_server_;
};

class TestConfig final {
 public:
  using DriverType = Dfv2Driver;
  using EnvironmentType = DriverTestEnvironment;
};

class MetadataGetterDfv2Test : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> start_result = driver_test().StartDriver();
    ASSERT_OK(start_result.status_value());

    namespace_ = driver_test().driver()->incoming_namespace();
    ASSERT_NE(namespace_, nullptr);
  }

  void TearDown() override {
    zx::result<> stop_result = driver_test().StopDriver();
    ASSERT_OK(stop_result.status_value());
  }

 protected:
  fdf_testing::ForegroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

  std::shared_ptr<fdf::Namespace> namespace_;

 private:
  fdf_testing::ForegroundDriverTest<TestConfig> driver_test_;
};

TEST_F(MetadataGetterDfv2Test, GetMetadata) {
  zx::result<std::unique_ptr<MetadataGetter>> create_metadata_getter_result =
      display::MetadataGetterDfv2::Create(namespace_);
  ASSERT_OK(create_metadata_getter_result.status_value());

  std::unique_ptr<MetadataGetter> metadata_getter =
      std::move(create_metadata_getter_result).value();

  zx::result<std::unique_ptr<TestMetadata>> metadata_result =
      metadata_getter->Get<TestMetadata>(kTestMetadataType, component::kDefaultInstance);
  ASSERT_OK(metadata_result.status_value());

  std::unique_ptr<TestMetadata> metadata = std::move(metadata_result).value();
  ASSERT_NE(metadata, nullptr);

  EXPECT_EQ(metadata->vendor_id, kTestMetadata.vendor_id);
  EXPECT_EQ(metadata->platform_id, kTestMetadata.platform_id);
  EXPECT_EQ(metadata->device_id, kTestMetadata.device_id);
}

TEST_F(MetadataGetterDfv2Test, ErrorOnIncorrectType) {
  zx::result<std::unique_ptr<MetadataGetter>> create_metadata_getter_result =
      display::MetadataGetterDfv2::Create(namespace_);
  ASSERT_OK(create_metadata_getter_result.status_value());

  std::unique_ptr<MetadataGetter> metadata_getter =
      std::move(create_metadata_getter_result).value();

  static constexpr uint32_t kInvalidMetadataType = 'INVA';
  zx::result<std::unique_ptr<TestMetadata>> metadata_result =
      metadata_getter->Get<TestMetadata>(kInvalidMetadataType, component::kDefaultInstance);
  EXPECT_NE(metadata_result.status_value(), ZX_OK);
}

TEST_F(MetadataGetterDfv2Test, ErrorOnIncorrectReturnType) {
  zx::result<std::unique_ptr<MetadataGetter>> create_metadata_getter_result =
      display::MetadataGetterDfv2::Create(namespace_);
  ASSERT_OK(create_metadata_getter_result.status_value());

  std::unique_ptr<MetadataGetter> metadata_getter =
      std::move(create_metadata_getter_result).value();

  struct DifferentFromTestMetadata {
    uint32_t vendor_id;
    uint32_t platform_id;
    uint32_t device_id;
    uint32_t serial_number;
  };
  zx::result<std::unique_ptr<DifferentFromTestMetadata>> metadata_result =
      metadata_getter->Get<DifferentFromTestMetadata>(kTestMetadataType,
                                                      component::kDefaultInstance);
  EXPECT_NE(metadata_result.status_value(), ZX_OK);
}

TEST_F(MetadataGetterDfv2Test, ErrorOnIncorrectInstance) {
  zx::result<std::unique_ptr<MetadataGetter>> create_metadata_getter_result =
      display::MetadataGetterDfv2::Create(namespace_);
  ASSERT_OK(create_metadata_getter_result.status_value());

  std::unique_ptr<MetadataGetter> metadata_getter =
      std::move(create_metadata_getter_result).value();

  zx::result<std::unique_ptr<TestMetadata>> metadata_result =
      metadata_getter->Get<TestMetadata>(kTestMetadataType, "incorrect-instance");
  EXPECT_NE(metadata_result.status_value(), ZX_OK);
}

}  // namespace

}  // namespace display
