// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/fs_management/cpp/fvm.h"

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <lib/fdio/limits.h>
#include <lib/fdio/watcher.h>
#include <lib/fit/defer.h>
#include <lib/stdcompat/string_view.h>
#include <string.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/processargs.h>
#include <zircon/syscalls.h>

#include <algorithm>
#include <memory>
#include <utility>

#include <fbl/string_printf.h>
#include <fbl/unique_fd.h>

#include "fidl/fuchsia.storage.block/cpp/natural_types.h"
#include "src/lib/fxl/strings/concatenate.h"
#include "src/storage/fvm/fvm.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"
#include "src/storage/lib/fs_management/cpp/format.h"
#include "src/storage/lib/fs_management/cpp/fvm_internal.h"

namespace fs_management {

namespace {

constexpr std::string_view kBlockDevPath = "/dev/class/block/";
constexpr std::string_view kBlockDevRelativePath = "class/block/";

zx::result<fidl::ClientEnd<fuchsia_device::Controller>> OpenPartitionImpl(
    fidl::ClientEnd<fuchsia_io::Directory> directory, const PartitionMatcher& matcher, bool wait) {
  auto cb = [&](fidl::UnownedClientEnd<fuchsia_io::Directory> directory, std::string_view name)
      -> std::optional<zx::result<fidl::ClientEnd<fuchsia_device::Controller>>> {
    std::string controller_path = std::string(name) + "/device_controller";
    zx::result channel =
        component::ConnectAt<fuchsia_device::Controller>(directory, controller_path);
    if (channel.is_error()) {
      return channel.take_error();
    }
    zx::result result = PartitionMatches(*channel, matcher);
    if (result.is_error()) {
      return {};
    }
    if (!result.value()) {
      return std::nullopt;
    }
    return zx::ok(std::move(*channel));
  };

  if (wait) {
    zx::result watch_result = device_watcher::WatchDirectoryForItems<
        zx::result<fidl::ClientEnd<fuchsia_device::Controller>>>(
        directory,
        [&directory, cb = std::move(cb)](std::string_view fn)
            -> std::optional<zx::result<fidl::ClientEnd<fuchsia_device::Controller>>> {
          return cb(directory, fn);
        });
    if (watch_result.is_error()) {
      return watch_result.take_error();
    }
    return std::move(*watch_result);
  }

  // TODO(https://fxbug.dev/42075490): Create a C++ wrapper for channel-oriented readdir and use it
  // here.
  int fd;
  if (zx_status_t status = fdio_fd_create(directory.TakeChannel().release(), &fd);
      status != ZX_OK) {
    return zx::error(status);
  }
  DIR* const dir = fdopendir(fd);
  auto cleanup = fit::defer([dir]() { closedir(dir); });
  fdio_cpp::UnownedFdioCaller caller(dirfd(dir));
  while (const dirent* const entry = readdir(dir)) {
    if (std::string_view(entry->d_name) == ".") {
      continue;
    }
    std::optional result = cb(caller.directory(), entry->d_name);
    if (!result.has_value()) {
      continue;
    }
    return std::move(result.value());
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

zx::result<> DestroyPartitionImpl(fidl::UnownedClientEnd<fuchsia_storage_block::Block> volume) {
  const fidl::WireResult result = fidl::WireCall(volume)->Destroy();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  return zx::make_result(result.value().status);
}

}  // namespace

__EXPORT
zx::result<bool> PartitionMatches(fidl::UnownedClientEnd<fuchsia_device::Controller> channel,
                                  const PartitionMatcher& matcher) {
  ZX_ASSERT(!matcher.type_guids.empty() || !matcher.instance_guids.empty() ||
            !matcher.detected_formats.empty() || !matcher.labels.empty() ||
            !matcher.parent_device.empty());

  auto [partition_client_end, partition_server_end] =
      fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  if (const fidl::OneWayStatus result =
          fidl::WireCall(channel)->ConnectToDeviceFidl(partition_server_end.TakeChannel());
      !result.ok()) {
    return zx::error(result.status());
  }

  if (!matcher.type_guids.empty()) {
    const fidl::WireResult result = fidl::WireCall(partition_client_end)->GetTypeGuid();
    if (!result.ok()) {
      return zx::error(result.status());
    }
    const fidl::WireResponse response = result.value();
    if (zx_status_t status = response.status; status != ZX_OK) {
      return zx::error(status);
    }
    if (!std::any_of(matcher.type_guids.cbegin(), matcher.type_guids.cend(),
                     [type_guid = response.guid->value](const uuid::Uuid& match_guid) {
                       return std::equal(type_guid.cbegin(), type_guid.cend(), match_guid.cbegin());
                     })) {
      return zx::ok(false);
    }
  }
  if (!matcher.instance_guids.empty()) {
    const fidl::WireResult result = fidl::WireCall(partition_client_end)->GetInstanceGuid();
    if (!result.ok()) {
      return zx::error(result.status());
    }
    const fidl::WireResponse response = result.value();
    if (zx_status_t status = response.status; status != ZX_OK) {
      return zx::error(status);
    }
    if (!std::any_of(matcher.instance_guids.cbegin(), matcher.instance_guids.cend(),
                     [instance_guid = response.guid->value](const uuid::Uuid& match_guid) {
                       return std::equal(instance_guid.cbegin(), instance_guid.cend(),
                                         match_guid.cbegin());
                     })) {
      return zx::ok(false);
    }
  }
  if (!matcher.labels.empty()) {
    const fidl::WireResult result = fidl::WireCall(partition_client_end)->GetName();
    if (!result.ok()) {
      return zx::error(result.status());
    }
    const fidl::WireResponse response = result.value();
    if (zx_status_t status = response.status; status != ZX_OK) {
      return zx::error(status);
    }

    if (!std::any_of(matcher.labels.cbegin(), matcher.labels.cend(),
                     [part_label = response.name.get()](std::string_view match_label) {
                       return part_label == match_label;
                     })) {
      return zx::ok(false);
    }
  }

  if (!matcher.parent_device.empty() || !matcher.ignore_prefix.empty() ||
      !matcher.ignore_if_path_contains.empty()) {
    const fidl::WireResult result = fidl::WireCall(channel)->GetTopologicalPath();
    if (!result.ok()) {
      return zx::error(result.status());
    }
    const fit::result response = result.value();
    if (response.is_error()) {
      return zx::error(response.error_value());
    }
    std::string_view path = response.value()->path.get();
    if (!matcher.parent_device.empty() && !path.starts_with(matcher.parent_device)) {
      return zx::ok(false);
    }
    if (!matcher.ignore_prefix.empty() && path.starts_with(matcher.ignore_prefix)) {
      return zx::ok(false);
    }
    if (!matcher.ignore_if_path_contains.empty() &&
        cpp23::contains(path, matcher.ignore_if_path_contains)) {
      return zx::ok(false);
    }
  }
  if (!matcher.detected_formats.empty()) {
    // TODO(https://fxbug.dev/42072982): avoid this cast
    const DiskFormat part_format =
        DetectDiskFormat(fidl::UnownedClientEnd<fuchsia_storage_block::Block>(
            partition_client_end.borrow().channel()));
    if (!std::any_of(
            matcher.detected_formats.cbegin(), matcher.detected_formats.cend(),
            [part_format](const DiskFormat match_format) { return part_format == match_format; })) {
      return zx::ok(false);
    }
  }
  return zx::ok(true);
}

__EXPORT
zx_status_t FvmInitPreallocated(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device,
                                uint64_t initial_volume_size, uint64_t max_volume_size,
                                size_t slice_size) {
  if (slice_size % fvm::kBlockSize != 0) {
    // Alignment
    return ZX_ERR_INVALID_ARGS;
  }
  if ((slice_size * fvm::kMaxVSlices) / fvm::kMaxVSlices != slice_size) {
    // Overflow
    return ZX_ERR_INVALID_ARGS;
  }
  if (initial_volume_size > max_volume_size || initial_volume_size == 0 || max_volume_size == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  fvm::Header header = fvm::Header::FromGrowableDiskSize(
      fvm::kMaxUsablePartitions, initial_volume_size, max_volume_size, slice_size);
  if (header.pslice_count == 0) {
    return ZX_ERR_NO_SPACE;
  }

  // This buffer needs to hold both copies of the metadata.
  // TODO(https://fxbug.dev/42138919): Eliminate layout assumptions.
  size_t metadata_allocated_bytes = header.GetMetadataAllocatedBytes();
  size_t dual_metadata_bytes = metadata_allocated_bytes * 2;
  std::unique_ptr<uint8_t[]> mvmo(new uint8_t[dual_metadata_bytes]);
  // Clear entire primary copy of metadata
  memset(mvmo.get(), 0, metadata_allocated_bytes);

  // Save the header to our primary metadata.
  memcpy(mvmo.get(), &header, sizeof(fvm::Header));
  size_t metadata_used_bytes = header.GetMetadataUsedBytes();
  fvm::UpdateHash(mvmo.get(), metadata_used_bytes);

  // Copy the new primary metadata to the backup copy.
  void* backup = mvmo.get() + header.GetSuperblockOffset(fvm::SuperblockType::kSecondary);
  memcpy(backup, mvmo.get(), metadata_allocated_bytes);

  // Validate our new state.
  if (!fvm::PickValidHeader(mvmo.get(), backup, metadata_used_bytes)) {
    return ZX_ERR_BAD_STATE;
  }

  // Write to primary copy.
  auto status = block_client::SingleWriteBytes(device, mvmo.get(), metadata_allocated_bytes, 0);
  if (status != ZX_OK) {
    return status;
  }
  // Write to secondary copy, to overwrite any previous FVM metadata copy that
  // could be here.
  return block_client::SingleWriteBytes(device, mvmo.get(), metadata_allocated_bytes,
                                        metadata_allocated_bytes);
}

__EXPORT
zx_status_t FvmInitWithSize(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device,
                            uint64_t volume_size, size_t slice_size) {
  return FvmInitPreallocated(device, volume_size, volume_size, slice_size);
}

__EXPORT
zx_status_t FvmInit(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device,
                    size_t slice_size) {
  // The metadata layout of the FVM is dependent on the
  // size of the FVM's underlying partition.
  const fidl::WireResult result = fidl::WireCall(device)->GetInfo();
  if (!result.ok()) {
    return result.status();
  }
  const fit::result response = result.value();
  if (response.is_error()) {
    return response.error_value();
  }
  const fuchsia_storage_block::wire::BlockInfo& block_info = response.value()->info;
  if (slice_size == 0 || slice_size % block_info.block_size) {
    return ZX_ERR_BAD_STATE;
  }

  return FvmInitWithSize(device, block_info.block_count * block_info.block_size, slice_size);
}

__EXPORT
zx::result<fidl::ClientEnd<fuchsia_device::Controller>> FvmAllocatePartition(
    fidl::UnownedClientEnd<fuchsia_storage_block::VolumeManager> fvm, uint64_t slice_count,
    uuid::Uuid type_guid, uuid::Uuid instance_guid, std::string_view name, uint32_t flags) {
  fuchsia_storage_block::wire::Guid type_fidl;
  memcpy(type_fidl.value.data(), type_guid.bytes(), fuchsia_storage_block::kGuidLength);
  fuchsia_storage_block::wire::Guid instance_fidl;
  memcpy(instance_fidl.value.data(), instance_guid.bytes(), fuchsia_storage_block::kGuidLength);
  fidl::StringView name_fidl = fidl::StringView::FromExternal(name);

  auto response = fidl::WireCall(fvm)->AllocatePartition(slice_count, type_fidl, instance_fidl,
                                                         name_fidl, flags);
  if (response.status() != ZX_OK) {
    return zx::error(response.status());
  }
  if (response->status != ZX_OK) {
    return zx::error(response->status);
  }
  PartitionMatcher matcher{
      .type_guids = {type_guid},
      .instance_guids = {instance_guid},
  };
  return OpenPartition(matcher);
}

__EXPORT
zx::result<fidl::ClientEnd<fuchsia_device::Controller>> FvmAllocatePartitionWithDevfs(
    fidl::UnownedClientEnd<fuchsia_io::Directory> devfs_root,
    fidl::UnownedClientEnd<fuchsia_storage_block::VolumeManager> fvm, uint64_t slice_count,
    uuid::Uuid type_guid, uuid::Uuid instance_guid, std::string_view name, uint32_t flags) {
  fuchsia_storage_block::wire::Guid type_fidl;
  memcpy(type_fidl.value.data(), type_guid.bytes(), fuchsia_storage_block::kGuidLength);
  fuchsia_storage_block::wire::Guid instance_fidl;
  memcpy(instance_fidl.value.data(), instance_guid.bytes(), fuchsia_storage_block::kGuidLength);
  fidl::StringView name_fidl = fidl::StringView::FromExternal(name);

  auto response = fidl::WireCall(fvm)->AllocatePartition(slice_count, type_fidl, instance_fidl,
                                                         name_fidl, flags);
  if (response.status() != ZX_OK) {
    return zx::error(response.status());
  }
  if (response->status != ZX_OK) {
    return zx::error(response->status);
  }
  PartitionMatcher matcher{
      .type_guids = {type_guid},
      .instance_guids = {instance_guid},
  };
  return OpenPartitionWithDevfs(devfs_root, matcher);
}

__EXPORT
zx::result<fuchsia_storage_block::wire::VolumeManagerInfo> FvmQuery(
    fidl::UnownedClientEnd<fuchsia_storage_block::VolumeManager> fvm) {
  const fidl::WireResult result = fidl::WireCall(fvm)->GetInfo();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  const fidl::WireResponse response = result.value();
  if (zx_status_t status = response.status; status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(*response.info);
}

__EXPORT
zx::result<fidl::ClientEnd<fuchsia_device::Controller>> OpenPartition(
    const PartitionMatcher& matcher, bool wait) {
  zx::result dir = component::OpenDirectory(kBlockDevPath);
  if (dir.is_error()) {
    return dir.take_error();
  }

  return OpenPartitionImpl(std::move(dir.value()), matcher, wait);
}

__EXPORT
zx::result<fidl::ClientEnd<fuchsia_device::Controller>> OpenPartitionWithDevfs(
    fidl::UnownedClientEnd<fuchsia_io::Directory> devfs_root, const PartitionMatcher& matcher,
    bool wait) {
  zx::result dir = component::OpenDirectoryAt(devfs_root, kBlockDevRelativePath);
  if (dir.is_error()) {
    return dir.take_error();
  }

  return OpenPartitionImpl(std::move(dir.value()), matcher, wait);
}

__EXPORT
zx::result<> DestroyPartition(const PartitionMatcher& matcher, bool wait) {
  zx::result controller = OpenPartition(matcher, wait);
  if (controller.is_error()) {
    return controller.take_error();
  }
  fidl::ClientEnd<fuchsia_storage_block::Block> volume;
  zx::result server = fidl::CreateEndpoints(&volume);
  if (server.is_error()) {
    return server.take_error();
  }
  if (fidl::OneWayStatus status =
          fidl::WireCall(controller.value())->ConnectToDeviceFidl(server->TakeChannel());
      !status.ok()) {
    return zx::error(status.status());
  }
  return DestroyPartitionImpl(volume.borrow());
}

__EXPORT
zx::result<> DestroyPartitionWithDevfs(int devfs_root_fd, const PartitionMatcher& matcher,
                                       bool wait) {
  fdio_cpp::UnownedFdioCaller devfs(devfs_root_fd);
  zx::result controller = OpenPartitionWithDevfs(devfs.directory(), matcher, wait);
  if (controller.is_error()) {
    return controller.take_error();
  }
  fidl::ClientEnd<fuchsia_storage_block::Block> volume;
  zx::result server = fidl::CreateEndpoints(&volume);
  if (server.is_error()) {
    return server.take_error();
  }
  if (fidl::OneWayStatus status =
          fidl::WireCall(controller.value())->ConnectToDeviceFidl(server->TakeChannel());
      !status.ok()) {
    return zx::error(status.status());
  }
  return DestroyPartitionImpl(volume.borrow());
}

__EXPORT
zx_status_t FvmActivate(int fvm_fd, fuchsia_storage_block::wire::Guid deactivate,
                        fuchsia_storage_block::wire::Guid activate) {
  fdio_cpp::UnownedFdioCaller caller(fvm_fd);
  fidl::UnownedClientEnd<fuchsia_storage_block::VolumeManager> client(caller.borrow_channel());
  auto response = fidl::WireCall(client)->Activate(deactivate, activate);
  if (response.status() != ZX_OK) {
    return response.status();
  }
  if (response.value().status != ZX_OK) {
    return response.value().status;
  }
  return ZX_OK;
}

}  // namespace fs_management
