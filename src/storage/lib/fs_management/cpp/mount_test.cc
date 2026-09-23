// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/fs_management/cpp/mount.h"

#include <errno.h>
#include <fcntl.h>
#include <fidl/fuchsia.fs/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/component/incoming/cpp/clone.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/syslog/cpp/macros.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/statfs.h>
#include <sys/statvfs.h>
#include <unistd.h>
#include <zircon/syscalls.h>

#include <utility>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <ramdevice-client/ramdisk.h>

#include "src/storage/lib/block_server/fake_server.h"
#include "src/storage/lib/fs_management/cpp/admin.h"
#include "src/storage/lib/fs_management/cpp/fvm.h"
#include "src/storage/testing/fvm.h"

namespace fs_management {
namespace {

const char* kTestMountPath = "/test/mount";

void CheckMountedFs(const char* path, const char* fs_name) {
  fbl::unique_fd fd(open(path, O_RDONLY | O_DIRECTORY));
  ASSERT_TRUE(fd);

  fdio_cpp::FdioCaller caller(std::move(fd));
  auto result = fidl::WireCall(caller.directory())->QueryFilesystem();
  ASSERT_EQ(result.status(), ZX_OK);
  ASSERT_EQ(result.value().s, ZX_OK);
  fuchsia_io::wire::FilesystemInfo info = *result.value().info;
  ASSERT_EQ(strncmp(fs_name, reinterpret_cast<char*>(info.name.data()), strlen(fs_name)), 0);
  ASSERT_LE(info.used_nodes, info.total_nodes) << "Used nodes greater than free nodes";
  ASSERT_LE(info.used_bytes, info.total_bytes) << "Used bytes greater than free bytes";
  // TODO(planders): eventually check that total/used counts are > 0
}

class RamdiskTestFixture : public testing::Test {
 public:
  void SetUp() override {
    zx::result ramdisk = ramdevice_client::Ramdisk::Create(512, 1 << 16);
    ASSERT_EQ(ramdisk.status_value(), ZX_OK);
    ramdisk_ = std::move(*ramdisk);

    auto component = FsComponent::FromDiskFormat(kDiskFormatMinfs);
    ASSERT_EQ(Mkfs(ramdisk_path().c_str(), component, {}), ZX_OK);
  }

  std::string ramdisk_path() const { return ramdisk_.path(); }
  ramdisk_client_t* ramdisk_client() const { return ramdisk_.client(); }

  struct MountResult {
    FsComponent component;
    StartedSingleVolumeFilesystem fs;
    NamespaceBinding binding;
  };

  // Mounts a minfs formatted partition to the desired point.
  zx::result<MountResult> MountMinfs(bool read_only) {
    MountOptions options{.readonly = read_only};

    zx::result<fidl::ClientEnd<fuchsia_storage_block::Block>> block_client =
        component::Connect<fuchsia_storage_block::Block>(ramdisk_path());
    if (block_client.is_error()) {
      return block_client.take_error();
    }

    auto component = FsComponent::FromDiskFormat(kDiskFormatMinfs);
    auto mounted_filesystem = Mount(std::move(block_client.value()), component, options);
    if (mounted_filesystem.is_error())
      return mounted_filesystem.take_error();
    auto data_root = mounted_filesystem->DataRoot();
    if (data_root.is_error())
      return data_root.take_error();
    auto binding = NamespaceBinding::Create(kTestMountPath, *std::move(data_root));
    if (binding.is_error())
      return binding.take_error();
    CheckMountedFs(kTestMountPath, "minfs");
    return zx::ok(MountResult{.component = std::move(component),
                              .fs = std::move(*mounted_filesystem),
                              .binding = std::move(*binding)});
  }

  // Formats the ramdisk with minfs, and writes a small file to it.
  void CreateTestFile(const char* file_name) {
    auto mounted_filesystem_or = MountMinfs(/*read_only=*/false);
    ASSERT_EQ(mounted_filesystem_or.status_value(), ZX_OK);

    fbl::unique_fd root_fd(open(kTestMountPath, O_RDONLY | O_DIRECTORY));
    ASSERT_TRUE(root_fd);
    fbl::unique_fd fd(openat(root_fd.get(), file_name, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR));
    ASSERT_TRUE(fd);
    ASSERT_EQ(write(fd.get(), "hello", 6), 6);
  }

