// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/driver/devicetree/manager/manager-test-helper.h"

#include <fcntl.h>
#include <lib/syslog/cpp/macros.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/assert.h>

#include <memory>
#include <sstream>
#include <utility>

namespace fdf_devicetree::testing {

std::vector<uint8_t> LoadTestBlob(const char* name) {
  int fd = open(name, O_RDONLY);
  ZX_ASSERT_MSG(fd >= 0, "Open failed '%s': %s", name, strerror(errno));

  struct stat stat_out;
  ZX_ASSERT_MSG(fstat(fd, &stat_out) >= 0, "fstat failed: %s", strerror(errno));

  std::vector<uint8_t> vec(stat_out.st_size);
  ssize_t bytes_read = read(fd, vec.data(), stat_out.st_size);
  ZX_ASSERT_MSG(bytes_read >= 0, "Read failed: %s", strerror(errno));

  vec.resize(bytes_read);
  return vec;
}

std::string DebugStringifyProperty(
    const fuchsia_driver_framework::NodePropertyKey& key,
    const std::vector<fuchsia_driver_framework::NodePropertyValue>& values) {
  std::stringstream ret;
  ret << "Key=";

  switch (key.Which()) {
    using Tag = fuchsia_driver_framework::NodePropertyKey::Tag;
    case Tag::kIntValue:
      ret << "Int{" << key.int_value().value() << "}";
      break;
    case Tag::kStringValue:
      ret << "Str{" << key.string_value().value() << "}";
      break;
    default:
      ret << "Unknown{" << static_cast<int>(key.Which()) << "}";
      break;
  }

  for (auto& value : values) {
    ret << " Value=";
    switch (value.Which()) {
      using Tag = fuchsia_driver_framework::NodePropertyValue::Tag;
      case Tag::kBoolValue:
        ret << "Bool{" << value.bool_value().value() << "}";
        break;
      case Tag::kEnumValue:
        ret << "Enum{" << value.enum_value().value() << "}";
        break;
      case Tag::kIntValue:
        ret << "Int{" << value.int_value().value() << "}";
        break;
      case Tag::kStringValue:
        ret << "String{" << value.string_value().value() << "}";
        break;
      default:
        ret << "Unknown{" << static_cast<int>(value.Which()) << "}";
        break;
    }
  }

  return ret.str();
}

std::string DebugStringifyProperty(
    const std::string& key,
    const std::vector<fuchsia_driver_framework::NodePropertyValue>& values) {
  std::stringstream ret;
  ret << "Key=" << key;

  for (auto& value : values) {
    ret << " Value=";
    switch (value.Which()) {
      using Tag = fuchsia_driver_framework::NodePropertyValue::Tag;
      case Tag::kBoolValue:
        ret << "Bool{" << value.bool_value().value() << "}";
        break;
      case Tag::kEnumValue:
        ret << "Enum{" << value.enum_value().value() << "}";
        break;
      case Tag::kIntValue:
        ret << "Int{" << value.int_value().value() << "}";
        break;
      case Tag::kStringValue:
        ret << "String{" << value.string_value().value() << "}";
        break;
      default:
        ret << "Unknown{" << static_cast<int>(value.Which()) << "}";
        break;
    }
  }

  return ret.str();
}

bool CheckHasProperties(
    std::vector<fuchsia_driver_framework::NodeProperty> expected,
    const std::vector<::fuchsia_driver_framework::NodeProperty>& node_properties,
    bool allow_additional_properties) {
  bool result = true;
  for (auto& property : node_properties) {
    auto iter = std::find(expected.begin(), expected.end(), property);
    if (iter == expected.end()) {
      if (!allow_additional_properties) {
        FX_LOGS(ERROR) << "Unexpected property: "
                       << DebugStringifyProperty(property.key(), {property.value()});
        result = false;
      }
    } else {
      expected.erase(iter);
    }
  }

  if (!expected.empty()) {
    FX_LOGS(ERROR) << "All expected properties are not present.";
    for (auto& property : expected) {
      FX_LOGS(ERROR) << "Property expected: "
                     << DebugStringifyProperty(property.key(), {property.value()});
    }
    result = false;
  }

  return result;
}

bool CheckHasProperties(
    std::vector<fuchsia_driver_framework::NodeProperty2> expected,
    const std::vector<::fuchsia_driver_framework::NodeProperty2>& node_properties,
    bool allow_additional_properties) {
  bool result = true;
  for (auto& property : node_properties) {
    auto iter = std::find(expected.begin(), expected.end(), property);
    if (iter == expected.end()) {
      if (!allow_additional_properties) {
        FX_LOGS(ERROR) << "Unexpected property: "
                       << DebugStringifyProperty(property.key(), {property.value()});
        result = false;
      }
    } else {
      expected.erase(iter);
    }
  }

  if (!expected.empty()) {
    FX_LOGS(ERROR) << "All expected properties are not present.";
    for (auto& property : expected) {
      FX_LOGS(ERROR) << "Property expected: "
                     << DebugStringifyProperty(property.key(), {property.value()});
    }
    result = false;
  }

  return result;
}

bool CheckHasBindRules(std::vector<fuchsia_driver_framework::BindRule> expected,
                       const std::vector<::fuchsia_driver_framework::BindRule>& node_rules,
                       bool allow_additional_rules) {
  bool result = true;
  for (auto& rule : node_rules) {
    auto iter = std::find(expected.begin(), expected.end(), rule);
    if (iter == expected.end()) {
      if (!allow_additional_rules) {
        FX_LOGS(WARNING) << "Unexpected bind rule: "
                         << DebugStringifyProperty(rule.key(), rule.values());
        result = false;
      }
    } else {
      expected.erase(iter);
    }
  }

  if (!expected.empty()) {
    FX_LOGS(WARNING) << "All expected bind rules are not present.";
    for (auto& rule : expected) {
      FX_LOGS(WARNING) << "Rule expected: " << DebugStringifyProperty(rule.key(), rule.values());
    }
    result = false;
  }

  return result;
}

bool CheckHasBindRules(std::vector<fuchsia_driver_framework::BindRule2> expected,
                       const std::vector<::fuchsia_driver_framework::BindRule2>& node_rules,
                       bool allow_additional_rules) {
  bool result = true;
  for (auto& rule : node_rules) {
    auto iter = std::find(expected.begin(), expected.end(), rule);
    if (iter == expected.end()) {
      if (!allow_additional_rules) {
        FX_LOGS(WARNING) << "Unexpected bind rule: "
                         << DebugStringifyProperty(rule.key(), rule.values());
        result = false;
      }
    } else {
      expected.erase(iter);
    }
  }

  if (!expected.empty()) {
    FX_LOGS(WARNING) << "All expected bind rules are not present.";
    for (auto& rule : expected) {
      FX_LOGS(WARNING) << "Rule expected: " << DebugStringifyProperty(rule.key(), rule.values());
    }
    result = false;
  }

  return result;
}

ManagerTestHelper::ManagerTestHelper(std::unique_ptr<TestPublisher> publisher)
    : publisher_(std::move(publisher)) {}

ManagerTestHelper::~ManagerTestHelper() = default;

zx::result<> ManagerTestHelper::DoPublish(Manager& manager) {
  return manager.PublishDevices(*publisher_);
}

}  // namespace fdf_devicetree::testing
