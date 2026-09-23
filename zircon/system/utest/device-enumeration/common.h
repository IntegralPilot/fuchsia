// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_UTEST_DEVICE_ENUMERATION_COMMON_H_
#define ZIRCON_SYSTEM_UTEST_DEVICE_ENUMERATION_COMMON_H_

#include <fidl/fuchsia.driver.development/cpp/fidl.h>

#include <span>
#include <string>
#include <type_traits>
#include <unordered_map>
#include <vector>

#include <zxtest/zxtest.h>

namespace device_enumeration {

void WaitForClassDeviceCount(const std::string& path_in_devfs, size_t count);

}  // namespace device_enumeration

class DeviceEnumerationTest : public zxtest::Test {
 public:
  void SetUp() override {
    if (!skip_node_retrieval_) {
      ASSERT_NO_FATAL_FAILURE(RetrieveNodeInfo());
    }
  }

  // Helper to combine collections and string types for cleaner test organization.
  template <typename... Args>
  static std::vector<const char*> Combine(const Args&... args) {
    std::vector<const char*> result;
    auto append = [&result](const auto& arg) {
      if constexpr (std::is_convertible_v<decltype(arg), const char*>) {
        result.push_back(arg);
      } else {
        result.append_range(arg);
      }
    };
    (append(args), ...);
    return result;
  }

 protected:
  struct Requirement {
    enum class Type { kAllOf, kOneOf, kNode };
    Type type;
    std::string node;
    std::vector<Requirement> children;
  };

  static Requirement AllOf(std::span<const char* const> node_monikers);
  static Requirement OneOf(std::span<const char* const> node_monikers);
  static Requirement AllOf(std::vector<Requirement> children);
  static Requirement OneOf(std::vector<Requirement> children);

  void Verify(const Requirement& requirement, bool fail_on_unexpected_nodes = false);
  void VerifyNodes(std::span<const char* const> node_monikers,
                   bool fail_on_unexpected_nodes = false);
  void VerifyOneOf(std::span<const char* const> node_monikers);
  bool HasNode(const std::string& node) const { return node_info_.contains(node); }

  void SetNodeMonikers(std::vector<std::string> monikers) {
    node_info_.clear();
    for (auto& m : monikers) {
      fuchsia_driver_development::NodeInfo info{};
      info.moniker() = m;
      node_info_.emplace(std::move(m), std::move(info));
    }
  }

  void SetSkipNodeRetrieval(bool skip) { skip_node_retrieval_ = skip; }

  struct MatchResult {
    std::vector<std::string> matched_nodes;
    std::vector<std::string> errors;

    bool is_ok() const { return errors.empty(); }
    bool is_error() const { return !errors.empty(); }
  };

  MatchResult GetMatchedNodes(const Requirement& req) const;

 private:
  void RetrieveNodeInfo();

  bool skip_node_retrieval_ = false;
  std::unordered_map<std::string, fuchsia_driver_development::NodeInfo> node_info_;
};

#endif  // ZIRCON_SYSTEM_UTEST_DEVICE_ENUMERATION_COMMON_H_
