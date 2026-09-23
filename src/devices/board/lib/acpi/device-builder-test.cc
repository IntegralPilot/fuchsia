// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/board/lib/acpi/device-builder.h"

#include <lib/async-loop/cpp/loop.h>

#include <zxtest/zxtest.h>

#include "src/devices/board/lib/acpi/manager.h"
#include "src/devices/board/lib/acpi/test/mock-acpi.h"
#include "src/devices/board/lib/acpi/test/null-iommu-manager.h"
#include "src/devices/testing/mock-ddk/mock-device.h"
#include "zircon/system/public/zircon/assert.h"

class DeviceBuilderTest : public zxtest::Test {
 public:
  void SetUp() override {
    acpi_.SetDeviceRoot(std::make_unique<acpi::test::Device>("\\"));
    acpi::status acpi_handle = acpi_.GetHandle(nullptr, "\\");
    ASSERT_OK(acpi_handle.status_value());
    acpi_handle_ = acpi_handle.value();
    root_device_builder_.emplace(acpi::DeviceBuilder::MakeRootDevice(acpi_handle_, root_.get()));
    manager_.emplace(&acpi_, &iommu_, root_.get());
  }

 protected:
  async::Loop& device_loop() { return device_loop_; }

  acpi::DeviceBuilder& root_device_builder() {
    ZX_ASSERT(root_device_builder_.has_value());
    return root_device_builder_.value();
  }

  acpi::Manager& manager() {
    ZX_ASSERT(manager_.has_value());
    return manager_.value();
  }

  ACPI_HANDLE acpi_handle() { return acpi_handle_; }

  MockDevice& root() {
    ZX_ASSERT(root_);
    return *root_;
  }

 private:
  async::Loop device_loop_{&kAsyncLoopConfigNeverAttachToThread};
  std::shared_ptr<MockDevice> root_ = MockDevice::FakeRootParent();
  acpi::test::MockAcpi acpi_;
  ACPI_HANDLE acpi_handle_;
  std::optional<acpi::DeviceBuilder> root_device_builder_;
  std::optional<acpi::Manager> manager_;
  NullIommuManager iommu_;
};

extern "C" {
// MockDevice does not cover adding composite devices within a driver.
__EXPORT zx_status_t device_add_composite_spec(zx_device_t* dev, const char* name,
                                               const composite_node_spec_t* spec) {
  return ZX_OK;
}
}

// Build an ACPI device and verify that a drive framework device was created.
TEST_F(DeviceBuilderTest, TestDeviceCreated) {
  const char* kDeviceName = "test-name";
  const uint64_t kDeviceId = 123;

  acpi::DeviceBuilder device_builder{kDeviceName, acpi_handle(), &root_device_builder(), false,
                                     kDeviceId};
  zx::result device = device_builder.Build(&manager(), device_loop().dispatcher());
  ASSERT_OK(device.status_value());

  // Only the non-composite device gets created in tests because MockDdk does not support creating
  // legacy composites or composite node specs.
  ASSERT_EQ(root().child_count(), 1);
  auto non_composite = root().GetLatestChild();
  ASSERT_TRUE(non_composite->GetStringProperties().empty());
  ASSERT_STREQ(non_composite->name(), kDeviceName);
}
