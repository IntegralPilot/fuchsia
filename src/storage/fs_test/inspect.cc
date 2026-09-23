// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/diagnostics/reader/cpp/inspect.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <stdio.h>
#include <unistd.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/storage/fs_test/fs_test.h"
#include "src/storage/fs_test/fs_test_fixture.h"
#include "src/storage/lib/vfs/cpp/inspect/inspect_data.h"
#include "src/storage/lib/vfs/cpp/inspect/inspect_tree.h"

namespace fs_test {
namespace {

using diagnostics::reader::InspectData;
using inspect::BoolPropertyValue;
using inspect::IntPropertyValue;
using inspect::StringPropertyValue;
using testing::IsSupersetOf;
using testing::UnorderedElementsAreArray;

// All properties we require the fs.info node to contain, excluding optional fields.
constexpr std::string_view kRequiredInfoProperties[] = {
    fs_inspect::InfoData::kPropId,
    fs_inspect::InfoData::kPropType,
    fs_inspect::InfoData::kPropName,
    fs_inspect::InfoData::kPropVersionMajor,
    fs_inspect::InfoData::kPropVersionMinor,
    fs_inspect::InfoData::kPropBlockSize,
    fs_inspect::InfoData::kPropMaxFilenameLength,
};

// All properties we expect the fs.usage node to contain.
constexpr std::string_view kAllUsageProperties[] = {
    fs_inspect::UsageData::kPropTotalBytes,
    fs_inspect::UsageData::kPropUsedBytes,
    fs_inspect::UsageData::kPropTotalNodes,
    fs_inspect::UsageData::kPropUsedNodes,
};

// All properties we expect the fs.fvm node to contain.
constexpr std::string_view kAllFvmProperties[] = {
    fs_inspect::FvmData::kPropSizeBytes,
    fs_inspect::FvmData::kPropSizeLimitBytes,
    fs_inspect::FvmData::kPropAvailableSpaceBytes,
    fs_inspect::FvmData::kPropOutOfSpaceEvents,
};

// Create a vector of all property names found in the given node.
std::vector<std::string> GetPropertyNames(const inspect::NodeValue& node) {
  std::vector<std::string> properties;
  for (const auto& property : node.properties()) {
    properties.push_back(property.name());
  }
  return properties;
}

// Validates that the snapshot's hierarchy is compliant so that the test case invariants can be
// ensured. Use with ASSERT_NO_FATAL_FAILURE.
void ValidateHierarchy(const inspect::Hierarchy& root, const TestFilesystemOptions& options) {
  // Ensure the expected properties under each node exist so that the invariants the getters above
  // rely on are valid (namely, that these specific nodes and their properties exist).

  // Validate that the required fs.info node properties are present.
  const inspect::Hierarchy* info = root.GetByPath({fs_inspect::kInfoNodeName});
  ASSERT_NE(info, nullptr) << "Could not find node " << fs_inspect::kInfoNodeName;
  EXPECT_THAT(GetPropertyNames(info->node()), IsSupersetOf(kRequiredInfoProperties));

  // Validate fs.usage node properties.
  const inspect::Hierarchy* usage = root.GetByPath({fs_inspect::kUsageNodeName});
  ASSERT_NE(usage, nullptr) << "Could not find node " << fs_inspect::kUsageNodeName;
  EXPECT_THAT(GetPropertyNames(usage->node()), UnorderedElementsAreArray(kAllUsageProperties));

  if (options.use_fvm) {
    // Validate fs.fvm node properties.
    const inspect::Hierarchy* fvm = root.GetByPath({fs_inspect::kFvmNodeName});
    ASSERT_NE(fvm, nullptr) << "Could not find node " << fs_inspect::kFvmNodeName;
    EXPECT_THAT(GetPropertyNames(fvm->node()), UnorderedElementsAreArray(kAllFvmProperties));
  }
}

// Parse the given fs.info node properties into a corresponding InfoData struct.
// Properties within the given node must both exist and be the correct type.
fs_inspect::InfoData GetInfoProperties(const inspect::NodeValue& info_node) {
  using fs_inspect::InfoData;

  // oldest_version is optional.
  std::optional<std::string> oldest_version = std::nullopt;
  if (info_node.get_property<StringPropertyValue>(InfoData::kPropOldestVersion)) {
    oldest_version =
        info_node.get_property<StringPropertyValue>(InfoData::kPropOldestVersion)->value();
  }

  return InfoData{
      .id = static_cast<uint64_t>(
          info_node.get_property<IntPropertyValue>(InfoData::kPropId)->value()),
      .type = static_cast<uint64_t>(
          info_node.get_property<IntPropertyValue>(InfoData::kPropType)->value()),
      .name = info_node.get_property<StringPropertyValue>(InfoData::kPropName)->value(),
      .version_major = static_cast<uint64_t>(
          info_node.get_property<IntPropertyValue>(InfoData::kPropVersionMajor)->value()),
      .version_minor = static_cast<uint64_t>(
          info_node.get_property<IntPropertyValue>(InfoData::kPropVersionMinor)->value()),
      .block_size = static_cast<uint64_t>(
          info_node.get_property<IntPropertyValue>(InfoData::kPropBlockSize)->value()),
      .max_filename_length = static_cast<uint64_t>(
          info_node.get_property<IntPropertyValue>(InfoData::kPropMaxFilenameLength)->value()),
      .oldest_version = std::move(oldest_version),
  };
}

// Parse the given fs.usage node properties into a corresponding UsageData struct.
// Properties within the given node must both exist and be the correct type.
fs_inspect::UsageData GetUsageProperties(const inspect::NodeValue& usage_node) {
  using fs_inspect::UsageData;
  return UsageData{
      .total_bytes = static_cast<uint64_t>(
          usage_node.get_property<IntPropertyValue>(UsageData::kPropTotalBytes)->value()),
      .used_bytes = static_cast<uint64_t>(
          usage_node.get_property<IntPropertyValue>(UsageData::kPropUsedBytes)->value()),
      .total_nodes = static_cast<uint64_t>(
          usage_node.get_property<IntPropertyValue>(UsageData::kPropTotalNodes)->value()),
      .used_nodes = static_cast<uint64_t>(
          usage_node.get_property<IntPropertyValue>(UsageData::kPropUsedNodes)->value()),
  };
}

// Parse the given fs.fvm node properties into a corresponding FvmData struct.
// Properties within the given node must both exist and be the correct type.
fs_inspect::FvmData GetFvmProperties(const inspect::NodeValue& fvm_node) {
  using fs_inspect::FvmData;
  return FvmData{
      .size_info =
          {
              .size_bytes = static_cast<uint64_t>(
                  fvm_node.get_property<IntPropertyValue>(FvmData::kPropSizeBytes)->value()),
              .size_limit_bytes = static_cast<uint64_t>(
                  fvm_node.get_property<IntPropertyValue>(FvmData::kPropSizeLimitBytes)->value()),
              .available_space_bytes = static_cast<uint64_t>(
                  fvm_node.get_property<IntPropertyValue>(FvmData::kPropAvailableSpaceBytes)
                      ->value()),
          },
      .out_of_space_events = static_cast<uint64_t>(
          fvm_node.get_property<IntPropertyValue>(FvmData::kPropOutOfSpaceEvents)->value()),
  };
}

// Parse the given fs.volumes.{name} node properties into a corresponding VolumeData struct.
// Properties within the given node must both exist and be the correct type.
fs_inspect::VolumeData GetVolumeProperties(const inspect::NodeValue& volume_node) {
  using fs_inspect::VolumeData;
  return VolumeData{
      .used_bytes = static_cast<uint64_t>(
          volume_node.get_property<IntPropertyValue>(VolumeData::kPropVolumeUsedBytes)->value()),
      .used_nodes = static_cast<uint64_t>(
          volume_node.get_property<IntPropertyValue>(VolumeData::kPropVolumeUsedNodes)->value()),
      .encrypted =
          volume_node.get_property<BoolPropertyValue>(VolumeData::kPropVolumeEncrypted)->value(),
  };
}

class InspectTest : public FilesystemTest {
 protected:
  // Initializes the test case by taking an initial snapshot of the inspect tree, and validates
  // the overall node hierarchy/layout.
  void SetUp() override {
    // Take an initial snapshot.
    ASSERT_NO_FATAL_FAILURE(UpdateAndValidateSnapshot()) << "Failed in InspectTest::SetUp";
  }

