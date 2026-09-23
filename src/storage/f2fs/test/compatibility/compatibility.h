// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_TEST_COMPATIBILITY_COMPATIBILITY_H_
#define SRC_STORAGE_F2FS_TEST_COMPATIBILITY_COMPATIBILITY_H_

#include <lib/fdio/fd.h>
#include <lib/fit/defer.h>

#include <cinttypes>
#include <cstddef>
#include <filesystem>
#include <sstream>
#include <string>
#include <string_view>
#include <vector>

#include <fbl/ref_ptr.h>

// clang-format off: work-around a collision between a banjo macro and a FIDL constant. We can
// work-around the issue by including the virtualization headers before the f2fs headers.
#include "src/virtualization/tests/lib/enclosed_guest.h"
#include "src/virtualization/tests/lib/guest_test.h"
// clang-format on
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/test/compatibility/file_backed_block_device.h"

namespace f2fs {

using Runner = ComponentRunner;
constexpr size_t kTestBlockSize = 4096;
constexpr size_t kTestBlockCount = 25600;
constexpr size_t kTestBlockDeviceSize = kTestBlockSize * kTestBlockCount;

constexpr std::string_view kLinuxPathPrefix = "//";
constexpr std::string_view kTestDeviceId = "f2fs_test_device";

class F2fsDebianGuest;
class LinuxOperator;

class TestFile {
 public:
  virtual ~TestFile() = default;

  virtual bool IsValid() = 0;

  virtual ssize_t Read(void* buf, size_t count) = 0;
  virtual ssize_t Write(const void* buf, size_t count) = 0;
  virtual int Fchmod(mode_t mode) = 0;
  virtual int Fstat(struct stat& file_stat) = 0;
  virtual int Ftruncate(off_t len) = 0;
  virtual int Fallocate(int mode, off_t offset, off_t len) = 0;

  virtual void WritePattern(size_t block_count, size_t interval) = 0;
  virtual void VerifyPattern(size_t block_count, size_t interval) = 0;
};

class LinuxTestFile : public TestFile {
 public:
  explicit LinuxTestFile(std::string_view filename, LinuxOperator* linux_operator)
      : filename_(filename), linux_operator_(linux_operator) {}

  bool IsValid() final;

  ssize_t Read(void* buf, size_t count) final { return -1; }
  ssize_t Write(const void* buf, size_t count) final;
  int Fchmod(mode_t mode) final;
  int Fstat(struct stat& file_stat) final;
  int Ftruncate(off_t len) final;
  int Fallocate(int mode, off_t offset, off_t len) final;

  void WritePattern(size_t block_count, size_t interval) final;
  void VerifyPattern(size_t block_count, size_t interval) final;

 private:
  std::string filename_;
  LinuxOperator* linux_operator_;
};

class FuchsiaTestFile : public TestFile {
 public:
  explicit FuchsiaTestFile(fbl::RefPtr<VnodeF2fs> vnode) : vnode_(std::move(vnode)) {}
  ~FuchsiaTestFile() override {
    if (vnode_ != nullptr) {
      vnode_->Close();
    }
  }

  bool IsValid() final { return (vnode_ != nullptr); }

  ssize_t Read(void* buf, size_t count) final;
  ssize_t Write(const void* buf, size_t count) final;
  int Fchmod(mode_t mode) final { return -1; }
  int Fstat(struct stat& file_stat) final;
  int Ftruncate(off_t len) final;
  int Fallocate(int mode, off_t offset, off_t len) final { return -1; }

  void WritePattern(size_t block_count, size_t interval) final;
  void VerifyPattern(size_t block_count, size_t interval) final;

  VnodeF2fs* GetRawVnodePtr() { return vnode_.get(); }

 private:
  fbl::RefPtr<VnodeF2fs> vnode_;
  // TODO: Add Lseek to adjust |offset_|
  size_t offset_ = 0;
};

class CompatibilityTestOperator {
 public:
  explicit CompatibilityTestOperator(std::string_view test_device) : test_device_(test_device) {}
  virtual ~CompatibilityTestOperator() = default;

  virtual void Mkfs() = 0;
  virtual void Fsck() = 0;
  virtual void Mount() = 0;
  virtual void Umount() = 0;

  virtual void Mkdir(std::string_view path, mode_t mode) = 0;
  // Return value is 0 on success, -1 on error.
  virtual int Rmdir(std::string_view path) = 0;
  virtual std::unique_ptr<TestFile> Open(std::string_view path, int flags, mode_t mode) = 0;
  virtual void Rename(std::string_view oldpath, std::string_view newpath) = 0;

 protected:
  const std::string test_device_;
};

class LinuxOperator : public CompatibilityTestOperator {
 public:
  explicit LinuxOperator(std::string_view test_device, F2fsDebianGuest* debian_guest)
      : CompatibilityTestOperator(test_device), debian_guest_(debian_guest) {}

  void Mkfs() final { Mkfs(std::string_view{""}); }
  void Mkfs(std::string_view opt);
  void Fsck() final;
  void Mount() final { Mount(std::string_view{""}); }
  void Mount(std::string_view opt);
  void Umount() final;

  void Mkdir(std::string_view path, mode_t mode) final;
  int Rmdir(std::string_view path) final;
  std::unique_ptr<TestFile> Open(std::string_view path, int flags, mode_t mode) final;
  void Rename(std::string_view oldpath, std::string_view newpath) final;

  zx_status_t Execute(const std::vector<std::string>& argv, std::string* result = nullptr);
  void ExecuteWithAssert(const std::vector<std::string>& argv, std::string* result = nullptr);
  std::string ConvertPath(std::string_view path);

