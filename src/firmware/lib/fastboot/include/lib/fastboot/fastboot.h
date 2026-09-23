// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_LIB_FASTBOOT_INCLUDE_LIB_FASTBOOT_FASTBOOT_H_
#define SRC_FIRMWARE_LIB_FASTBOOT_INCLUDE_LIB_FASTBOOT_FASTBOOT_H_

#include <fidl/fuchsia.fshost/cpp/wire.h>
#include <fidl/fuchsia.hardware.power.statecontrol/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.paver/cpp/wire.h>
#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/zx/result.h>
#include <stddef.h>
#include <zircon/status.h>

#include <condition_variable>
#include <mutex>
#include <optional>
#include <string>
#include <string_view>
#include <thread>
#include <unordered_map>

#include "fastboot_base.h"
#include "src/developer/sshd-host/constants.h"

namespace fastboot {

// The disk topology prefix that indicates a ramdisk.
constexpr std::string_view kRamDiskString = "/ramdisk-";

// The magic string that tells `flash gpt-meta` to initialize the default GPT.
constexpr std::string_view kGptMetaDefault = "default";

class __EXPORT Fastboot : public FastbootBase {
 public:
  // Creates a Fastboot with max download size as a portion of total system memory.
  Fastboot();
  // Creates a Fastboot with specified max download size and svc_root for testing.
  explicit Fastboot(size_t max_download_size, fidl::ClientEnd<fuchsia_io::Directory> svc_root =
                                                  fidl::ClientEnd<fuchsia_io::Directory>());

 private:
  zx::result<> ProcessCommand(std::string_view command, Transport *transport) override;
  zx::result<void *> GetDownloadBuffer(size_t total_download_size) override;
  void DoClearDownload() override;

  zx::result<> GetVar(const std::string &command, Transport *transport);
  zx::result<std::string> GetVarMaxDownloadSize(const std::vector<std::string_view> &, Transport *);
  zx::result<std::string> GetVarSlotCount(const std::vector<std::string_view> &, Transport *);
  zx::result<std::string> GetVarIsUserspace(const std::vector<std::string_view> &, Transport *);
  zx::result<std::string> GetVarHwRevision(const std::vector<std::string_view> &, Transport *);
  zx::result<std::string> GetVarProduct(const std::vector<std::string_view> &, Transport *);
  zx::result<std::string> GetVarVersion(const std::vector<std::string_view> &, Transport *);
  zx::result<std::string> GetVarAll(const std::vector<std::string_view> &, Transport *);
  zx::result<> Flash(const std::string &command, Transport *transport);
  zx::result<> Erase(const std::string &command, Transport *transport);
  zx::result<> SetActive(const std::string &command, Transport *transport);
  zx::result<> Reboot(const std::string &command, Transport *transport);
  zx::result<> RebootBootloader(const std::string &command, Transport *transport);
  zx::result<> RebootFastboot(const std::string &command, Transport *transport);
  zx::result<> RebootRecovery(const std::string &command, Transport *transport);
  zx::result<> Continue(const std::string &command, Transport *transport);
  zx::result<> OemAddStagedBootloaderFile(const std::string &command, Transport *transport);
  zx::result<> OemInitPartitionTables(const std::string &command, Transport *transport);
  zx::result<> OemInstallFromUsb(const std::string &command, Transport *transport);
  zx::result<> OemWipePartitionTables(const std::string &command, Transport *transport);
  zx::result<> OemInstallBlobImage(const std::string &command, Transport *transport);
  zx::result<> UpdateSuper(const std::string &command, Transport *transport);

  zx::result<fidl::WireSyncClient<fuchsia_paver::Paver>> ConnectToPaver();
  zx::result<fidl::WireSyncClient<fuchsia_paver::DataSink>> ConnectToDataSink(Transport *transport);
  zx::result<fidl::WireSyncClient<fuchsia_paver::DynamicDataSink>> ConnectToDynamicDataSink(
      Transport *transport);
  zx::result<fidl::WireSyncClient<fuchsia_hardware_power_statecontrol::Admin>>
  ConnectToPowerStateControl();
  zx::result<fidl::UnownedClientEnd<fuchsia_fshost::Recovery>> ConnectToRecoveryService();
  zx::result<fidl::WireSyncClient<fuchsia_paver::BootManager>> FindBootManager();
  zx::result<> HandleShutdown(Transport *transport,
                              fuchsia_hardware_power_statecontrol::ShutdownAction action);
  zx::result<> WipeUserdata(Transport *transport);
  zx::result<> WriteFirmware(fuchsia_paver::wire::Configuration config,
                             std::string_view firmware_type, Transport *transport,
                             fidl::WireSyncClient<fuchsia_paver::DataSink> &data_sink);
  zx::result<> WriteAsset(fuchsia_paver::wire::Configuration config,
                          fuchsia_paver::wire::Asset asset, Transport *transport,
                          fidl::WireSyncClient<fuchsia_paver::DataSink> &data_sink);

