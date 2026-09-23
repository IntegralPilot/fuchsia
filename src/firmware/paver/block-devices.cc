// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/block-devices.h"

#include <dirent.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/watcher.h>
#include <lib/fit/defer.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <fbl/unique_fd.h>

#include "src/firmware/paver/pave-logging.h"

namespace paver {

namespace {

zx::result<fbl::unique_fd> open_in_dir(int dir_fd, const std::string& filename) {
  zx::channel partition_local, partition_remote;
  if (zx_status_t status = zx::channel::create(0, &partition_local, &partition_remote);
      status != ZX_OK) {
    return zx::error(status);
  }
  fbl::unique_fd fd;
  if (zx_status_t status = fdio_open3_fd_at(dir_fd, filename.c_str(),
                                            static_cast<uint64_t>(fuchsia_io::kPermReadable),
                                            fd.reset_and_get_address());
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(fd));
}

}  // namespace

DirBasedVolumeConnector::DirBasedVolumeConnector(fbl::unique_fd dir,
                                                 std::string volume_connector_path)
    : dir_(std::move(dir)), volume_connector_path_(std::move(volume_connector_path)) {}

zx::result<fidl::ClientEnd<fuchsia_storage_block::Block>> DirBasedVolumeConnector::Connect() const {
  fdio_cpp::UnownedFdioCaller caller(dir_);
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_storage_block::Block>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }
  auto [client, server] = std::move(*endpoints);
  if (zx_status_t status = fdio_service_connect_at(
          caller.borrow_channel(), volume_connector_path_.c_str(), server.TakeChannel().release());
      status != ZX_OK) {
    LOG("Failed to connect to volume service: %s\n", zx_status_get_string(status));
    return zx::error(status);
  }
  return zx::ok(std::move(client));
}

zx::result<fidl::ClientEnd<fuchsia_storage_partitions::Partition>>
DirBasedVolumeConnector::PartitionManagement() const {
  ZX_ASSERT_MSG(false, "Called PartitionManagement on a VolumeServiceBasedVolumeConnector");
}

PartitionServiceBasedVolumeConnector::PartitionServiceBasedVolumeConnector(
    fbl::unique_fd service_dir)
    : DirBasedVolumeConnector(std::move(service_dir), std::string("volume")) {}

zx::result<fidl::ClientEnd<fuchsia_storage_partitions::Partition>>
PartitionServiceBasedVolumeConnector::PartitionManagement() const {
  fdio_cpp::UnownedFdioCaller caller(dir_);
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_storage_partitions::Partition>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }
  auto [client, server] = std::move(*endpoints);
  if (zx_status_t status = fdio_service_connect_at(caller.borrow_channel(), "partition",
                                                   server.TakeChannel().release());
      status != ZX_OK) {
    LOG("Failed to connect to partition service: %s\n", zx_status_get_string(status));
    return zx::error(status);
  }
  return zx::ok(std::move(client));
}

BlockDevices::BlockDevices(fbl::unique_fd root, Variant variant)
    : root_(std::move(root)), variant_(variant) {}

zx::result<BlockDevices> BlockDevices::CreateFromFshostBlockDir(fbl::unique_fd block_dir) {
  if (!block_dir) {
    if (zx_status_t status =
            fdio_open3_fd("/block", static_cast<uint64_t>(fuchsia_io::kPermReadable),
                          block_dir.reset_and_get_address());
        status != ZX_OK) {
      ERROR("Failed to open /block: %s\n", zx_status_get_string(status));
      return zx::error(status);
    }
  }
  return zx::ok(BlockDevices(std::move(block_dir), Variant::kFshostBlockDir));
}

zx::result<BlockDevices> BlockDevices::CreateFromPartitionService(
    fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root) {
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }
  auto [client, server] = std::move(*endpoints);
  if (zx_status_t status = fdio_open3_at(
          svc_root.handle()->get(), "fuchsia.storage.partitions.PartitionService",
          static_cast<uint64_t>(fuchsia_io::wire::kPermReadable), server.TakeChannel().release());
      status != ZX_OK) {
    ERROR("Failed to open partition service: %s\n", zx_status_get_string(status));
    return zx::error(status);
  }
  fbl::unique_fd partition_dir;
  if (zx_status_t status =
          fdio_fd_create(client.TakeChannel().release(), partition_dir.reset_and_get_address());
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(BlockDevices(std::move(partition_dir), Variant::kPartitionService));
}

zx::result<BlockDevices> BlockDevices::CreateFromSkipBlockService(
    fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root) {
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }
  auto [client, server] = std::move(*endpoints);
  if (zx_status_t status = fdio_open3_at(
          svc_root.handle()->get(), "fuchsia.hardware.skipblock.Service",
          static_cast<uint64_t>(fuchsia_io::wire::kPermReadable), server.TakeChannel().release());
      status != ZX_OK) {
    ERROR("Failed to open skip block service: %s\n", zx_status_get_string(status));
    return zx::error(status);
  }
  fbl::unique_fd skip_block_dir;
  if (zx_status_t status =
          fdio_fd_create(client.TakeChannel().release(), skip_block_dir.reset_and_get_address());
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(BlockDevices(std::move(skip_block_dir), Variant::kSkipBlockService));
}

BlockDevices BlockDevices::CreateEmpty() { return BlockDevices({}, {}); }

BlockDevices BlockDevices::Duplicate() const { return BlockDevices(root_.duplicate(), variant_); }