  // Take a new snapshot of the filesystem's inspect tree, and validate the layout for compliance.
  //
  // All calls to this function *must* be wrapped with ASSERT_NO_FATAL_FAILURE. Failure to do so
  // can result in some test fixture methods segfaulting.
  void UpdateAndValidateSnapshot() {
    std::optional<InspectData> data;
    fs().TakeSnapshot(&data);
    ASSERT_TRUE(data.has_value());
    ASSERT_TRUE(data->payload().has_value());
    ASSERT_NO_FATAL_FAILURE(ValidateHierarchy(**data->payload(), fs().options()));
    snapshot_ = std::move(*data);
  }

  // Obtains InfoData containing values from the latest snapshot's fs.info node.
  fs_inspect::InfoData GetInfoData() const {
    EXPECT_NE(nullptr, snapshot_->payload().value());
    auto* n = snapshot_->payload().value()->GetByPath({fs_inspect::kInfoNodeName});
    EXPECT_NE(nullptr, n);
    return GetInfoProperties(n->node());
  }

  // Obtains UsageData containing values from the latest snapshot's fs.usage node.
  fs_inspect::UsageData GetUsageData() const {
    EXPECT_NE(nullptr, snapshot_->payload().value());
    auto* n = snapshot_->payload().value()->GetByPath({fs_inspect::kUsageNodeName});
    EXPECT_NE(nullptr, n);
    return GetUsageProperties(n->node());
  }