 private:
  ramdevice_client::Ramdisk ramdisk_;
};

using MountTest = RamdiskTestFixture;

TEST_F(MountTest, MountRemount) {
  // We should still be able to mount and unmount the filesystem multiple times
  for (size_t i = 0; i < 10; i++) {
    auto fs = MountMinfs(/*read_only=*/false);
    ASSERT_EQ(fs.status_value(), ZX_OK);
  }
}

TEST_F(MountTest, MountFsck) {
  {
    auto mounted_filesystem_or = MountMinfs(/*read_only=*/false);
    ASSERT_EQ(mounted_filesystem_or.status_value(), ZX_OK);
  }

  // Fsck shouldn't require any user input for a newly mkfs'd filesystem.
  auto component = FsComponent::FromDiskFormat(kDiskFormatMinfs);
  ASSERT_EQ(Fsck(ramdisk_path(), component, FsckOptions()), ZX_OK);
}

// Tests that setting read-only on the mount options works as expected.
TEST_F(MountTest, MountReadonly) {
  const char file_name[] = "some_file";
  CreateTestFile(file_name);

  bool read_only = true;
  auto mounted_filesystem_or = MountMinfs(read_only);
  ASSERT_EQ(mounted_filesystem_or.status_value(), ZX_OK);

  fbl::unique_fd root_fd(open(kTestMountPath, O_RDONLY | O_DIRECTORY));
  ASSERT_TRUE(root_fd);
  fbl::unique_fd fd(openat(root_fd.get(), file_name, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR));

  // We can no longer open the file as writable
  ASSERT_FALSE(fd);

  // We CAN open it as readable though
  fd.reset(openat(root_fd.get(), file_name, O_RDONLY));
  ASSERT_TRUE(fd);
  ASSERT_LT(write(fd.get(), "hello", 6), 0);
  char buf[6];
  ASSERT_EQ(read(fd.get(), buf, 6), 6);
  ASSERT_EQ(memcmp(buf, "hello", 6), 0);

  ASSERT_LT(renameat(root_fd.get(), file_name, root_fd.get(), "new_file"), 0);
  ASSERT_LT(unlinkat(root_fd.get(), file_name, 0), 0);
}

TEST_F(MountTest, StatfsTest) {
  auto mounted_filesystem_or = MountMinfs(/*read_only=*/false);
  ASSERT_EQ(mounted_filesystem_or.status_value(), ZX_OK);

  errno = 0;
  struct statfs stats;
  int rc = statfs("", &stats);
  int err = errno;
  ASSERT_EQ(rc, -1);
  ASSERT_EQ(err, ENOENT);

  rc = statfs(kTestMountPath, &stats);
  ASSERT_EQ(rc, 0);

  // Verify that at least some values make sense, without making the test too brittle.
  ASSERT_EQ(stats.f_type, fidl::ToUnderlying(fuchsia_fs::VfsType::kMinfs));
  ASSERT_NE(stats.f_fsid.__val[0] | stats.f_fsid.__val[1], 0);
  ASSERT_EQ(stats.f_bsize, 8192u);
  ASSERT_EQ(stats.f_namelen, 255u);
  ASSERT_GT(stats.f_bavail, 0u);
  ASSERT_GT(stats.f_ffree, 0u);
}

TEST_F(MountTest, StatvfsTest) {
  auto mounted_filesystem_or = MountMinfs(/*read_only=*/false);
  ASSERT_EQ(mounted_filesystem_or.status_value(), ZX_OK);

  errno = 0;
  struct statvfs stats;
  int rc = statvfs("", &stats);
  int err = errno;
  ASSERT_EQ(rc, -1);
  ASSERT_EQ(err, ENOENT);

  rc = statvfs(kTestMountPath, &stats);
  ASSERT_EQ(rc, 0);

  // Verify that at least some values make sense, without making the test too brittle.
  ASSERT_NE(stats.f_fsid, 0ul);
  ASSERT_EQ(stats.f_bsize, 8192u);
  ASSERT_EQ(stats.f_frsize, 8192u);
  ASSERT_EQ(stats.f_namemax, 255u);
  ASSERT_GT(stats.f_bavail, 0u);
  ASSERT_GT(stats.f_ffree, 0u);
  ASSERT_GT(stats.f_favail, 0u);
}

void GetPartitionSliceCount(fidl::UnownedClientEnd<fuchsia_storage_block::Block> volume,
                            size_t* out_count) {
  auto res = fidl::WireCall(volume)->GetVolumeInfo();
  ASSERT_EQ(res.status(), ZX_OK);
  ASSERT_EQ(res.value().status, ZX_OK);

  size_t allocated_slices = 0;
  std::vector<uint64_t> start_slices = {0};
  while (start_slices[0] < res.value().manager->max_virtual_slice) {
    auto res =
        fidl::WireCall(volume)->QuerySlices(fidl::VectorView<uint64_t>::FromExternal(start_slices));
    ASSERT_EQ(res.status(), ZX_OK);
    ASSERT_EQ(res.value().status, ZX_OK);

    start_slices[0] += res.value().response[0].count;
    if (res.value().response[0].allocated) {
      allocated_slices += res.value().response[0].count;
    }
  }

  // The two methods of getting the partition slice count should agree.
  ASSERT_EQ(res.value().volume->partition_slice_count, allocated_slices);

  *out_count = allocated_slices;
}

class PartitionOverFvmWithRamdiskFixture : public testing::Test {
 public:
  const char* partition_path() const { return fvm_partition_->path().c_str(); }

