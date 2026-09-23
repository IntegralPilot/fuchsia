// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_PAVER_PAVER_H_
#define SRC_FIRMWARE_PAVER_PAVER_H_

#include <fidl/fuchsia.mem/cpp/wire.h>
#include <fidl/fuchsia.paver/cpp/wire.h>
#include <lib/async/dispatcher.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include <utility>
#include <variant>

#include <fbl/mutex.h>
#include <fbl/string.h>
#include <fbl/unique_fd.h>
#include <sdk/lib/syslog/cpp/macros.h>

#include "abr-client.h"
#include "block-devices.h"
#include "device-partitioner.h"
#include "paver-context.h"

namespace paver {

#include "src/firmware/paver/config.h"

class Paver : public fidl::WireServer<fuchsia_paver::Paver> {
 public:
  void FindDataSink(FindDataSinkRequestView request,
                    FindDataSinkCompleter::Sync& completer) override;

  void FindPartitionTableManager(FindPartitionTableManagerRequestView request,
                                 FindPartitionTableManagerCompleter::Sync& completer) override;

  void FindBootManager(FindBootManagerRequestView request,
                       FindBootManagerCompleter::Sync& completer) override;

  void FindSysconfig(FindSysconfigRequestView request,
                     FindSysconfigCompleter::Sync& completer) override;

  void FindSysconfig(fidl::ServerEnd<fuchsia_paver::Sysconfig> sysconfig);

  void set_dispatcher(async_dispatcher_t* dispatcher) { dispatcher_ = dispatcher; }
  void set_svc_root(fidl::ClientEnd<fuchsia_io::Directory> svc_root) {
    svc_root_ = std::move(svc_root);
  }

  const BlockDevices& devices() const { return devices_; }

  Paver(PaverConfig config, BlockDevices devices, fidl::ClientEnd<fuchsia_io::Directory> svc_root)
      : config_(config),
        devices_(std::move(devices)),
        svc_root_(std::move(svc_root)),
        context_(std::make_shared<Context>()) {}

  static zx::result<std::unique_ptr<Paver>> Create(PaverConfig config = {},
                                                   fbl::unique_fd devfs_root = {});

  void LifecycleStopCallback(fit::callback<void(zx_status_t status)> cb);

 private:
  const PaverConfig config_;
  // Used for test injection.
  BlockDevices devices_;
  fidl::ClientEnd<fuchsia_io::Directory> svc_root_;

  async_dispatcher_t* dispatcher_ = nullptr;

  // Declared as shared_ptr to avoid life time issues (i.e. Paver exiting before the created device
  // partitioners).
  std::shared_ptr<Context> context_;
};

// Common shared implementation for DataSink and DynamicDataSink. Necessary to work around lack of
// "is-a" relationship in llcpp bindings.
class DataSinkImpl {
 public:
  DataSinkImpl(BlockDevices devices, std::unique_ptr<DevicePartitioner> partitioner)
      : devices_(std::move(devices)), partitioner_(std::move(partitioner)) {}

  zx::result<fuchsia_mem::wire::Buffer> ReadAsset(fuchsia_paver::wire::Configuration configuration,
                                                  fuchsia_paver::wire::Asset asset);

  zx::result<> WriteAsset(fuchsia_paver::wire::Configuration configuration,
                          fuchsia_paver::wire::Asset asset, fuchsia_mem::wire::Buffer payload);

  zx::result<> WriteOpaqueVolume(fuchsia_mem::wire::Buffer payload);

  zx::result<> WriteSparseVolume(fuchsia_mem::wire::Buffer payload);

  // FIDL llcpp unions don't currently support memory ownership so we need to
  // return something that does own the underlying memory.
  //
  // Once unions do support owned memory we can just return
  // WriteBootloaderResult directly here.
  std::variant<zx_status_t, bool> WriteFirmware(fuchsia_paver::wire::Configuration configuration,
                                                fidl::StringView type,
                                                fuchsia_mem::wire::Buffer payload);

  zx::result<fuchsia_mem::wire::Buffer> ReadFirmware(
      fuchsia_paver::wire::Configuration configuration, fidl::StringView type);

  DevicePartitioner* partitioner() { return partitioner_.get(); }

 private:
  // Used for test injection.
  BlockDevices devices_;

  std::unique_ptr<DevicePartitioner> partitioner_;

  // A helper to get firmware partition spec.
  std::optional<PartitionSpec> GetFirmwarePartitionSpec(
      fuchsia_paver::wire::Configuration configuration, fidl::StringView type);
};

class DataSink : public fidl::WireServer<fuchsia_paver::DataSink> {
 public:
  DataSink(BlockDevices devices, std::unique_ptr<DevicePartitioner> partitioner)
      : sink_(std::move(devices), std::move(partitioner)) {}

  // Automatically finds block device to use.
  static void Bind(async_dispatcher_t* dispatcher, BlockDevices devices,
                   fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
                   const PaverConfig& config, fidl::ServerEnd<fuchsia_paver::DataSink> server,
                   std::shared_ptr<Context> context);

  void ReadAsset(ReadAssetRequestView request, ReadAssetCompleter::Sync& completer) override;

