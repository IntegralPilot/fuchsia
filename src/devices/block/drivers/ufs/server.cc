// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "server.h"

#include <lib/driver/logging/cpp/logger.h>

#include "src/devices/block/drivers/ufs/registers.h"
#include "src/devices/block/drivers/ufs/uic/uic_commands.h"
#include "src/devices/block/drivers/ufs/upiu/attributes.h"
#include "src/devices/block/drivers/ufs/upiu/descriptors.h"
#include "src/devices/block/drivers/ufs/upiu/flags.h"

namespace ufs {

template <typename ResponseUpiu>
fit::result<QueryErrorCode, ResponseUpiu> UfsServer::HandleQueryRequestUpiu(
    QueryRequestUpiu& request) {
  auto response = controller_->GetTransferRequestProcessor().SendQueryRequestUpiu(request);
  if (response.is_error()) {
    return fit::error(QueryErrorCode::kGeneralFailure);
  }
  if (response->GetHeader().response != UpiuHeaderResponseCode::kTargetSuccess) {
    return fit::error(QueryErrorCode(response->GetHeader().response));
  }
  return fit::ok(std::move(response->GetResponse<ResponseUpiu>()));
}

template fit::result<QueryErrorCode, DescriptorResponseUpiu>
UfsServer::HandleQueryRequestUpiu<DescriptorResponseUpiu>(QueryRequestUpiu& request);

template fit::result<QueryErrorCode, FlagResponseUpiu>
UfsServer::HandleQueryRequestUpiu<FlagResponseUpiu>(QueryRequestUpiu& request);

template fit::result<QueryErrorCode, AttributeResponseUpiu>
UfsServer::HandleQueryRequestUpiu<AttributeResponseUpiu>(QueryRequestUpiu& request);

namespace {
// Extracts the 'type' from the FIDL request object, converts it to an internal type,
// and then extracts the index from the 'identifier' to return it.
// Return value:
//  - On success: Returns a std::pair containing the internal type and the index.
//  - On failure: Returns an appropriate QueryErrorCode error.
// Parameters:
//  - request: The FIDL request object.
//  - max_count: The valid range for the internal type.
template <typename RequestView, typename InternalType>
fit::result<QueryErrorCode, std::pair<InternalType, uint8_t>> GetInternalTypeAndIndex(
    const RequestView& request, InternalType max_count) {
  if (!request.has_type()) {
    fdf::error("Invalid FIDL request: missing request type");
    return fit::error(QueryErrorCode::kInvalidIdn);
  }

  // Since the DescriptorType, Flags, and Attributes enums are defined to have the same numeric
  // values as those defined in the UFS Spec for each entry in the FIDL enums DescriptorType,
  // FlagType, and AttributeType, we can convert between them by casting the underlying numeric
  // value.
  uint8_t value = fidl::ToUnderlying(request.type());
  if (value >= static_cast<uint8_t>(max_count)) {
    fdf::error("Cannot convert fidl type to internal Type");
    return fit::error(QueryErrorCode::kGeneralFailure);
  }
  uint8_t index = 0;
  if (request.has_identifier() && request.identifier().has_index()) {
    index = request.identifier().index();
  }

  return fit::ok(std::make_pair(static_cast<InternalType>(value), index));
}

}  // namespace

void UfsServer::ReadDescriptor(ReadDescriptorRequestView request,
                               ReadDescriptorCompleter::Sync& completer) {
  auto result = GetInternalTypeAndIndex<fuchsia_hardware_ufs::wire::Descriptor, DescriptorType>(
      request->descriptor, DescriptorType::kDescriptorCount);
  if (result.is_error()) {
    completer.Reply(result.take_error());
    return;
  }

  auto [type, index] = result.value();
  ReadDescriptorUpiu read_desc_upiu(type, index);
  auto response = HandleQueryRequestUpiu<DescriptorResponseUpiu>(read_desc_upiu);
  if (response.is_error()) {
    completer.Reply(response.take_error());
    return;
  }

  auto data = response.value().GetData<QueryResponseUpiuData>()->command_data;

  fidl::Arena<> arena;
  fidl::VectorView<uint8_t> desc_data(arena, data.begin(), data.end());

  completer.ReplySuccess(desc_data);
}

void UfsServer::WriteDescriptor(WriteDescriptorRequestView request,
                                WriteDescriptorCompleter::Sync& completer) {
  auto result = GetInternalTypeAndIndex<fuchsia_hardware_ufs::wire::Descriptor, DescriptorType>(
      request->descriptor, DescriptorType::kDescriptorCount);
  if (result.is_error()) {
    completer.Reply(result.take_error());
    return;
  }

  auto [type, index] = result.value();
  // Configuration Descriptor controls per-LU bBootLunID (pre-vbmeta boot
  // selection) and LU layout. Not needed by any current FIDL client.
  if (type == DescriptorType::kConfiguration) {
    fdf::error("WriteDescriptor: refusing provisioning descriptor IDN 0x{:x}",
               static_cast<uint8_t>(type));
    completer.Reply(fit::error(QueryErrorCode::kParameterNotWriteable));
    return;
  }
  if (request->data.size() < GetDescriptorSize(type)) {
    fdf::error(
        "Invalid FIDL request: descriptor data length (%zu) is smaller than descriptor size (%zu)",
        request->data.size(), GetDescriptorSize(type));
    completer.Reply(fit::error(QueryErrorCode::kGeneralFailure));
    return;
  }
  WriteDescriptorUpiu write_desc_upiu(type, request->data.data(), index);
  auto response = HandleQueryRequestUpiu<DescriptorResponseUpiu>(write_desc_upiu);
  if (response.is_error()) {
    completer.Reply(response.take_error());
    return;
  }

  completer.ReplySuccess();
}

void UfsServer::ReadFlag(ReadFlagRequestView request, ReadFlagCompleter::Sync& completer) {
  auto result = GetInternalTypeAndIndex<fuchsia_hardware_ufs::wire::Flag, Flags>(request->flag,
                                                                                 Flags::kFlagCount);
  if (result.is_error()) {
    completer.Reply(result.take_error());
    return;
  }

  auto [type, _] = result.value();
  ReadFlagUpiu read_flag_upiu(type);
  auto response = HandleQueryRequestUpiu<FlagResponseUpiu>(read_flag_upiu);
  if (response.is_error()) {
    completer.Reply(response.take_error());
    return;
  }

  completer.ReplySuccess(response.value().GetFlag());
}

void UfsServer::SetFlag(SetFlagRequestView request, SetFlagCompleter::Sync& completer) {
  auto result = GetInternalTypeAndIndex<fuchsia_hardware_ufs::wire::Flag, Flags>(request->flag,
                                                                                 Flags::kFlagCount);
  if (result.is_error()) {
    completer.Reply(result.take_error());
    return;
  }

  auto [type, _] = result.value();
  if (IsWriteOnce(type)) {
    fdf::error("SetFlag: refusing write-once flag IDN 0x{:x}", static_cast<uint8_t>(type));
    completer.Reply(fit::error(QueryErrorCode::kParameterNotWriteable));
    return;
  }
  SetFlagUpiu set_flag_upiu(type);
  auto response = HandleQueryRequestUpiu<FlagResponseUpiu>(set_flag_upiu);
  if (response.is_error()) {
    completer.Reply(response.take_error());
    return;
  }

  completer.ReplySuccess(response.value().GetFlag());
}

void UfsServer::ClearFlag(ClearFlagRequestView request, ClearFlagCompleter::Sync& completer) {
  auto result = GetInternalTypeAndIndex<fuchsia_hardware_ufs::wire::Flag, Flags>(request->flag,
                                                                                 Flags::kFlagCount);
  if (result.is_error()) {
    completer.Reply(result.take_error());
    return;
  }

  auto [type, _] = result.value();
  if (IsWriteOnce(type)) {
    fdf::error("ClearFlag: refusing write-once flag IDN 0x{:x}", static_cast<uint8_t>(type));
    completer.Reply(fit::error(QueryErrorCode::kParameterNotWriteable));
    return;
  }
  ClearFlagUpiu clear_flag_upiu(type);
  auto response = HandleQueryRequestUpiu<FlagResponseUpiu>(clear_flag_upiu);
  if (response.is_error()) {
    completer.Reply(response.take_error());
    return;
  }

  completer.ReplySuccess(response.value().GetFlag());
}

void UfsServer::ToggleFlag(ToggleFlagRequestView request, ToggleFlagCompleter::Sync& completer) {
  auto result = GetInternalTypeAndIndex<fuchsia_hardware_ufs::wire::Flag, Flags>(request->flag,
                                                                                 Flags::kFlagCount);
  if (result.is_error()) {
    completer.Reply(result.take_error());
    return;
  }

  auto [type, _] = result.value();
  if (IsWriteOnce(type)) {
    fdf::error("ToggleFlag: refusing write-once flag IDN 0x{:x}", static_cast<uint8_t>(type));
    completer.Reply(fit::error(QueryErrorCode::kParameterNotWriteable));
    return;
  }
  ToggleFlagUpiu toggle_flag_upiu(type);
  auto response = HandleQueryRequestUpiu<FlagResponseUpiu>(toggle_flag_upiu);
  if (response.is_error()) {
    completer.Reply(response.take_error());
    return;
  }

  completer.ReplySuccess(response.value().GetFlag());
}

void UfsServer::ReadAttribute(ReadAttributeRequestView request,
                              ReadAttributeCompleter::Sync& completer) {
  auto result = GetInternalTypeAndIndex<fuchsia_hardware_ufs::wire::Attribute, Attributes>(
      request->attr, Attributes::kAttributeCount);
  if (result.is_error()) {
    completer.Reply(result.take_error());
    return;
  }

  auto [type, index] = result.value();
  ReadAttributeUpiu read_attr_upiu(type, index);
  auto response = HandleQueryRequestUpiu<AttributeResponseUpiu>(read_attr_upiu);
  if (response.is_error()) {
    completer.Reply(response.take_error());
    return;
  }
  completer.ReplySuccess(response.value().GetAttribute());
}

void UfsServer::WriteAttribute(WriteAttributeRequestView request,
                               WriteAttributeCompleter::Sync& completer) {
  auto result = GetInternalTypeAndIndex<fuchsia_hardware_ufs::wire::Attribute, Attributes>(
      request->attr, Attributes::kAttributeCount);
  if (result.is_error()) {
    completer.Reply(result.take_error());
    return;
  }

  auto [type, index] = result.value();
  switch (type) {
    case Attributes::bBootLunEn:
      break;
    default:
      fdf::error("WriteAttribute: refusing unapproved attribute IDN 0x{:x}",
                 static_cast<uint8_t>(type));
      completer.Reply(fit::error(QueryErrorCode::kParameterNotWriteable));
      return;
  }
  WriteAttributeUpiu write_attr_upiu(type, request->value, index);
  auto response = HandleQueryRequestUpiu<AttributeResponseUpiu>(write_attr_upiu);
  if (response.is_error()) {
    completer.Reply(response.take_error());
    return;
  }

  completer.ReplySuccess();
}

void UfsServer::SendUicCommand(SendUicCommandRequestView request,
                               SendUicCommandCompleter::Sync& completer) {
  const auto opcode = static_cast<UicCommandOpcode>(fidl::ToUnderlying(request->opcode));

  zx::result<std::optional<uint32_t>> result = DispatchUicCommand(opcode, request);
  if (result.is_error()) {
    fdf::error("Failed to send UicCommand Opcode: 0x{:x}", static_cast<uint32_t>(opcode));
    completer.ReplyError(result.error_value());
    return;
  }
  completer.ReplySuccess(result.value().value_or(0));
}

namespace {
// External clients are non-driver components (paver, console-launcher) and must
// not be able to drive arbitrary UniPro/M-PHY attributes. Restrict DME_SET /
// DME_PEER_SET to a minimal read-mostly diagnostic allowlist; everything
// else is rejected. Power-mode / lane / gear attributes are intentionally
// excluded -- those must go through DeviceManager's UFSHCI 7.4 sequence.
constexpr bool IsClientSettableAttribute(uint16_t attribute) {
  constexpr uint16_t kAllowedAttributes[] = {PA_TActivate, PA_Granularity};
  for (uint16_t allowed : kAllowedAttributes) {
    if (attribute == allowed) {
      return true;
    }
  }
  return false;
}

zx::result<> ValidateSetCommand(uint16_t attribute, AttrSetType attr_set_type,
                                std::string_view command_name) {
  if (!IsClientSettableAttribute(attribute)) {
    fdf::error("SendUicCommand: {} attribute 0x{:x} not in allowlist", command_name, attribute);
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  if (attr_set_type != AttrSetType::kVolatile) {
    fdf::error("SendUicCommand: {} attr_set_type 0x{:x} not supported (only volatile is allowed)",
               command_name, static_cast<uint8_t>(attr_set_type));
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  return zx::ok();
}
}  // namespace

zx::result<std::optional<uint32_t>> UfsServer::DispatchUicCommand(
    UicCommandOpcode opcode, SendUicCommandRequestView request) {
  const auto arg1 = UicCommandArgument1Reg::Get().FromValue(request->argument[0]);
  const auto arg2 = UicCommandArgument2Reg::Get().FromValue(request->argument[1]);
  const auto attribute = static_cast<uint16_t>(arg1.mib_attribute());
  const auto gen_selector_index = static_cast<uint16_t>(arg1.gen_selector_index());
  const AttrSetType attr_set_type = arg2.attr_set_type();
  const uint32_t value = request->argument[2];

  switch (opcode) {
    case UicCommandOpcode::kDmeGet: {
      DmeGetUicCommand command(*controller_, attribute, gen_selector_index);
      return command.SendCommand();
    }
    case UicCommandOpcode::kDmePeerGet: {
      DmePeerGetUicCommand command(*controller_, attribute, gen_selector_index);
      return command.SendCommand();
    }
    case UicCommandOpcode::kDmeSet: {
      if (zx::result status = ValidateSetCommand(attribute, attr_set_type, "DME_SET");
          status.is_error()) {
        return status.take_error();
      }
      DmeSetUicCommand command(*controller_, attribute, gen_selector_index, AttrSetType::kVolatile,
                               value);
      return command.SendCommand();
    }
    case UicCommandOpcode::kDmePeerSet: {
      if (zx::result status = ValidateSetCommand(attribute, attr_set_type, "DME_PEER_SET");
          status.is_error()) {
        return status.take_error();
      }
      DmePeerSetUicCommand command(*controller_, attribute, gen_selector_index,
                                   AttrSetType::kVolatile, value);
      return command.SendCommand();
    }
    default:
      fdf::error("Unsupported UIC command opcode: 0x{:x}", static_cast<uint32_t>(opcode));
      return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
}

void UfsServer::ReadBuffer(ReadBufferRequestView request, ReadBufferCompleter::Sync& completer) {
  uint64_t size;
  if (request->data.get_size(&size); size < request->length) {
    completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
    return;
  }

  std::vector<uint8_t> buf(request->length);
  if (zx_status_t status = controller_->ReadBuffer(
          kPlaceholderTarget, request->lun, static_cast<uint8_t>(request->mode), request->buffer_id,
          request->buffer_offset, {.iov_base = buf.data(), .iov_len = request->length});
      status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }

  if (zx_status_t status = request->data.write(buf.data(), 0, request->length); status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }

  completer.ReplySuccess();
}

void UfsServer::WriteBuffer(WriteBufferRequestView request, WriteBufferCompleter::Sync& completer) {
  // External clients are only permitted to use standard data buffer modes.
  // Microcode/firmware download (FFU) and vendor-specific modes are rejected.
  // TODO(https://fxbug.dev/555277260): If Field Firmware Update (FFU) support is
  // needed from Fuchsia, expose it via a separate privileged capability rather
  // than the general SCSI protocol, or perform it in the bootloader instead.
  switch (request->mode) {
    case fuchsia_hardware_scsi::wire::WriteBufferMode::kData:
      break;
    default:
      fdf::error("WriteBuffer: refusing unapproved mode 0x{:x}",
                 static_cast<uint8_t>(request->mode));
      completer.ReplyError(ZX_ERR_ACCESS_DENIED);
      return;
  }

  std::vector<uint8_t> buf(request->length);
  if (zx_status_t status = request->data.read(buf.data(), 0, request->length); status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }

  if (zx_status_t status = controller_->WriteBuffer(
          kPlaceholderTarget, request->lun, static_cast<uint8_t>(request->mode), request->buffer_id,
          request->buffer_offset, {.iov_base = buf.data(), .iov_len = request->length});
      status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }

  completer.ReplySuccess();
}

}  // namespace ufs
