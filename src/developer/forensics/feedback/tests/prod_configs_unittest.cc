// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <optional>
#include <string>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/config.h"
#include "src/developer/forensics/utils/storage_size.h"
#include "src/lib/files/path.h"

namespace forensics::feedback {
namespace {

using testing::UnorderedElementsAreArray;

class ProdConfigTest : public testing::Test {
 public:
  static std::optional<SnapshotConfig> ReadSnapshotConfig(const std::string& config_filename) {
    return GetSnapshotConfig(files::JoinPath("/pkg/data/snapshot/configs", config_filename));
  }
};

TEST_F(ProdConfigTest, DefaultSnapshot) {
  const std::optional<SnapshotConfig> config = ReadSnapshotConfig("default.json");
  ASSERT_TRUE(config.has_value());

  EXPECT_THAT(config->default_annotations, UnorderedElementsAreArray({
                                               "build.board",
                                               "build.compilation-mode",
                                               "build.latest-commit-date",
                                               "build.product",
                                               "build.version",
                                               "build.version.previous-boot",
                                               "build.platform.utc-backstop",
                                               "build.platform.version",
                                               "build.platform.version.previous-boot",
                                               "build.product.version",
                                               "build.product.version.previous-boot",
                                               "device.board-name",
                                               "device.battery.level",
                                               "device.battery.on-charger",
                                               "device.battery.state",
                                               "device.feedback-id",
                                               "device.num-cpus",
                                               "device.runtime",
                                               "device.total-suspended-time",
                                               "device.uptime",
                                               "device.utc-time",
                                               "hardware.board.name",
                                               "hardware.board.revision",
                                               "hardware.product.language",
                                               "hardware.product.locale-list",
                                               "hardware.product.manufacturer",
                                               "hardware.product.model",
                                               "hardware.product.name",
                                               "hardware.product.regulatory-domain",
                                               "hardware.product.sku",
                                               "system.boot-id.current",
                                               "system.boot-id.previous",
                                               "system.boot-id.timeline",
                                               "system.last-reboot.reason",
                                               "system.last-reboot.runtime",
                                               "system.last-reboot.total-suspended-time",
                                               "system.last-reboot.uptime",
                                               "system.last-shutdown.graceful-action",
                                               "system.locale.primary",
                                               "system.timezone.primary",
                                               "system.update-channel.current",
                                               "system.update-channel.target",
                                               "system.user-activity.current.state",
                                               "system.user-activity.current.duration",
                                           }));

  EXPECT_THAT(config->attachment_allowlist, UnorderedElementsAreArray({
                                                "build.kernel-boot-options.txt",
                                                "inspect.json",
                                                "inspect.previous_boot.json",
                                                "log.kernel.previous_boot.txt",
                                                "log.kernel.txt",
                                                "log.system.previous_boot.txt",
                                                "log.system.txt",
                                                "process_tree.txt",
                                            }));
}

}  // namespace
}  // namespace forensics::feedback
