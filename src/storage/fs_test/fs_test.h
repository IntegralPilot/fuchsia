// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_FS_TEST_FS_TEST_H_
#define SRC_STORAGE_FS_TEST_FS_TEST_H_

#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>

#include <functional>
#include <iostream>
#include <limits>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <variant>
#include <vector>

#include <fbl/unique_fd.h>
#include <ramdevice-client/ramdisk.h>
#include <ramdevice-client/ramnand.h>

#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/compression_settings.h"
#include "src/storage/lib/fs_management/cpp/component.h"
#include "src/storage/lib/fs_management/cpp/mount.h"
#include "src/storage/lib/fs_management/cpp/options.h"
#include "src/storage/testing/fvm.h"

namespace fs_test {

class Filesystem;

// Holds the ram disk or nand and additional required drivers (e.g. FVM).
class RamDevice {
 public:
  RamDevice(ramdevice_client::Ramdisk ram_disk, std::string_view path)
      : device_(std::move(ram_disk)), path_(path) {}
  RamDevice(ramdevice_client::RamNand ram_nand, std::string_view path)
      : device_(std::move(ram_nand)), path_(path) {}

  void set_fvm_partition(storage::FvmPartition fvm_partition) {
    path_ = fvm_partition.path();
    fvm_partition_ = std::move(fvm_partition);
  }

  void set_path(std::string_view path) { path_ = path; }
  const std::string& path() const { return path_; }

  ramdevice_client::Ramdisk* ram_disk() { return std::get_if<ramdevice_client::Ramdisk>(&device_); }
  ramdevice_client::RamNand* ram_nand() { return std::get_if<ramdevice_client::RamNand>(&device_); }

  std::optional<storage::FvmPartition>& fvm_partition() { return fvm_partition_; }

 private:
  std::variant<ramdevice_client::Ramdisk, ramdevice_client::RamNand> device_;
  std::string path_;
  std::optional<storage::FvmPartition> fvm_partition_;
};

constexpr const char kDefaultVolumeName[] = "default";

struct TestFilesystemOptions {
  static TestFilesystemOptions DefaultBlobfs();
  static TestFilesystemOptions BlobfsWithoutFvm();

  std::string description;
  bool use_ram_nand = false;
  // If set specifies a VMO to be used to back the device.  If used for ram-nand, Its size must
  // match the device size (if device_block_count is non-zero), including the extra required for
  // OOB.
  zx::unowned_vmo vmo;

  // Mount an fvm partition to use for the resident filesystems. If use_existing_fvm is false this
  // will try to create a new fvm partition.
  bool use_fvm = false;

  // Look for an existing fvm and attach to that.
  bool use_existing_fvm = false;

  // Uses this vmo as an aside for storing actual nand erase counts for a ramn and. There should be
  // enough space to store a uint32_t per physical block of the ram nand.
  zx::unowned_vmo nand_wear_vmo;

  // If non-zero, create a dummy FVM partition which has the effect of moving the location of the
  // partition under test to be at a different offset on the underlying device.
  uint64_t dummy_fvm_partition_size = 0;

  uint64_t device_block_size = 0;
  uint64_t device_block_count = 0;

  uint32_t nand_page_size = 0;
  uint32_t nand_pages_per_block = 0;
  uint32_t nand_oob_size = 0;

  uint64_t fvm_slice_size = 0;
  uint64_t initial_fvm_slice_count = 1;
  // Only supported for blobfs for now.
  uint64_t num_inodes = 0;
  const Filesystem* filesystem = nullptr;
  // By default the ram-disk we create is filled with a non-zero value (so that we don't
  // inadvertently depend on it), but that won't work for very large ram-disks (they will trigger
  // OOMs), in which case they can be zero filled.
  bool zero_fill = false;

  // The format blobfs should store blobs in.
  blobfs::BlobLayoutFormat blob_layout_format = blobfs::BlobLayoutFormat::kCompactMerkleTreeAtEnd;

  // If using ram_nand, the number of writes after which writes should fail.
  uint32_t fail_after;

  // If true, when the ram-disk is disconnected it will discard random writes performed since the
  // last flush (which is all that any device will guarantee).
  bool ram_disk_discard_random_after_last_flush = false;
};

std::ostream& operator<<(std::ostream& out, const TestFilesystemOptions& options);

std::vector<TestFilesystemOptions> AllTestFilesystems();
// Provides the ability to map and filter all test file systems, using the supplied function.
std::vector<TestFilesystemOptions> MapAndFilterAllTestFilesystems(
    const std::function<std::optional<TestFilesystemOptions>(const TestFilesystemOptions&)>&);
TestFilesystemOptions OptionsWithDescription(std::string_view description);

zx::result<RamDevice> CreateRamDevice(const TestFilesystemOptions& options);

// Returns a handle to a test crypt service.
zx::result<fidl::ClientEnd<fuchsia_fxfs::Crypt>> InitializeCryptService();

// A file system instance is a specific instance created for test purposes.
//
// These abstract base classes are derived in the shared libraries that filesystem test suites
// implement. The definitions here must have public LTO visibility to be compatible with the
// definitions in the shared libraries. (See
// https://clang.llvm.org/docs/LTOVisibility.html).
class [[clang::lto_visibility_public]] FilesystemInstance {
 public:
  FilesystemInstance() = default;
  FilesystemInstance(const FilesystemInstance&) = delete;
  FilesystemInstance& operator=(const FilesystemInstance&) = delete;
  virtual ~FilesystemInstance() = default;

