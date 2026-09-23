// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "fake-paver.h"

#include <fidl/fuchsia.paver/cpp/common_types.h>
#include <lib/fzl/vmo-mapper.h>
#include <zircon/errors.h>

namespace paver_test {

void FakePaver::Connect(async_dispatcher_t* dispatcher,
                        fidl::ServerEnd<fuchsia_paver::Paver> request) {
  dispatcher_ = dispatcher;
  fidl::BindServer(dispatcher, std::move(request), this);
}

void FakePaver::FindDataSink(FindDataSinkRequestView request,
                             FindDataSinkCompleter::Sync& _completer) {
  fidl::BindServer(
      dispatcher_,
      fidl::ServerEnd<fuchsia_paver::DynamicDataSink>(request->data_sink.TakeChannel()), this);
}

void FakePaver::FindPartitionTableManager(FindPartitionTableManagerRequestView request,
                                          FindPartitionTableManagerCompleter::Sync& _completer) {
  fidl::BindServer(
      dispatcher_,
      fidl::ServerEnd<fuchsia_paver::DynamicDataSink>(request->data_sink.TakeChannel()), this);
}

void FakePaver::FindBootManager(FindBootManagerRequestView request,
                                FindBootManagerCompleter::Sync& _completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kInitializeAbr);
  if (abr_supported_) {
    fidl::BindServer(dispatcher_, std::move(request->boot_manager), this);
  }
}

void FakePaver::QueryCurrentConfiguration(QueryCurrentConfigurationCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kQueryCurrentConfiguration);
  completer.ReplySuccess(fuchsia_paver::wire::Configuration::kA);
}

void FakePaver::FindSysconfig(FindSysconfigRequestView request,
                              FindSysconfigCompleter::Sync& _completer) {}

void FakePaver::QueryActiveConfiguration(QueryActiveConfigurationCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kQueryActiveConfiguration);

  // This is not quite the same logic as the paver uses, but for testing
  // purposes it is simpler and should be equivalent.
  // See:
  // https://cs.opensource.google/fuchsia/fuchsia/+/refs/heads/main:src/firmware/lib/abr/flow.c;l=80;drc=d0e362718c30f2e490c2e84607b6a37579058a17
  bool slot_a_active = abr_data_.slot_a.active && !abr_data_.slot_a.unbootable;
  bool slot_b_active = abr_data_.slot_b.active && !abr_data_.slot_b.unbootable;

  if (slot_a_active) {
    completer.ReplySuccess(fuchsia_paver::Configuration::kA);
    return;
  }

  if (slot_b_active) {
    completer.ReplySuccess(fuchsia_paver::Configuration::kB);
    return;
  }

  completer.ReplySuccess(fuchsia_paver::Configuration::kRecovery);
}

void FakePaver::QueryConfigurationLastSetActive(
    QueryConfigurationLastSetActiveCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kQueryConfigurationLastSetActive);
  if (abr_data_.last_set_active.has_value()) {
    completer.ReplySuccess(abr_data_.last_set_active.value());
  } else {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
  }
}

namespace {

// Returns the `ConfigurationStatus` and boot attempts for the requested `configuration`.
// The boot attempts is only valid for configuration status == `kPending`, and will be 0 otherwise.
zx::result<std::pair<fuchsia_paver::wire::ConfigurationStatus, uint8_t>> GetConfigurationSlotData(
    const AbrData& abr_data, fuchsia_paver::wire::Configuration configuration) {
  AbrSlotData slot_data;
  switch (configuration) {
    case fuchsia_paver::wire::Configuration::kA:
      slot_data = abr_data.slot_a;
      break;

    case fuchsia_paver::wire::Configuration::kB:
      slot_data = abr_data.slot_b;
      break;

    case fuchsia_paver::wire::Configuration::kRecovery:
      return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (slot_data.unbootable) {
    return zx::ok(std::make_pair(fuchsia_paver::wire::ConfigurationStatus::kUnbootable, 0));
  }
  if (!slot_data.healthy) {
    return zx::ok(std::make_pair(fuchsia_paver::wire::ConfigurationStatus::kPending,
                                 slot_data.boot_attempts));
  }
  return zx::ok(std::make_pair(fuchsia_paver::wire::ConfigurationStatus::kHealthy, 0));
}

}  // namespace

void FakePaver::QueryConfigurationStatus(QueryConfigurationStatusRequestView request,
                                         QueryConfigurationStatusCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kQueryConfigurationStatus);