  struct CommandEntry {
    const char *name;
    zx::result<> (Fastboot::*cmd)(const std::string &, Transport *);
  };

  using VariableHashTable =
      std::unordered_map<std::string, zx::result<std::string> (Fastboot::*)(
                                          const std::vector<std::string_view> &, Transport *)>;

  // A static table of command name to method mapping.
  static const std::vector<CommandEntry> &GetCommandTable();

  // A static table of fastboot variable name to method mapping.
  static const VariableHashTable &GetVariableTable();

  zx::result<fidl::UnownedClientEnd<fuchsia_io::Directory>> GetSvcRoot();

  fuchsia_mem::wire::Buffer GetWireBufferFromDownload();

  friend class FastbootDownloadTest;

  /// Specifies the target when flashing the "blob" partition.
  enum class FlashBlobTarget : uint8_t {
    /// The new system blob image will be written via the fuchsia.fshost/Recovery protocol.
    /// No existing data on the device will be modified. The new system image will be installed
    /// automatically the next the the system boots up. On installation failure, existing data
    /// on the device will remain untouched.
    kBlob,
    /// The new system blob image will overwrite the entire super partition. This will cause the
    /// existing filesystem on the device to be fully overwritten.
    ///
    /// *WARNING*: All data on the device will be lost when this is set!
    kSuper,
  };

  zx::result<> PrepareFlashBlob(Transport *transport);
  zx::result<> FlashBlob(Transport *Transport);
  zx::result<> FlashSuper(Transport *transport);

  /// Holds the state persisted across chunks when flashing a new blob volume image.
  class BlobImageWriter;

  size_t max_download_size_;
  fzl::OwnedVmoMapper download_vmo_mapper_;
  // Channel to svc.
  fidl::ClientEnd<fuchsia_io::Directory> svc_root_;
  fidl::ClientEnd<fuchsia_fshost::Recovery> fshost_recovery_;
  std::unique_ptr<BlobImageWriter> blob_writer_;
  /// Used to control what the target of "flash blob" requests are.
  ///
  /// By default, flashing blob writes the new system image into a temporary volume, and completes
  /// installation on the next successful boot. This ensures that no existing volumes in the system
  /// container are modified until installation succeeds, and upon failure, no volumes are modified.
  /// When doing a full-wipe flash, we set this to true since there is no need to preserve existing
  /// volumes. This makes flashing slightly quicker, and allows us to more easily reason about the
  /// state of the device.
  FlashBlobTarget flash_blob_target_ = FlashBlobTarget::kBlob;
};

class Fastboot::BlobImageWriter {
 public:
  BlobImageWriter(fidl::ClientEnd<fuchsia_io::File> image_file, zx::eventpair mount_token,
                  zx::vmo file_vmo, fzl::OwnedVmoMapper fill_buffer, uint64_t image_size);
  ~BlobImageWriter();

  BlobImageWriter(BlobImageWriter &&) = delete;
  BlobImageWriter &operator=(BlobImageWriter &&) = delete;

  zx::vmo &file_vmo() { return file_vmo_; }
  fzl::OwnedVmoMapper &fill_buffer() { return fill_buffer_; }

  zx::result<> EnsureSize(uint64_t new_size);

  zx::result<> CheckSyncError();

  zx::result<> JoinSyncThread();

  void QueueSync();

 private:
  /// Handle to the image file in the system container. This is where the unsparsed system image
  /// containing the new blob volume should be written to.
  fidl::ClientEnd<fuchsia_io::File> image_file_;
  /// Mount token which keeps the system container mounted. The system container will be unmounted
  /// asynchronously when this is dropped. As long as the image file has been flushed to disk, it
  /// should be safe to reboot even without gracefully unmounting the filesystem.
  zx::eventpair mount_token_;
  /// VMO backing the image file.
  zx::vmo file_vmo_;
  /// Buffer used to optimize writing of fill chunks when unsparsing the system image.
  fzl::OwnedVmoMapper fill_buffer_;
  /// Unsparsed size of the image.
  uint64_t image_size_;

  std::mutex mutex_;
  std::condition_variable_any cv_;
  std::jthread sync_thread_;
  bool sync_requested_ = false;
  std::optional<zx::result<>> sync_error_ = std::nullopt;
};

}  // namespace fastboot

#endif  // SRC_FIRMWARE_LIB_FASTBOOT_INCLUDE_LIB_FASTBOOT_FASTBOOT_H_