 protected:
  static constexpr uint64_t kBlockSize = 512;

  void SetUp() override {
    size_t ramdisk_block_count = zx_system_get_physmem() / (1024);
    zx::result ramdisk = ramdevice_client::Ramdisk::Create(kBlockSize, ramdisk_block_count);
    ASSERT_EQ(ramdisk.status_value(), ZX_OK);
    ramdisk_ = std::move(*ramdisk);

    uint64_t slice_size = kBlockSize * (2 << 10);
    auto partition = storage::CreateFvmPartition(ramdisk_.path(), static_cast<size_t>(slice_size));
    ASSERT_TRUE(partition.is_ok()) << partition.status_string();
    fvm_partition_.emplace(*std::move(partition));
  }

 private:
  ramdevice_client::Ramdisk ramdisk_;
  std::optional<storage::FvmPartition> fvm_partition_;
};

using PartitionOverFvmWithRamdiskCase = PartitionOverFvmWithRamdiskFixture;

// Reformat the partition using a number of slices and verify that there are as many slices as
// originally pre-allocated.
TEST_F(PartitionOverFvmWithRamdiskCase, MkfsMinfsWithMinFvmSlices) {
  size_t base_slices = 0;
  auto component = fs_management::FsComponent::FromDiskFormat(kDiskFormatMinfs);
  MkfsOptions options;
  ASSERT_EQ(Mkfs(partition_path(), component, options), ZX_OK);
  zx::result volume = component::Connect<fuchsia_storage_block::Block>(partition_path());
  ASSERT_TRUE(volume.is_ok()) << volume.status_string();
  GetPartitionSliceCount(volume.value(), &base_slices);
  options.fvm_data_slices += 10;

  ASSERT_EQ(Mkfs(partition_path(), component, options), ZX_OK);
  size_t allocated_slices = 0;
  GetPartitionSliceCount(volume.value(), &allocated_slices);
  EXPECT_GE(allocated_slices, base_slices + 10);

  DiskFormat actual_format = DetectDiskFormat(
      fidl::UnownedClientEnd<fuchsia_storage_block::Block>(volume.value().channel().borrow()));
  ASSERT_EQ(actual_format, kDiskFormatMinfs);
}

TEST(FvmTest, Basic) {
  block_server::FakeServer fake_server(block_server::PartitionInfo{
      .block_count = 4096,
      .block_size = 512,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "block-device",
  });

  auto endpoints = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  fake_server.Serve(std::move(endpoints.server));

  fidl::ClientEnd<fuchsia_storage_block::Block> client(endpoints.client.TakeChannel());

  constexpr int kSliceSize = 32768;
  ASSERT_EQ(FvmInit(client, kSliceSize), ZX_OK);

  auto check_volume = [](MountedVolume* volume) {
    auto volume_endpoints = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
    ASSERT_EQ(fdio_service_connect_at(volume->ExportRoot().channel().get(),
                                      (std::string("svc/") +
                                       fidl::DiscoverableProtocolName<fuchsia_storage_block::Block>)
                                          .c_str(),
                                      volume_endpoints.server.TakeChannel().get()),
              ZX_OK);

    const fidl::WireResult result = fidl::WireCall(volume_endpoints.client)->GetInfo();
    ASSERT_EQ(result.status(), ZX_OK);

    ASSERT_TRUE(result.value().is_ok()) << zx_status_get_string(result.value().error_value());

    ASSERT_EQ(result.value()->info.block_size, 512u);
  };

  {
    auto component = FsComponent::FromDiskFormat(fs_management::kDiskFormatFvm);

    auto fs = MountMultiVolume(std::move(client), component, fs_management::MountOptions());
    ASSERT_EQ(fs.status_value(), ZX_OK);

    fidl::Arena arena;
    zx::result volume = fs->CreateVolume("test",
                                         fuchsia_fs_startup::wire::CreateOptions::Builder(arena)
                                             .type_guid(fidl::Array<uint8_t, 16>{1, 2, 3, 4})
                                             .initial_size(16 * kSliceSize)
                                             .Build(),
                                         fuchsia_fs_startup::wire::MountOptions());
    ASSERT_EQ(volume.status_value(), ZX_OK);

    check_volume(*volume);
  }

  // Bind again and check we can mount the volume we created.
  endpoints = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  fake_server.Serve(std::move(endpoints.server));

  auto component = FsComponent::FromDiskFormat(fs_management::kDiskFormatFvm);

  auto fs = MountMultiVolume(
      fidl::ClientEnd<fuchsia_storage_block::Block>(endpoints.client.TakeChannel()), component,
      fs_management::MountOptions());
  ASSERT_EQ(fs.status_value(), ZX_OK);

  zx::result volume = fs->OpenVolume("test", fuchsia_fs_startup::wire::MountOptions());
  ASSERT_EQ(volume.status_value(), ZX_OK);

  check_volume(*volume);
}

}  // namespace
}  // namespace fs_management