  // Obtains FvmData containing values from the latest snapshot's fs.fvm node.
  fs_inspect::FvmData GetFvmData() const {
    EXPECT_NE(nullptr, snapshot_->payload().value());
    auto* n = snapshot_->payload().value()->GetByPath({fs_inspect::kFvmNodeName});
    EXPECT_NE(nullptr, n);
    return GetFvmProperties(n->node());
  }

  // Obtains FvmData containing values from the latest snapshot's fs.volumes.`volume_name` node.
  fs_inspect::VolumeData GetVolumeData(const char* volume_name) const {
    EXPECT_NE(nullptr, snapshot_->payload().value());
    auto* n = snapshot_->payload().value()->GetByPath({fs_inspect::kVolumesNodeName, volume_name});
    EXPECT_NE(nullptr, n);
    return GetVolumeProperties(n->node());
  }

 private:
  // Last snapshot taken of the inspect tree.
  std::optional<InspectData> snapshot_;
};

// Validate values in the fs.info node.
TEST_P(InspectTest, ValidateInfoNode) {
  fs_inspect::InfoData info_data = GetInfoData();
  // The filesystem name (type) should match those in the filesystem traits.
  EXPECT_EQ(info_data.name, fs().GetTraits().name);
  // The filesystem instance identifier should be a valid handle (i.e. non-zero).
  EXPECT_NE(info_data.id, ZX_HANDLE_INVALID);
  // The maximum filename length should be set (i.e. > 0).
  EXPECT_GT(info_data.max_filename_length, 0u);
  // If the filesystem reports oldest_version, ensure it is the correct format (oldest maj/min or
  // maj.min).
  if (info_data.oldest_version.has_value()) {
    EXPECT_THAT(info_data.oldest_version.value(),
                ::testing::MatchesRegex("^[0-9]+[\\/\\.][0-9]+$"));
  }
}

// Validate values in the fs.usage node.
TEST_P(InspectTest, ValidateUsageNode) {
  fs_inspect::UsageData usage_data = GetUsageData();
  EXPECT_LE(usage_data.total_bytes,
            fs().options().device_block_count * fs().options().device_block_size);

  // Multi-volume systems will have further functionality exercised in ValidateVolumeNode (where the
  // bytes/nodes are accounted for).
  if (fs().GetTraits().is_multi_volume) {
    GTEST_SKIP();
  }

  uint64_t orig_used_bytes = usage_data.used_bytes;
  uint64_t orig_used_nodes = usage_data.used_nodes;
  EXPECT_GT(usage_data.total_nodes, 0u);
  EXPECT_GT(usage_data.total_bytes, 0u);

  // Write a file to disk.
  std::string test_filename = GetPath("test_file");
  const size_t kDataWriteSize = 128ul * 1024ul;

  fbl::unique_fd fd(open(test_filename.c_str(), O_CREAT | O_RDWR, 0666));
  ASSERT_TRUE(fd);
  std::vector<uint8_t> data(kDataWriteSize);
  ASSERT_EQ(write(fd.get(), data.data(), data.size()), static_cast<ssize_t>(data.size()));
  ASSERT_EQ(fsync(fd.get()), 0);

  // Take a new inspect snapshot, ensure used_bytes/used_nodes are updated correctly.
  ASSERT_NO_FATAL_FAILURE(UpdateAndValidateSnapshot());
  usage_data = GetUsageData();
  // Used bytes should increase by at least the amount of written data, and we should now use
  // at least one more inode than before.
  EXPECT_GE(usage_data.used_bytes, orig_used_bytes + kDataWriteSize);
  EXPECT_GE(usage_data.used_nodes, orig_used_nodes + 1);
}

// Validate values in the fs.fvm node.
TEST_P(InspectTest, ValidateFvmNode) {
  if (!fs().options().use_fvm) {
    GTEST_SKIP();
  }
  fs_inspect::FvmData fvm_data = GetFvmData();
  EXPECT_EQ(fvm_data.out_of_space_events, 0u);
  uint64_t device_size = fs().options().device_block_count * fs().options().device_block_size;
  uint64_t init_fvm_size = fs().options().fvm_slice_size * fs().options().initial_fvm_slice_count;
  ASSERT_GT(device_size, 0u) << "Invalid block device size!";
  ASSERT_GT(init_fvm_size, 0u) << "Invalid FVM volume size!";

  // The reported volume size should be at least the amount of initial FVM slices, but not exceed
  // the size of the block device.
  EXPECT_GE(fvm_data.size_info.size_bytes, init_fvm_size);
  EXPECT_LT(fvm_data.size_info.size_bytes, device_size);

  // There should be some free space if |size_limit_bytes| is smaller than the device size.
  // Otherwise, the filesystem may utilize all or part of the available slices. However, the amount
  // of free space should not exceed the size of the block device."
  uint64_t min = fvm_data.size_info.size_limit_bytes ? 1 : 0;
  EXPECT_GE(fvm_data.size_info.available_space_bytes, min);
  EXPECT_LT(fvm_data.size_info.available_space_bytes, device_size);

  // We do not set a volume size limit in fs_test currently, so this should always be zero.
  EXPECT_EQ(fvm_data.size_info.size_limit_bytes, 0u);
}

// Validate values in the fs.volumes/{name} nodes.
TEST_P(InspectTest, ValidateVolumeNode) {
  if (!fs().GetTraits().is_multi_volume) {
    GTEST_SKIP();
  }

  fs_inspect::VolumeData volume_data = GetVolumeData("default");
  EXPECT_EQ(volume_data.bytes_limit, std::nullopt);
  EXPECT_TRUE(volume_data.encrypted);
  uint64_t orig_used_bytes = volume_data.used_bytes;
  uint64_t orig_used_nodes = volume_data.used_nodes;

  // Write a file to disk.
  std::string test_filename = GetPath("test_file");
  const size_t kDataWriteSize = 128ul * 1024ul;

  fbl::unique_fd fd(open(test_filename.c_str(), O_CREAT | O_RDWR, 0666));
  ASSERT_TRUE(fd);
  std::vector<uint8_t> data(kDataWriteSize);
  ASSERT_EQ(write(fd.get(), data.data(), data.size()), static_cast<ssize_t>(data.size()));
  ASSERT_EQ(fsync(fd.get()), 0);

  // Take a new inspect snapshot, ensure used_bytes/used_nodes are updated correctly.
  ASSERT_NO_FATAL_FAILURE(UpdateAndValidateSnapshot());

  volume_data = GetVolumeData("default");
  // Used bytes should increase by at least the amount of written data, and we should now use
  // at least one more inode than before.
  EXPECT_GE(volume_data.used_bytes, orig_used_bytes + kDataWriteSize);
  EXPECT_GE(volume_data.used_nodes, orig_used_nodes + 1);
}

std::vector<TestFilesystemOptions> GetTestCombinations() {
  return MapAndFilterAllTestFilesystems(
      [](const TestFilesystemOptions& options) -> std::optional<TestFilesystemOptions> {
        if (options.filesystem->GetTraits().supports_inspect) {
          return options;
        }
        return std::nullopt;
      });
}

INSTANTIATE_TEST_SUITE_P(/*no prefix*/, InspectTest, ::testing::ValuesIn(GetTestCombinations()),
                         ::testing::PrintToStringParamName());

GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(InspectTest);

}  // namespace
}  // namespace fs_test
