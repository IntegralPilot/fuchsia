// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/testing/fvm.h"

#include <fcntl.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fdio.h>
#include <lib/syslog/cpp/macros.h>

#include <fbl/unique_fd.h>
#include <ramdevice-client/ramdisk.h>

#include "src/storage/lib/fs_management/cpp/format.h"
#include "src/storage/lib/fs_management/cpp/fvm.h"

namespace storage {
namespace {

// If `slice_size` is set, formats the device.
zx::result<FvmInstance> CreateFvmInstance(const std::string& device_path,
                                          std::optional<size_t> slice_size) {
  zx::result device = component::Connect<fuchsia_storage_block::Block>(device_path);
  if (device.is_error()) {
    return device.take_error();
  }
  if (slice_size) {
    if (zx::result status = zx::make_result(fs_management::FvmInit(device.value(), *slice_size));
        status.is_error()) {
      FX_LOGS(ERROR) << "Could not format disk with FVM";
      return status.take_error();
    }
  }

  // Start the FVM filesystem.
  auto component = fs_management::FsComponent::FromDiskFormat(fs_management::kDiskFormatFvm);

  auto fs = MountMultiVolume(*std::move(device), component, fs_management::MountOptions());
  if (fs.is_error())
    return fs.take_error();
  return zx::ok(FvmInstance(std::move(component), *std::move(fs)));
}

namespace fio = fuchsia_io;

constexpr std::array<uint8_t, 16> kTestPartGUID = {0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
                                                   0xFF, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07};
}  // namespace

zx::result<FvmPartition> OpenFvmPartition(const std::string& device_path,
                                          std::string_view partition_name) {
  zx::result fvm = CreateFvmInstance(device_path, std::nullopt);
  if (fvm.is_error()) {
    return fvm.take_error();
  }

  zx::result volume =
      fvm->fs().OpenVolume(partition_name, fuchsia_fs_startup::wire::MountOptions());
  if (volume.is_error()) {
    return volume.take_error();
  }

  static std::atomic<int> counter(0);
  std::string path = "/test-fvm-" + std::to_string(++counter);

  auto [client, server] = fidl::Endpoints<fio::Directory>::Create();
  if (fidl::OneWayStatus status =
          fidl::WireCall<fuchsia_io::Directory>(volume->ExportRoot())
              ->Clone(fidl::ServerEnd<fuchsia_unknown::Cloneable>(server.TakeChannel()));
      !status.ok()) {
    return zx::error(status.status());
  }

  auto binding = fs_management::NamespaceBinding::Create(path.c_str(), std::move(client));
  if (binding.is_error())
    return binding.take_error();

  path += "/svc/fuchsia.storage.block.Block";

  return zx::ok(FvmPartition(*std::move(fvm), *std::move(binding), partition_name, path));
}

zx::result<FvmPartition> CreateFvmPartition(const std::string& device_path, size_t slice_size,
                                            const FvmOptions& options) {
  // Format the raw device to support FVM, and bind the FVM driver to it.
  zx::result fvm = CreateFvmInstance(device_path, slice_size);
  if (fvm.is_error()) {
    return fvm.take_error();
  }

  fidl::Array<uint8_t, 16> type_guid = fidl::Array<uint8_t, 16>{1, 2, 3, 4};
  memcpy(type_guid.data(), options.type ? options.type->data() : kTestPartGUID.data(), 16);

  fidl::Arena arena;
  zx::result volume =
      fvm->fs().CreateVolume(options.name,
                             fuchsia_fs_startup::wire::CreateOptions::Builder(arena)
                                 .type_guid(std::move(type_guid))
                                 .initial_size(options.initial_fvm_slice_count * slice_size)
                                 .Build(),
                             fuchsia_fs_startup::wire::MountOptions());
  if (volume.is_error())
    return volume.take_error();

  static std::atomic<int> counter(0);
  std::string path = "/test-fvm-" + std::to_string(++counter);

  auto [client, server] = fidl::Endpoints<fio::Directory>::Create();
  if (fidl::OneWayStatus status =
          fidl::WireCall<fuchsia_io::Directory>(volume->ExportRoot())
              ->Clone(fidl::ServerEnd<fuchsia_unknown::Cloneable>(server.TakeChannel()));
      !status.ok()) {
    return zx::error(status.status());
  }

  auto binding = fs_management::NamespaceBinding::Create(path.c_str(), std::move(client));
  if (binding.is_error())
    return binding.take_error();

  path += "/svc/fuchsia.storage.block.Block";

  return zx::ok(FvmPartition(*std::move(fvm), *std::move(binding), options.name, path));
}

zx::result<> FvmPartition::SetLimit(uint64_t limit) {
  zx::result volume = component::ConnectAt<fuchsia_fs_startup::Volume>(
      fvm_.fs().ServiceDirectory(), (std::string("volumes/") + partition_name_).c_str());
  if (volume.is_error())
    return volume.take_error();
  fidl::WireResult result = fidl::WireCall(*volume)->SetLimit(limit);
  if (!result.ok()) {
    FX_LOGS(ERROR) << "SetLimit FIDL call failed: " << result.FormatDescription();
    return zx::error(result.status());
  }
  if (result->is_error()) {
    FX_LOGS(ERROR) << "SetLimit failed: " << zx_status_get_string(result->error_value());
    return zx::error(result->error_value());
  }
  return zx::ok();
}

zx::result<fidl::ClientEnd<fuchsia_storage_block::Block>> FvmPartition::Connect() const {
  auto [client, server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  if (zx_status_t status =
          fdio_service_connect(path_.c_str(), std::move(server).TakeHandle().release());
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(client));
}

}  // namespace storage