zx::result<std::vector<std::unique_ptr<VolumeConnector>>> BlockDevices::OpenAllPartitions(
    fit::function<bool(const zx::channel&)> filter) const {
  return OpenAllPartitionsInner(std::move(filter), /*limit=*/std::numeric_limits<size_t>::max());
}

zx::result<std::unique_ptr<VolumeConnector>> BlockDevices::OpenPartition(
    fit::function<bool(const zx::channel&)> filter) const {
  zx::result results = OpenAllPartitionsInner(std::move(filter), /*limit=*/1);
  if (results.is_error()) {
    return results.take_error();
  }
  if (results->empty()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  return zx::ok(std::move(results->front()));
}

zx::result<std::vector<std::unique_ptr<VolumeConnector>>> BlockDevices::OpenAllPartitionsInner(
    fit::function<bool(const zx::channel&)> filter, size_t limit) const {
  if (!root_) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  const char* path = ".";
  int parent_fd = root_.get();
  fbl::unique_fd dir_fd;
  if (zx_status_t status =
          fdio_open3_fd_at(parent_fd, path, static_cast<uint64_t>(fuchsia_io::kPermReadable),
                           dir_fd.reset_and_get_address());
      status != ZX_OK) {
    return zx::error(status);
  }

  DIR* dir = fdopendir(dir_fd.duplicate().release());
  if (dir == nullptr) {
    ERROR("Cannot inspect block devices: %s\n", strerror(errno));
    return zx::error(ZX_ERR_INTERNAL);
  }
  const auto closer = fit::defer([dir]() { closedir(dir); });

  std::vector<std::unique_ptr<VolumeConnector>> results;
  struct dirent* de;
  while (results.size() < limit && (de = readdir(dir)) != nullptr) {
    if (std::string_view{de->d_name} == ".") {
      continue;
    }
    std::string filename(de->d_name, strnlen(de->d_name, sizeof(de->d_name)));
    std::unique_ptr<VolumeConnector> connector;
    switch (variant_) {
      case Variant::kFshostBlockDir: {
        zx::result fd = open_in_dir(dir_fd.get(), filename);
        if (fd.is_error()) {
          return fd.take_error();
        }
        connector = std::make_unique<DirBasedVolumeConnector>(
            *std::move(fd), std::string("fuchsia.storage.block.Block"));
        break;
      }
      case Variant::kPartitionService: {
        zx::result fd = open_in_dir(dir_fd.get(), filename);
        if (fd.is_error()) {
          return fd.take_error();
        }
        connector = std::make_unique<PartitionServiceBasedVolumeConnector>(*std::move(fd));
        break;
      }
      case Variant::kSkipBlockService: {
        zx::result fd = open_in_dir(dir_fd.get(), filename);
        if (fd.is_error()) {
          return fd.take_error();
        }
        connector =
            std::make_unique<DirBasedVolumeConnector>(*std::move(fd), std::string("skipblock"));
        break;
      }
    }
    zx::result partition = connector->Connect();
    if (partition.is_error()) {
      return partition.take_error();
    }
    if (filter(partition->channel())) {
      results.push_back(std::move(connector));
    }
  }
  return zx::ok(std::move(results));
}

zx::result<std::unique_ptr<VolumeConnector>> BlockDevices::WaitForPartition(
    fit::function<bool(const zx::channel&)> filter, zx_duration_t timeout) const {
  if (!root_) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  struct CallbackInfo {
    std::unique_ptr<VolumeConnector> out_partition;
    fit::function<bool(const zx::channel&)> filter;
    Variant variant;
  };

  CallbackInfo info = {
      .out_partition = {},
      .filter = std::move(filter),
      .variant = variant_,
  };

  auto cb = [](int dirfd, int event, const char* filename, void* cookie) {
    if (event != WATCH_EVENT_ADD_FILE) {
      return ZX_OK;
    }
    if ((strcmp(filename, ".") == 0) || strcmp(filename, "..") == 0) {
      return ZX_OK;
    }
    auto info = static_cast<CallbackInfo*>(cookie);
    std::unique_ptr<VolumeConnector> connector;
    switch (info->variant) {
      case Variant::kFshostBlockDir: {
        zx::result fd = open_in_dir(dirfd, filename);
        if (fd.is_error()) {
          return fd.status_value();
        }
        connector = std::make_unique<DirBasedVolumeConnector>(
            *std::move(fd), std::string("fuchsia.storage.block.Block"));
        break;
      }
      case Variant::kPartitionService: {
        zx::result fd = open_in_dir(dirfd, filename);
        if (fd.is_error()) {
          return fd.status_value();
        }
        connector = std::make_unique<PartitionServiceBasedVolumeConnector>(*std::move(fd));
        break;
      }
      case Variant::kSkipBlockService: {
        zx::result fd = open_in_dir(dirfd, filename);
        if (fd.is_error()) {
          return fd.status_value();
        }
        connector =
            std::make_unique<DirBasedVolumeConnector>(*std::move(fd), std::string("skipblock"));
        break;
      }
    }
    zx::result volume = connector->Connect();
    if (volume.is_error()) {
      return volume.status_value();
    }
    if (!info->filter(volume->channel())) {
      // ZX_OK means keep going
      return ZX_OK;
    }

    info->out_partition = std::move(connector);
    return ZX_ERR_STOP;
  };

  zx_time_t deadline = zx_deadline_after(timeout);
  if (zx_status_t status = fdio_watch_directory(root_.get(), cb, deadline, &info);
      status != ZX_ERR_STOP) {
    return zx::error(status);
  }
  return zx::ok(std::move(info.out_partition));
}

}  // namespace paver