  void CheckLinuxVersion(const int major, const int minor);
  // "dry-run" of fsck needs version at least 1.14
  void CheckF2fsToolsVersion(const int major = 1, const int minor = 14);

 private:
  F2fsDebianGuest* debian_guest_;
  const std::string mount_path_ = "compat_mnt";
};

class FuchsiaOperator : public CompatibilityTestOperator {
 public:
  explicit FuchsiaOperator(std::string_view test_device, size_t block_count, size_t block_size)
      : CompatibilityTestOperator(test_device), block_count_(block_count), block_size_(block_size) {
    auto fd = fbl::unique_fd(open(test_device_.c_str(), O_RDWR));
    auto device = std::make_unique<FileBackedBlockDevice>(std::move(fd), block_count_, block_size_);
    auto bc_or = CreateBcacheMapper(std::move(device), true);
    if (bc_or.is_ok()) {
      bc_ = std::move(*bc_or);
    }
    loop_.StartThread();
  }
  ~FuchsiaOperator() override {
    loop_.RunUntilIdle();
    loop_.Quit();
    loop_.JoinThreads();
  }

  void Mkfs() final { Mkfs(MkfsOptions{}); }
  void Mkfs(MkfsOptions opt);
  void Fsck() final;
  void Mount() final { Mount(MountOptions{}); }
  void Mount(MountOptions opt);
  void Umount() final;

  void Mkdir(std::string_view path, mode_t mode) final;
  int Rmdir(std::string_view path) final;
  std::unique_ptr<TestFile> Open(std::string_view path, int flags, mode_t mode) final;
  void Rename(std::string_view oldpath, std::string_view newpath) final;

  // Maximum number of inline dentry slots
  uint32_t MaxInlineDentrySlots();
  // Maximum inline data length in bytes
  uint32_t MaxInlineDataLength();

 private:
  zx::result<std::pair<fbl::RefPtr<fs::Vnode>, std::string>> GetLastDirVnodeAndFileName(
      std::string_view absolute_path);

  size_t block_count_;
  size_t block_size_;
  std::unique_ptr<BcacheMapper> bc_;
  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  std::unique_ptr<F2fs> fs_;
  fbl::RefPtr<VnodeF2fs> root_;
};

class F2fsDebianGuest : public DebianEnclosedGuest {
 public:
  F2fsDebianGuest(async_dispatcher_t* dispatcher, RunLoopUntilFunc run_loop_until)
      : DebianEnclosedGuest(dispatcher, std::move(run_loop_until)) {}

  zx_status_t BuildLaunchInfo(GuestLaunchInfo* launch_info) override {
    if (zx_status_t status = DebianEnclosedGuest::BuildLaunchInfo(launch_info); status != ZX_OK) {
      return status;
    }

    // Disable other virtio devices to ensure there's enough space on the PCI
    // bus, and to simplify slot assignment.
    launch_info->config.set_default_net(false);
    launch_info->config.set_virtio_balloon(false);
    launch_info->config.set_virtio_gpu(false);
    launch_info->config.set_virtio_rng(false);
    launch_info->config.set_virtio_sound(false);
    launch_info->config.set_virtio_vsock(false);

    auto* cfg = &launch_info->config;

    std::vector<fuchsia::virtualization::BlockSpec> block_specs;

    std::string guest_path = "/tmp/guest-test.XXXXXX";

    fbl::unique_fd fd(mkstemp(guest_path.data()));
    guest_path_ = guest_path;
    if (!fd) {
      FX_LOGS(ERROR) << "Failed to create temporary file";
      return ZX_ERR_IO;
    }
    if (auto status = ftruncate(fd.get(), kTestBlockDeviceSize); status != ZX_OK) {
      return status;
    }

    zx::channel channel;
    if (zx_status_t status = fdio_fd_transfer(fd.release(), channel.reset_and_get_address());
        status != ZX_OK) {
      return status;
    }
    block_specs.emplace_back(fuchsia::virtualization::BlockSpec{
        .id = std::string(kTestDeviceId),
        .mode = fuchsia::virtualization::BlockMode::READ_WRITE,
        .format = fuchsia::virtualization::BlockFormat::WithFile(
            fidl::InterfaceHandle<fuchsia::io::File>(std::move(channel))),
    });
    cfg->set_block_devices(std::move(block_specs));

    linux_operator_ = std::make_unique<LinuxOperator>(linux_device_path_, this);
    fuchsia_operator_ =
        std::make_unique<FuchsiaOperator>(guest_path_, kTestBlockCount, kTestBlockSize);

    return ZX_OK;
  }

  const std::string& GuestPath() { return guest_path_; }
  const std::string& LinuxDevicePath() { return linux_device_path_; }

  LinuxOperator& GetLinuxOperator() { return *linux_operator_; }
  FuchsiaOperator& GetFuchsiaOperator() { return *fuchsia_operator_; }

 private:
  std::string guest_path_;
  // Could be a different path on aarch64
  const std::string linux_device_path_ = "/dev/disk/by-id/virtio-" + std::string(kTestDeviceId);

  std::unique_ptr<LinuxOperator> linux_operator_;
  std::unique_ptr<FuchsiaOperator> fuchsia_operator_;
};

class F2fsGuestTest : public GuestTest<F2fsDebianGuest> {
 protected:
  void SetUp() override {
    GuestTest<F2fsDebianGuest>::SetUp();
    GetEnclosedGuest().GetLinuxOperator().CheckF2fsToolsVersion();
  }
};

fs::DeprecatedOptions ConvertFlag(int flags);
void CompareStat(const struct stat& a, const struct stat& b);

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_TEST_COMPATIBILITY_COMPATIBILITY_H_
