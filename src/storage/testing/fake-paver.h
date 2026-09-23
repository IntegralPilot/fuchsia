// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_TESTING_FAKE_PAVER_H_
#define SRC_STORAGE_TESTING_FAKE_PAVER_H_

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.paver/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/dispatcher.h>
#include <lib/sync/completion.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <threads.h>
#include <zircon/errors.h>
#include <zircon/time.h>

#include <algorithm>
#include <memory>
#include <optional>
#include <vector>

#include <fbl/auto_lock.h>
#include <fbl/mutex.h>

namespace paver_test {

enum class Command {
  kUnknown,
  kInitializeAbr,
  kQueryCurrentConfiguration,
  kQueryActiveConfiguration,
  kQueryConfigurationLastSetActive,
  kQueryConfigurationStatus,
  kQueryConfigurationStatusAndBootAttempts,
  kSetConfigurationActive,
  kSetConfigurationUnbootable,
  kSetConfigurationHealthy,
  kReadAsset,
  kWriteAsset,
  kWriteFirmware,
  kWriteOpaqueVolume,
  kWriteSparseVolume,
  kWriteDataFile,
  kInitPartitionTables,
  kWipePartitionTables,
  kDataSinkFlush,
  kBootManagerFlush,
};

struct AbrSlotData {
  bool unbootable;
  bool active;
  bool healthy;
  uint8_t boot_attempts;
};

struct AbrData {
  AbrSlotData slot_a;
  AbrSlotData slot_b;
  std::optional<fuchsia_paver::Configuration> last_set_active;
};

constexpr AbrData kInitAbrData = {
    .slot_a =
        {
            .unbootable = false,
            .active = false,
            .healthy = false,
            .boot_attempts = 0,
        },
    .slot_b =
        {
            .unbootable = false,
            .active = false,
            .healthy = false,
            .boot_attempts = 0,
        },
    .last_set_active = std::nullopt,
};

class FakePaver : public fidl::WireServer<fuchsia_paver::Paver>,
                  public fidl::WireServer<fuchsia_paver::BootManager>,
                  public fidl::WireServer<fuchsia_paver::DynamicDataSink> {
 public:
  void Connect(async_dispatcher_t* dispatcher, fidl::ServerEnd<fuchsia_paver::Paver> request);

  void FindDataSink(FindDataSinkRequestView request,
                    FindDataSinkCompleter::Sync& _completer) override;

  void FindPartitionTableManager(FindPartitionTableManagerRequestView request,
                                 FindPartitionTableManagerCompleter::Sync& _completer) override;

  void FindBootManager(FindBootManagerRequestView request,
                       FindBootManagerCompleter::Sync& _completer) override;

  void QueryCurrentConfiguration(QueryCurrentConfigurationCompleter::Sync& completer) override;

  void FindSysconfig(FindSysconfigRequestView request,
                     FindSysconfigCompleter::Sync& _completer) override;

  void QueryActiveConfiguration(QueryActiveConfigurationCompleter::Sync& completer) override;

  void QueryConfigurationLastSetActive(

      QueryConfigurationLastSetActiveCompleter::Sync& completer) override;

  void QueryConfigurationStatus(QueryConfigurationStatusRequestView request,
                                QueryConfigurationStatusCompleter::Sync& completer) override;

  void QueryConfigurationStatusAndBootAttempts(
      QueryConfigurationStatusAndBootAttemptsRequestView request,
      QueryConfigurationStatusAndBootAttemptsCompleter::Sync& completer) override;

  void SetConfigurationActive(SetConfigurationActiveRequestView request,
                              SetConfigurationActiveCompleter::Sync& completer) override;

  void SetConfigurationUnbootable(SetConfigurationUnbootableRequestView request,
                                  SetConfigurationUnbootableCompleter::Sync& completer) override;

  void SetConfigurationHealthy(SetConfigurationHealthyRequestView request,
                               SetConfigurationHealthyCompleter::Sync& completer) override;

  void SetOneShotRecovery(SetOneShotRecoveryCompleter::Sync& completer) override;

  void Flush(
      fidl::WireServer<fuchsia_paver::DynamicDataSink>::FlushCompleter::Sync& completer) override;

  void Flush(
      fidl::WireServer<fuchsia_paver::BootManager>::FlushCompleter::Sync& completer) override;

  void ReadAsset(ReadAssetRequestView request, ReadAssetCompleter::Sync& completer) override;

  void WriteAsset(WriteAssetRequestView request, WriteAssetCompleter::Sync& completer) override;

  void WriteOpaqueVolume(WriteOpaqueVolumeRequestView request,
                         WriteOpaqueVolumeCompleter::Sync& completer) override;

  void WriteSparseVolume(WriteSparseVolumeRequestView request,
                         WriteSparseVolumeCompleter::Sync& completer) override;

  void WriteFirmware(WriteFirmwareRequestView request,
                     WriteFirmwareCompleter::Sync& completer) override;

  void ReadFirmware(ReadFirmwareRequestView request,
                    ReadFirmwareCompleter::Sync& completer) override;

  void InitializePartitionTables(InitializePartitionTablesCompleter::Sync& completer) override;

  void WipePartitionTables(WipePartitionTablesCompleter::Sync& completer) override;

  void WaitForWritten(size_t size);

  std::vector<Command> GetCommandTrace();

  std::string last_firmware_type() const;
  fuchsia_paver::wire::Configuration last_firmware_config() const;
  fuchsia_paver::wire::Configuration last_asset_config() const;
  fuchsia_paver::wire::Asset last_asset() const;
  const std::string& data_file_path() const;

  void set_expected_payload_size(size_t size) { expected_payload_size_ = size; }
  void set_supported_firmware_type(std::string type);
  void set_abr_supported(bool supported) { abr_supported_ = supported; }
  void set_wait_for_start_signal(bool wait) { wait_for_start_signal_ = wait; }
  void set_expected_device(std::string expected);
  // Sets the boot attempts for the given configuration. No-op if configuration is R.
  void set_boot_attempts(fuchsia_paver::wire::Configuration configuration, uint8_t boot_attempts);

  AbrData abr_data();

  std::atomic<async_dispatcher_t*>& dispatcher() { return dispatcher_; }

 private:
  std::atomic<bool> wait_for_start_signal_ = false;
  sync_completion_t start_signal_;
  sync_completion_t done_signal_;
  std::atomic<size_t> signal_size_;

  mutable fbl::Mutex lock_;

  std::string last_firmware_type_ TA_GUARDED(lock_);
  fuchsia_paver::wire::Asset last_asset_ TA_GUARDED(lock_);
  fuchsia_paver::wire::Configuration last_firmware_config_ TA_GUARDED(lock_);
  fuchsia_paver::wire::Configuration last_asset_config_ TA_GUARDED(lock_);
  std::string data_file_path_ TA_GUARDED(lock_);

  std::atomic<size_t> expected_payload_size_ = 0;
  std::string expected_block_device_ TA_GUARDED(lock_);
  std::string supported_firmware_type_ TA_GUARDED(lock_);
  std::atomic<bool> abr_supported_ = false;
  AbrData abr_data_ TA_GUARDED(lock_) = kInitAbrData;

  std::atomic<async_dispatcher_t*> dispatcher_ = nullptr;

  std::vector<Command> command_trace_ TA_GUARDED(lock_);
  void AppendCommand(Command cmd) TA_REQ(lock_) { command_trace_.push_back(cmd); }
};

}  // namespace paver_test

#endif  // SRC_STORAGE_TESTING_FAKE_PAVER_H_
