// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_MANAGER_MANAGER_TEST_HELPER_H_
#define LIB_DRIVER_DEVICETREE_MANAGER_MANAGER_TEST_HELPER_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/driver/devicetree/manager/manager.h>
#include <lib/driver/devicetree/manager/test-publisher.h>

#include <memory>

namespace fdf_devicetree::testing {

// Load the file |name| into a vector and return it.
std::vector<uint8_t> LoadTestBlob(const char* name);

bool CheckHasProperties(
    std::vector<fuchsia_driver_framework::NodeProperty> expected,
    const std::vector<::fuchsia_driver_framework::NodeProperty>& node_properties,
    bool allow_additional_properties);

bool CheckHasProperties(
    std::vector<fuchsia_driver_framework::NodeProperty2> expected,
    const std::vector<::fuchsia_driver_framework::NodeProperty2>& node_properties,
    bool allow_additional_properties);

bool CheckHasBindRules(std::vector<fuchsia_driver_framework::BindRule> expected,
                       const std::vector<::fuchsia_driver_framework::BindRule>& node_rules,
                       bool allow_additional_rules);

bool CheckHasBindRules(std::vector<fuchsia_driver_framework::BindRule2> expected,
                       const std::vector<::fuchsia_driver_framework::BindRule2>& node_rules,
                       bool allow_additional_rules);

std::string DebugStringifyProperty(
    const fuchsia_driver_framework::NodePropertyKey& key,
    const std::vector<fuchsia_driver_framework::NodePropertyValue>& values);

std::string DebugStringifyProperty(
    const std::string& key, const std::vector<fuchsia_driver_framework::NodePropertyValue>& values);



class ManagerTestHelper {
 public:
  explicit ManagerTestHelper(std::unique_ptr<TestPublisher> publisher);
  ~ManagerTestHelper();

  zx::result<> DoPublish(Manager& manager);

  TestPublisher* publisher() { return publisher_.get(); }

 private:
  std::unique_ptr<TestPublisher> publisher_;
};

}  // namespace fdf_devicetree::testing

#endif  // LIB_DRIVER_DEVICETREE_MANAGER_MANAGER_TEST_HELPER_H_