  virtual zx::result<> Format(const TestFilesystemOptions&) = 0;
  virtual zx::result<> Mount(const std::string& mount_path,
                             const fs_management::MountOptions& options) = 0;
  virtual zx::result<> Unmount(const std::string& mount_path);
  virtual zx::result<> Fsck() = 0;

  // For filesystems that run on top of a block device, this returns an object that holds the ram
  // disk or nand device plus any additional drivers that might be required (e.g. FVM).
  virtual RamDevice* GetRamDevice() { return nullptr; }

  // Returns path of the device on which the filesystem is created. For filesystem that are not
  // block device based, like memfs, the function returns an error.
  virtual zx::result<std::string> DevicePath() {
    if (RamDevice* device = GetRamDevice(); device) {
      return zx::ok(std::string(device->path()));
    } else {
      return zx::error(ZX_ERR_BAD_STATE);
    }
  }
  virtual ramdevice_client::Ramdisk* GetRamDisk() {
    RamDevice* device = GetRamDevice();
    return device ? device->ram_disk() : nullptr;
  }
  virtual ramdevice_client::RamNand* GetRamNand() {
    RamDevice* device = GetRamDevice();
    return device ? device->ram_nand() : nullptr;
  }
  virtual fs_management::SingleVolumeFilesystemInterface* fs() = 0;
  virtual fidl::UnownedClientEnd<fuchsia_io::Directory> ServiceDirectory() const {
    return fidl::ClientEnd<fuchsia_io::Directory>();
  }
  virtual void Reset() {}
  virtual std::string GetMoniker() const = 0;
};

// Base class for all supported file systems. It is a factory class that generates
// instances of FilesystemInstance subclasses.
class [[clang::lto_visibility_public]] Filesystem {
 public:
  struct Traits {
    bool has_directory_size_limit = false;
    bool in_memory = false;
    bool is_case_sensitive = true;
    bool is_journaled = true;
    bool is_multi_volume = false;
    bool is_slow = false;
    int64_t max_block_size = std::numeric_limits<int64_t>::max();
    int64_t max_file_size = std::numeric_limits<int64_t>::max();
    std::string name;
    bool supports_fsck_after_every_transaction = false;
    bool supports_hard_links = true;
    bool supports_symlinks = false;
    bool supports_inspect = false;
    bool supports_mmap = false;
    bool supports_mmap_shared_write = false;
    bool supports_resize = false;
    bool supports_shutdown_on_no_connections = false;
    bool supports_sparse_files = true;
    bool supports_watch_event_deleted = true;
    zx::duration timestamp_granularity = zx::nsec(1);
    bool uses_crypt = false;
    // If true, tests will avoid creating volumes smaller than the size given by
    // TestFilesystemOptions::device_block_size * TestFilesystemOptions::device_block_count.
    bool has_min_volume_size = false;
    uint64_t inode_size = 256;
  };

  virtual ~Filesystem() = default;
  virtual zx::result<std::unique_ptr<FilesystemInstance>> Make(
      const TestFilesystemOptions& options) const = 0;
  virtual zx::result<std::unique_ptr<FilesystemInstance>> Open(
      const TestFilesystemOptions& options) const {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  virtual const Traits& GetTraits() const = 0;
};

// Template that implementations can use to gain the SharedInstance method.
template <typename T>
class FilesystemImpl : public Filesystem {
 public:
  static const T& SharedInstance() {
    static const auto* const kInstance = new T();
    return *kInstance;
  }
};

template <typename T>
class FilesystemImplWithDefaultMake : public FilesystemImpl<T> {
 public:
  virtual std::unique_ptr<FilesystemInstance> Create(RamDevice device) const = 0;

  zx::result<std::unique_ptr<FilesystemInstance>> Make(
      const TestFilesystemOptions& options) const override {
    auto device = CreateRamDevice(options);
    if (device.is_error()) {
      return device.take_error();
    }
    auto instance = Create(*std::move(device));
    zx::result<> status = instance->Format(options);
    if (status.is_error()) {
      return status.take_error();
    }
    return zx::ok(std::move(instance));
  }
};

// -- Default implementations that use fs-management --

zx::result<> FsFormat(const std::string& device_path, fs_management::FsComponent& component,
                      const fs_management::MkfsOptions& options, bool create_default_volume);

zx::result<std::pair<std::unique_ptr<fs_management::SingleVolumeFilesystemInterface>,
                     fs_management::NamespaceBinding>>
FsMount(const std::string& device_path, const std::string& mount_path,
        fs_management::FsComponent& component, const fs_management::MountOptions& mount_options);

zx::result<RamDevice> OpenRamDevice(const TestFilesystemOptions& options);

std::string StripTrailingSlash(const std::string& in);

// Removes `mount_path` from the namespace.
zx::result<> FsUnbind(const std::string& mount_path);

}  // namespace fs_test

#endif  // SRC_STORAGE_FS_TEST_FS_TEST_H_
