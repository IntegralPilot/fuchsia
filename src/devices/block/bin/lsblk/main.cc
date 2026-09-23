// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.skipblock/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <inttypes.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fit/defer.h>
#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/zx/vmo.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <zircon/process.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gpt/c/gpt.h>
#include <gpt/guid.h>
#include <pretty/hexdump.h>

#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace fuchsia_block = fuchsia_storage_block;
namespace fuchsia_skipblock = fuchsia_hardware_skipblock;

namespace {

constexpr char DEV_BLOCK[] = "/block";
constexpr char DEV_SKIP_BLOCK[] = "/dev/class/skip-block";

char* size_to_cstring(char* str, size_t maxlen, uint64_t size) {
  constexpr size_t kibi = 1024;
  const char* unit;
  uint64_t div;
  if (size < kibi) {
    unit = "";
    div = 1;
  } else if (size >= kibi && size < kibi * kibi) {
    unit = "K";
    div = kibi;
  } else if (size >= kibi * kibi && size < kibi * kibi * kibi) {
    unit = "M";
    div = kibi * kibi;
  } else if (size >= kibi * kibi * kibi && size < kibi * kibi * kibi * kibi) {
    unit = "G";
    div = kibi * kibi * kibi;
  } else {
    unit = "T";
    div = kibi * kibi * kibi * kibi;
  }
  snprintf(str, maxlen, "%" PRIu64 "%s", size / div, unit);
  return str;
}

int cmd_list_blk() {
  struct dirent* de;
  DIR* dir = opendir(DEV_BLOCK);
  if (!dir) {
    fprintf(stderr, "Error opening %s\n", DEV_BLOCK);
    return -1;
  }
  auto cleanup = fit::defer([&dir]() { closedir(dir); });

  printf("%-3s %-4s %-16s %-20s %-6s %s\n", "ID", "SIZE", "TYPE", "LABEL", "FLAGS", "DEVICE");

  while ((de = readdir(dir)) != nullptr) {
    if (!strcmp(de->d_name, ".") || !strcmp(de->d_name, "..")) {
      continue;
    }
    std::string device_path = fxl::StringPrintf("%s/%s", DEV_BLOCK, de->d_name);
    std::string volume_path = device_path + "/fuchsia.storage.block.Block";
    std::string source_path = device_path + "/source";

    std::string source;
    files::ReadFileToString(source_path, &source);

    zx::result device = component::Connect<fuchsia_block::Block>(volume_path);
    if (device.is_error()) {
      fprintf(stderr, "Error opening %s: %s\n", device_path.c_str(), device.status_string());
      continue;
    }

    char sizestr[6] = {};
    fuchsia_block::wire::BlockInfo block_info;
    {
      if (const fidl::WireResult result = fidl::WireCall(device.value())->GetInfo(); result.ok()) {
        if (const fit::result response = result.value(); response.is_ok()) {
          block_info = response.value()->info;
          size_to_cstring(sizestr, sizeof(sizestr), block_info.block_size * block_info.block_count);
        }
      }
    }

    std::string type;
    std::string label;
    {
      if (const fidl::WireResult result = fidl::WireCall(device.value())->GetTypeGuid();
          result.ok()) {
        if (const fidl::WireResponse response = result.value(); response.status == ZX_OK) {
          type = gpt::KnownGuid::TypeDescription(response.guid->value.data());
        }
      }
      if (const fidl::WireResult result = fidl::WireCall(device.value())->GetName(); result.ok()) {
        if (const fidl::WireResponse response = result.value(); response.status == ZX_OK) {
          label = response.name.get();
        }
      }
    }

    char flags[20] = {0};
    if (block_info.flags & fuchsia_block::wire::DeviceFlag::kReadonly) {
      strlcat(flags, "RO ", sizeof(flags));
    }
    if (block_info.flags & fuchsia_block::wire::DeviceFlag::kRemovable) {
      strlcat(flags, "RE ", sizeof(flags));
    }
    printf("%-3s %4s %-16s %-20s %-6s %s\n", de->d_name, sizestr, type.c_str(), label.c_str(),
           flags, source.c_str());
  }
  return 0;
}

int cmd_list_skip_blk() {
  struct dirent* de;
  DIR* dir = opendir(DEV_SKIP_BLOCK);
  if (!dir) {
    fprintf(stderr, "Error opening %s\n", DEV_SKIP_BLOCK);
    return -1;
  }
  while ((de = readdir(dir)) != nullptr) {
    if (!strcmp(de->d_name, ".") || !strcmp(de->d_name, "..")) {
      continue;
    }
    std::string device_path = fxl::StringPrintf("%s/%s", DEV_SKIP_BLOCK, de->d_name);
    std::string controller_path = device_path + "/device_controller";

    std::string topological_path;
    {
      zx::result controller = component::Connect<fuchsia_device::Controller>(controller_path);
      if (controller.is_error()) {
        fprintf(stderr, "Error opening %s: %s\n", controller_path.c_str(),
                controller.status_string());
        continue;
      }
      fidl::WireResult result = fidl::WireCall(controller.value())->GetTopologicalPath();
      if (!result.ok()) {
        fprintf(stderr, "Error getting topological path for %s: %s\n", controller_path.c_str(),
                result.status_string());
        continue;
      }
      fit::result response = result.value();
      if (response.is_error()) {
        fprintf(stderr, "Error getting topological path for %s: %s\n", controller_path.c_str(),
                zx_status_get_string(response.error_value()));
        continue;
      }
      topological_path = response.value()->path.get();
    }

    std::string type;
    {
      zx::result device = component::Connect<fuchsia_skipblock::SkipBlock>(device_path);
      if (device.is_error()) {
        fprintf(stderr, "Error opening %s: %s\n", device_path.c_str(), device.status_string());
        continue;
      }
      if (const fidl::WireResult result = fidl::WireCall(device.value())->GetPartitionInfo();
          result.ok()) {
        if (const fidl::WireResponse response = result.value(); response.status == ZX_OK) {
          type = gpt::KnownGuid::TypeDescription(response.partition_info.partition_guid.data());
        }
      }
    }

    printf("%-3s %4sr%-16s %-20s %-6s %s\n", de->d_name, "", type.c_str(), "", "",
           topological_path.c_str());
  }
  closedir(dir);
  return 0;
}

}  // namespace

int main(int argc, const char** argv) {
  if (argc > 1) {
    goto usage;
  }
  return cmd_list_blk() || cmd_list_skip_blk();
usage:
  fprintf(stderr, "Usage:\n");
  fprintf(stderr, "%s\n", argv[0]);
  return 0;
}
