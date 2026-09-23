// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/metadata/metadata-getter-dfv1.h"

#include <lib/component/incoming/cpp/constants.h>

#include <gtest/gtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
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

class MetadataGetterDfv1Test : public testing::Test {
 public:
  MetadataGetterDfv1Test() {
    fake_parent_ = MockDevice::FakeRootParent();
    fake_parent_->SetMetadata(kTestMetadataType, &kTestMetadata, sizeof(TestMetadata));
  }

  MockDevice* fake_parent() const { return fake_parent_.get(); }

 private:
  std::shared_ptr<MockDevice> fake_parent_;
};

TEST_F(MetadataGetterDfv1Test, GetMetadata) {
  zx::result<std::unique_ptr<MetadataGetter>> metadata_getter_result =
      MetadataGetterDfv1::Create(fake_parent());
  ASSERT_OK(metadata_getter_result.status_value());
  std::unique_ptr<MetadataGetter> metadata_getter = std::move(metadata_getter_result).value();

  zx::result<std::unique_ptr<TestMetadata>> metadata_result =
      metadata_getter->Get<TestMetadata>(kTestMetadataType, component::kDefaultInstance);
  ASSERT_OK(metadata_result.status_value());

  std::unique_ptr<TestMetadata> metadata = std::move(metadata_result.value());
  ASSERT_NE(metadata, nullptr);

  EXPECT_EQ(metadata->vendor_id, kTestMetadata.vendor_id);
  EXPECT_EQ(metadata->platform_id, kTestMetadata.platform_id);
  EXPECT_EQ(metadata->device_id, kTestMetadata.device_id);
}

TEST_F(MetadataGetterDfv1Test, ErrorOnIncorrectType) {
  zx::result<std::unique_ptr<MetadataGetter>> metadata_getter_result =
      MetadataGetterDfv1::Create(fake_parent());
  ASSERT_OK(metadata_getter_result.status_value());
  std::unique_ptr<MetadataGetter> metadata_getter = std::move(metadata_getter_result).value();

  static constexpr uint32_t kInvalidMetadataType = 'INVA';
  zx::result<std::unique_ptr<TestMetadata>> metadata_result =
      metadata_getter->Get<TestMetadata>(kInvalidMetadataType, component::kDefaultInstance);
  EXPECT_NE(metadata_result.status_value(), ZX_OK);
}

TEST_F(MetadataGetterDfv1Test, ErrorOnIncorrectReturnType) {
  zx::result<std::unique_ptr<MetadataGetter>> metadata_getter_result =
      MetadataGetterDfv1::Create(fake_parent());
  ASSERT_OK(metadata_getter_result.status_value());
  std::unique_ptr<MetadataGetter> metadata_getter = std::move(metadata_getter_result).value();

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

}  // namespace

}  // namespace display