  void WriteAsset(WriteAssetRequestView request, WriteAssetCompleter::Sync& completer) override {
    completer.Reply(
        sink_.WriteAsset(request->configuration, request->asset, std::move(request->payload))
            .status_value());
  }

  void WriteOpaqueVolume(WriteOpaqueVolumeRequestView request,
                         WriteOpaqueVolumeCompleter::Sync& completer) override {
    zx::result<> res = sink_.WriteOpaqueVolume(std::move(request->payload));
    if (res.is_ok()) {
      completer.ReplySuccess();
    } else {
      completer.ReplyError(res.status_value());
    }
  }

  void WriteSparseVolume(WriteSparseVolumeRequestView request,
                         WriteSparseVolumeCompleter::Sync& completer) override {
    zx::result<> res = sink_.WriteSparseVolume(std::move(request->payload));
    if (res.is_ok()) {
      completer.ReplySuccess();
    } else {
      completer.ReplyError(res.status_value());
    }
  }

  void WriteFirmware(WriteFirmwareRequestView request,
                     WriteFirmwareCompleter::Sync& completer) override;

  void ReadFirmware(ReadFirmwareRequestView request,
                    ReadFirmwareCompleter::Sync& completer) override;

  void Flush(FlushCompleter::Sync& completer) override {
    completer.Reply(sink_.partitioner()->Flush().status_value());
  }

 private:
  DataSinkImpl sink_;
};

class DynamicDataSink : public fidl::WireServer<fuchsia_paver::DynamicDataSink> {
 public:
  DynamicDataSink(BlockDevices devices, std::unique_ptr<DevicePartitioner> partitioner)
      : sink_(std::move(devices), std::move(partitioner)) {}

  static void Bind(async_dispatcher_t* dispatcher, BlockDevices devices,
                   fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
                   const PaverConfig& config,
                   fidl::ServerEnd<fuchsia_paver::DynamicDataSink> server,
                   std::shared_ptr<Context> context);

  void InitializePartitionTables(InitializePartitionTablesCompleter::Sync& completer) override;

  void WipePartitionTables(WipePartitionTablesCompleter::Sync& completer) override;

  void ReadAsset(ReadAssetRequestView request, ReadAssetCompleter::Sync& completer) override;

  void WriteAsset(WriteAssetRequestView request, WriteAssetCompleter::Sync& completer) override {
    completer.Reply(
        sink_.WriteAsset(request->configuration, request->asset, std::move(request->payload))
            .status_value());
  }

  void WriteOpaqueVolume(WriteOpaqueVolumeRequestView request,
                         WriteOpaqueVolumeCompleter::Sync& completer) override {
    zx::result<> res = sink_.WriteOpaqueVolume(std::move(request->payload));
    if (res.is_ok()) {
      completer.ReplySuccess();
    } else {
      completer.ReplyError(res.status_value());
    }
  }

  void WriteSparseVolume(WriteSparseVolumeRequestView request,
                         WriteSparseVolumeCompleter::Sync& completer) override {
    zx::result<> res = sink_.WriteSparseVolume(std::move(request->payload));
    if (res.is_ok()) {
      completer.ReplySuccess();
    } else {
      completer.ReplyError(res.status_value());
    }
  }

  void WriteFirmware(WriteFirmwareRequestView request,
                     WriteFirmwareCompleter::Sync& completer) override;

  void ReadFirmware(ReadFirmwareRequestView request,
                    ReadFirmwareCompleter::Sync& completer) override;

  void Flush(FlushCompleter::Sync& completer) override {
    completer.Reply(sink_.partitioner()->Flush().status_value());
  }

 private:
  DataSinkImpl sink_;
};

class BootManager : public fidl::WireServer<fuchsia_paver::BootManager> {
 public:
  BootManager(std::unique_ptr<DevicePartitioner> partitioner,
              std::unique_ptr<abr::Client> abr_client, const PaverConfig& config)
      : partitioner_(std::move(partitioner)), abr_client_(std::move(abr_client)), config_(config) {}

  static void Bind(async_dispatcher_t* dispatcher, BlockDevices devices,
                   fidl::ClientEnd<fuchsia_io::Directory> svc_root, const PaverConfig& config,
                   std::shared_ptr<Context> context,
                   fidl::ServerEnd<fuchsia_paver::BootManager> server);

  void QueryCurrentConfiguration(QueryCurrentConfigurationCompleter::Sync& completer) override;

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

  void Flush(FlushCompleter::Sync& completer) override {
    completer.Reply(abr_client_->Flush().status_value());
  }

 private:
  std::unique_ptr<DevicePartitioner> partitioner_;
  std::unique_ptr<abr::Client> abr_client_;
  const PaverConfig& config_;

  // Returns true if we are currently executing the final boot attempt on the given slot.
  bool IsFinalBootAttempt(const AbrSlotInfo& slot_info,
                          fuchsia_paver::wire::Configuration configuration);

  // Returns the current state for the given `configuration`.
  struct ConfigurationState {
    fuchsia_paver::wire::ConfigurationStatus status;
    std::optional<uint8_t> boot_attempts;
    std::optional<fuchsia_paver::wire::UnbootableReason> unbootable_reason;
  };
  zx::result<ConfigurationState> GetConfigurationState(
      fuchsia_paver::wire::Configuration configuration);
};

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_PAVER_H_
