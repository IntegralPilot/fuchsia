// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/fvm/fvm_test_instance.h"

#include <fidl/fuchsia.driver.test/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/fd.h>

#include <bind/fuchsia/platform/cpp/bind.h>
#include <zxtest/zxtest.h>

#include "src/storage/lib/block_server/fake_server.h"
#include "src/storage/lib/fs_management/cpp/fvm.h"
#include "src/storage/lib/fs_management/cpp/mount.h"

namespace fvm {

class ComponentBlockConnector : public BlockConnector {
 public:
  ~ComponentBlockConnector() = default;

  static std::unique_ptr<BlockConnector> Create(const fs_management::MountedVolume* volume) {
    auto connector = std::make_unique<ComponentBlockConnector>();
    connector->volume_ = volume;
    fidl::ServerEnd server_end =
        fidl::Endpoints<fuchsia_io::Directory>::Create(&connector->svc_dir_);
    EXPECT_TRUE(fidl::WireCall(connector->volume_->ExportRoot())
                    ->Open("svc", fuchsia_io::wire::kPermReadable, {}, server_end.TakeChannel())
                    .ok());
    connector->partition_ =
        fidl::ClientEnd<fuchsia_storage_block::Block>(connector->connect_block().TakeChannel());
    return std::move(connector);
  }

  fidl::ClientEnd<fuchsia_storage_block::Block> connect_block() const override {
    zx::result volume = component::ConnectAt<fuchsia_storage_block::Block>(svc_dir_);
    EXPECT_OK(volume);
    return std::move(volume.value());
  }

  fidl::UnownedClientEnd<fuchsia_storage_block::Block> as_block() const override {
    return fidl::UnownedClientEnd<fuchsia_storage_block::Block>(partition_.channel().borrow());
  }

 private:
  const fs_management::MountedVolume* volume_;
  fidl::ClientEnd<fuchsia_io::Directory> svc_dir_;
  fidl::ClientEnd<fuchsia_storage_block::Block> partition_;
};

void FvmInstance::CreateRamdisk(uint64_t block_size, uint64_t block_count) {
  if (!vmo_) {
    ASSERT_OK(zx::vmo::create(block_size * block_count, 0, &vmo_));
  }
  zx::vmo duplicate_vmo;
  ASSERT_OK(vmo_.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate_vmo));
  // This will also cause the destructor to run for any previous device.
  device_ = std::make_unique<block_server::FakeServer>(
      block_server::PartitionInfo{
          .block_count = block_count,
          .block_size = static_cast<uint32_t>(block_size),
          .max_transfer_size = 524288,
      },
      std::move(duplicate_vmo));
  auto [block_client, block_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  block_ = std::move(block_client);
  device_->Serve(std::move(block_server));
}

void FvmInstance::CreateFvm(uint64_t block_size, uint64_t block_count, uint64_t slice_size) {
  CreateRamdisk(block_size, block_count);
  ASSERT_OK(fs_management::FvmInitPreallocated(GetRamdiskPartition(), block_count * block_size,
                                               block_count * block_size, slice_size));
  StartFvm();
}

void FvmInstance::StartFvm() {
  ASSERT_TRUE(device_);
  auto [block_client, block_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  device_->Serve(std::move(block_server));
  zx::result fs = fs_management::MountMultiVolume(std::move(block_client), component_, {});
  ASSERT_OK(fs);
  fvm_ = std::make_unique<fs_management::StartedMultiVolumeFilesystem>(std::move(*fs));
  auto info = GetFvmInfo();
  slice_size_ = info.slice_size;
}

void FvmInstance::RestartFvm() {
  if (fvm_) {
    ASSERT_OK(fvm_->Unmount());
    ASSERT_OK(component_.DestroyChild());
    fvm_ = {};
  }
  StartFvm();
}

void FvmInstance::RestartFvmWithNewDiskSize(uint64_t block_size, uint64_t block_count) {
  if (fvm_) {
    ASSERT_OK(fvm_->Unmount());
    ASSERT_OK(component_.DestroyChild());
    fvm_ = {};
  }

  zx::vmo vmo;
  ASSERT_OK(vmo_.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, block_count * block_size, &vmo));
  vmo_ = std::move(vmo);

  CreateRamdisk(block_size, block_count);
  StartFvm();
}

fuchsia_storage_block::wire::VolumeManagerInfo FvmInstance::GetFvmInfo() const {
  EXPECT_TRUE(fvm_);
  zx::result volumes = component::ConnectAt<fuchsia_fs_startup::Volumes>(fvm_->ServiceDirectory());
  EXPECT_OK(volumes);
  fidl::WireResult info = fidl::WireCall(*volumes)->GetInfo();
  EXPECT_TRUE(info.ok());
  EXPECT_TRUE(info->is_ok());
  return *info.value()->info;
}

zx::result<std::unique_ptr<BlockConnector>> FvmInstance::AllocatePartition(
    const AllocatePartitionRequest& request) const {
  EXPECT_TRUE(fvm_);
  fidl::Array<uint8_t, 16> type_guid = fidl::Array<uint8_t, 16>{1, 2, 3, 4};
  memcpy(type_guid.data(), request.type.bytes(), 16);
  fidl::Arena arena;
  zx::result volume = fvm_->CreateVolume(request.name,
                                         fuchsia_fs_startup::wire::CreateOptions::Builder(arena)
                                             .type_guid(type_guid)
                                             .initial_size(request.slice_count * slice_size_)
                                             .Build(),
                                         {});
  if (volume.is_error()) {
    return volume.take_error();
  }
  return zx::ok(ComponentBlockConnector::Create(volume.value()));
}

zx::result<std::unique_ptr<BlockConnector>> FvmInstance::OpenPartition(
    std::string_view label) const {
  EXPECT_TRUE(fvm_);
  std::string name = std::string(label);
  const fs_management::MountedVolume* volume = fvm_->GetVolume(name);
  if (volume == nullptr) {
    zx::result opened_volume = fvm_->OpenVolume(name, {});
    if (opened_volume.is_error()) {
      return opened_volume.take_error();
    }
    volume = opened_volume.value();
  }
  return zx::ok(ComponentBlockConnector::Create(volume));
}

void FvmInstance::DestroyPartition(std::string_view label) const {
  ASSERT_OK(fvm_->RemoveVolume(label));
}

fidl::ClientEnd<fuchsia_storage_block::Block> FvmInstance::GetRamdiskPartition() const {
  auto [block_client, block_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  device_->Serve(std::move(block_server));
  return std::move(block_client);
}

}  // namespace fvm