  auto slot_data = GetConfigurationSlotData(abr_data_, request->configuration);
  if (slot_data.is_error()) {
    completer.ReplyError(slot_data.error_value());
    return;
  }
  completer.ReplySuccess(slot_data->first);
}

void FakePaver::QueryConfigurationStatusAndBootAttempts(
    QueryConfigurationStatusAndBootAttemptsRequestView request,
    QueryConfigurationStatusAndBootAttemptsCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kQueryConfigurationStatusAndBootAttempts);

  auto slot_data = GetConfigurationSlotData(abr_data_, request->configuration);
  if (slot_data.is_error()) {
    completer.ReplyError(slot_data.error_value());
    return;
  }

  fidl::Arena arena;
  auto builder =
      fuchsia_paver::wire::BootManagerQueryConfigurationStatusAndBootAttemptsResponse::Builder(
          arena);

  const auto& [status, boot_attempts] = *slot_data;
  builder.status(status);
  if (status == fuchsia_paver::wire::ConfigurationStatus::kPending) {
    builder.boot_attempts(boot_attempts);
  }
  completer.ReplySuccess(builder.Build());
}

void FakePaver::SetConfigurationActive(SetConfigurationActiveRequestView request,
                                       SetConfigurationActiveCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kSetConfigurationActive);
  zx_status_t status;
  switch (request->configuration) {
    case fuchsia_paver::wire::Configuration::kA:
      abr_data_.slot_a.active = true;
      abr_data_.slot_b.active = false;

      abr_data_.slot_a.unbootable = false;
      abr_data_.slot_a.healthy = false;
      abr_data_.slot_a.boot_attempts = 0;
      abr_data_.last_set_active = fuchsia_paver::Configuration::kA;
      status = ZX_OK;
      break;

    case fuchsia_paver::wire::Configuration::kB:
      abr_data_.slot_b.active = true;
      abr_data_.slot_a.active = false;

      abr_data_.slot_b.unbootable = false;
      abr_data_.slot_b.healthy = false;
      abr_data_.slot_b.boot_attempts = 0;
      abr_data_.last_set_active = fuchsia_paver::Configuration::kB;
      status = ZX_OK;
      break;

    case fuchsia_paver::wire::Configuration::kRecovery:
      status = ZX_ERR_INVALID_ARGS;
      break;
  }
  completer.Reply(status);
}

void FakePaver::SetConfigurationUnbootable(SetConfigurationUnbootableRequestView request,
                                           SetConfigurationUnbootableCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kSetConfigurationUnbootable);
  zx_status_t status;
  switch (request->configuration) {
    case fuchsia_paver::wire::Configuration::kA:
      abr_data_.slot_a.unbootable = true;
      abr_data_.slot_a.healthy = false;
      status = ZX_OK;
      break;

    case fuchsia_paver::wire::Configuration::kB:
      abr_data_.slot_b.unbootable = true;
      abr_data_.slot_b.healthy = false;
      status = ZX_OK;
      break;

    case fuchsia_paver::wire::Configuration::kRecovery:
      status = ZX_ERR_INVALID_ARGS;
      break;
  }
  completer.Reply(status);
}

void FakePaver::SetConfigurationHealthy(SetConfigurationHealthyRequestView request,
                                        SetConfigurationHealthyCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kSetConfigurationHealthy);
  zx_status_t status;
  switch (request->configuration) {
    case fuchsia_paver::wire::Configuration::kA:
      abr_data_.slot_a.unbootable = false;
      abr_data_.slot_a.healthy = true;
      status = ZX_OK;
      break;

    case fuchsia_paver::wire::Configuration::kB:
      abr_data_.slot_b.unbootable = false;
      abr_data_.slot_b.healthy = true;
      status = ZX_OK;
      break;

    case fuchsia_paver::wire::Configuration::kRecovery:
      status = ZX_ERR_INVALID_ARGS;
      break;
  }
  completer.Reply(status);
}

void FakePaver::SetOneShotRecovery(SetOneShotRecoveryCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void FakePaver::Flush(
    fidl::WireServer<fuchsia_paver::DynamicDataSink>::FlushCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kDataSinkFlush);
  completer.Reply(ZX_OK);
}

void FakePaver::Flush(
    fidl::WireServer<fuchsia_paver::BootManager>::FlushCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kBootManagerFlush);
  completer.Reply(ZX_OK);
}

