// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <fuchsia/hardware/block/driver/c/banjo.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <lib/fit/defer.h>
#include <stdlib.h>
#include <unistd.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <unordered_map>

#include <fbl/string.h>
#include <fbl/unique_fd.h>
#include <gpt/c/gpt.h>
#include <gpt/guid.h>
#include <zxtest/base/log-sink.h>
#include <zxtest/zxtest.h>

#include "src/lib/fsl/io/device_watcher.h"

namespace fuchsia_partition = fuchsia_storage_block;

using device_watcher::RecursiveWaitForFile;

#define DEV_BLOCK "/dev/class/block"

class PartitionMappingTest : public zxtest::Test {
 protected:
  static void ScanBlockAndValidateMapping(
      const std::unordered_map<std::string, std::string>& mapping) {
    fbl::unique_fd devfs_root(open(DEV_BLOCK, O_RDONLY));
    ASSERT_TRUE(devfs_root);

    struct dirent* de;
    DIR* dir = opendir(DEV_BLOCK);
    ASSERT_NOT_NULL(dir);
    auto cleanup = fit::defer([&dir]() { closedir(dir); });

    while ((de = readdir(dir)) != nullptr) {
      if (!strcmp(de->d_name, ".") || !strcmp(de->d_name, "..")) {
        continue;
      }

      zx::result channel = RecursiveWaitForFile(devfs_root.get(), de->d_name);
      EXPECT_OK(channel.status_value(), "%s/%s", DEV_BLOCK, de->d_name);
      fidl::ClientEnd<fuchsia_partition::Partition> client_end(std::move(channel.value()));

      std::string partition_name = GetLabel(client_end);

      if (mapping.count(partition_name)) {
        std::string expected_type = mapping.at(partition_name);
        EXPECT_EQ(GetType(client_end), expected_type);
      }
    }
  }

  static std::string GetType(const fidl::ClientEnd<fuchsia_partition::Partition>& client_end) {
    auto guid_resp = fidl::WireCall(client_end)->GetTypeGuid();
    if (guid_resp.ok() && guid_resp->status == ZX_OK && guid_resp->guid) {
      return gpt::KnownGuid::TypeDescription(guid_resp->guid->value.data());
    }
    return {};
  }

  static std::string GetLabel(const fidl::ClientEnd<fuchsia_partition::Partition>& client_end) {
    auto name_resp = fidl::WireCall(client_end)->GetName();
    if (name_resp.ok() && name_resp->status == ZX_OK) {
      return std::string(name_resp->name.data(), name_resp->name.size());
    }
    return {};
  }
};

TEST_F(PartitionMappingTest, NelsonPartitionMapping) {
  std::unordered_map<std::string, std::string> mapping = {
      {"misc", "misc"},         {"boot_a", "zircon-a"},     {"boot_b", "zircon-b"},
      {"cache", "zircon-r"},    {"zircon_r", "zircon-r"},   {"vbmeta_a", "vbmeta_a"},
      {"vbmeta_b", "vbmeta_b"}, {"reserved_c", "vbmeta_r"}, {"vbmeta_r", "vbmeta_r"},
      {"fvm", "fuchsia-fvm"},
  };
  ScanBlockAndValidateMapping(mapping);
}