void FakePaver::ReadAsset(ReadAssetRequestView request, ReadAssetCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kReadAsset);
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void FakePaver::WriteAsset(WriteAssetRequestView request, WriteAssetCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kWriteAsset);
  auto status = request->payload.size == expected_payload_size_ ? ZX_OK : ZX_ERR_INVALID_ARGS;
  last_asset_ = request->asset;
  last_asset_config_ = request->configuration;
  completer.Reply(status);
}

void FakePaver::WriteOpaqueVolume(WriteOpaqueVolumeRequestView request,
                                  WriteOpaqueVolumeCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kWriteOpaqueVolume);
  if (request->payload.size == expected_payload_size_) {
    completer.ReplySuccess();
  } else {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
  }
}

void FakePaver::WriteSparseVolume(WriteSparseVolumeRequestView request,
                                  WriteSparseVolumeCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kWriteSparseVolume);
  if (request->payload.size == expected_payload_size_) {
    completer.ReplySuccess();
  } else {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
  }
}

void FakePaver::WriteFirmware(WriteFirmwareRequestView request,
                              WriteFirmwareCompleter::Sync& completer) {
  using fuchsia_paver::wire::WriteFirmwareResult;
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kWriteFirmware);
  last_firmware_type_ = std::string(request->type.data(), request->type.size());
  last_firmware_config_ = request->configuration;

  // Reply varies depending on whether we support |type| or not.
  if (supported_firmware_type_ == std::string_view(request->type.data(), request->type.size())) {
    auto status = request->payload.size == expected_payload_size_ ? ZX_OK : ZX_ERR_INVALID_ARGS;
    completer.Reply(WriteFirmwareResult::WithStatus(status));
  } else {
    completer.Reply(WriteFirmwareResult::WithUnsupported(true));
  }
}

void FakePaver::ReadFirmware(ReadFirmwareRequestView request,
                             ReadFirmwareCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void FakePaver::InitializePartitionTables(InitializePartitionTablesCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kInitPartitionTables);
  completer.Reply(ZX_OK);
}

void FakePaver::WipePartitionTables(WipePartitionTablesCompleter::Sync& completer) {
  fbl::AutoLock al(&lock_);
  AppendCommand(Command::kWipePartitionTables);
  completer.Reply(ZX_OK);
}

void FakePaver::WaitForWritten(size_t size) {
  signal_size_ = size;
  sync_completion_signal(&start_signal_);
  sync_completion_wait(&done_signal_, ZX_TIME_INFINITE);
  sync_completion_reset(&done_signal_);
}

std::vector<Command> FakePaver::GetCommandTrace() {
  fbl::AutoLock al(&lock_);
  return command_trace_;
}

std::string FakePaver::last_firmware_type() const {
  fbl::AutoLock al(&lock_);
  return last_firmware_type_;
}

fuchsia_paver::wire::Configuration FakePaver::last_firmware_config() const {
  fbl::AutoLock al(&lock_);
  return last_firmware_config_;
}

fuchsia_paver::wire::Configuration FakePaver::last_asset_config() const {
  fbl::AutoLock al(&lock_);
  return last_asset_config_;
}

fuchsia_paver::wire::Asset FakePaver::last_asset() const {
  fbl::AutoLock al(&lock_);
  return last_asset_;
}

const std::string& FakePaver::data_file_path() const {
  fbl::AutoLock al(&lock_);
  return data_file_path_;
}

void FakePaver::set_supported_firmware_type(std::string type) {
  fbl::AutoLock al(&lock_);
  supported_firmware_type_ = std::move(type);
}

void FakePaver::set_expected_device(std::string expected) {
  fbl::AutoLock al(&lock_);
  expected_block_device_ = std::move(expected);
}

void FakePaver::set_boot_attempts(fuchsia_paver::wire::Configuration configuration,
                                  uint8_t boot_attempts) {
  fbl::AutoLock al(&lock_);
  switch (configuration) {
    case fuchsia_paver::wire::Configuration::kA:
      abr_data_.slot_a.boot_attempts = boot_attempts;
      break;

    case fuchsia_paver::wire::Configuration::kB:
      abr_data_.slot_b.boot_attempts = boot_attempts;
      break;

    case fuchsia_paver::wire::Configuration::kRecovery:
      break;
  }
}

AbrData FakePaver::abr_data() {
  fbl::AutoLock al(&lock_);
  return abr_data_;
}

}  // namespace paver_test
