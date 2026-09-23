// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "fastboot.h"

#include <ctype.h>
#include <lib/abr/abr.h>
#include <lib/fastboot/fastboot_base.h>
#include <lib/storage/gpt_utils.h>
#include <lib/storage/sparse.h>
#include <lib/storage/storage.h>
#include <lib/zbi/zbi.h>
#include <lib/zircon_boot/zircon_boot.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <xefi.h>

#include <algorithm>
#include <numeric>
#include <string>

#include <efi/string/string.h>
#include <efi/types.h>
#include <fbl/vector.h>
#include <phys/efi/main.h>
#include <pretty/hexdump.h>

#include "boot_zbi_items.h"
#include "gpt.h"
#include "lib/zx/result.h"
#include "utils.h"

namespace gigaboot {

static constexpr std::optional<AbrSlotIndex> ParseAbrSlotStr(std::string_view str, bool allow_r) {
  if (str == "a") {
    return kAbrSlotIndexA;
  } else if (str == "b") {
    return kAbrSlotIndexB;
  } else if (allow_r && str == "r") {
    return kAbrSlotIndexR;
  } else {
    return std::nullopt;
  }
}

zx::result<> Fastboot::ProcessCommand(std::string_view cmd, fastboot::Transport *transport) {
  auto cmd_table = GetCommandCallbackTable();
  for (const CommandCallbackEntry &ele : cmd_table) {
    if (MatchCommand(cmd, ele.name.data())) {
      return (this->*(ele.cmd))(cmd, transport);
    }
  }
  return SendResponse(ResponseType::kFail, "Unsupported command", transport);
}

void Fastboot::DoClearDownload() {}

zx::result<void *> Fastboot::GetDownloadBuffer(size_t total_download_size) {
  if (total_download_size > download_buffer_.size()) {
    return zx::error(ZX_ERR_BUFFER_TOO_SMALL);
  }
  return zx::ok(download_buffer_.data());
}

constexpr std::string_view kSlotArgs[] = {"a", "b"};

std::span<Fastboot::VariableEntry> Fastboot::GetVariableTable() {
  static VariableEntry var_entries[] = {
      {"all", VarFuncAndArgs{&Fastboot::GetVarAll}},
      // Function based variables
      {"max-download-size", VarFuncAndArgs{&Fastboot::GetVarMaxDownloadSize}},
      {"current-slot", VarFuncAndArgs{&Fastboot::GetVarCurrentSlot}},
      {"slot-last-set-active", VarFuncAndArgs{&Fastboot::GetVarSlotLastSetActive}},
      {"slot-retry-count", VarFuncAndArgs{&Fastboot::GetVarSlotRetryCount, kSlotArgs}},
      {"slot-successful", VarFuncAndArgs{&Fastboot::GetVarSlotSuccessful, kSlotArgs}},
      {"slot-unbootable", VarFuncAndArgs{&Fastboot::GetVarSlotUnbootable, kSlotArgs}},
      {"slot-unbootable-reason", VarFuncAndArgs{&Fastboot::GetVarSlotUnbootableReason, kSlotArgs}},
      // Constant based variables
      {"slot-count", {"2"}},
      {"slot-suffixes", {"a,b"}},
      {"hw-revision", {BOARD_NAME}},
      {"version", {"0.4"}},
  };

  return var_entries;
}

std::span<Fastboot::CommandCallbackEntry> Fastboot::GetCommandCallbackTable() {
  static CommandCallbackEntry cmd_entries[] = {
      {"getvar", &Fastboot::GetVar},
      {"flash", &Fastboot::Flash},
      {"boot", &Fastboot::BootRam},
      {"continue", &Fastboot::Continue},
      {"reboot", &Fastboot::Reboot},
      {"reboot-bootloader", &Fastboot::RebootBootloader},
      {"reboot-recovery", &Fastboot::RebootRecovery},
      {"set_active", &Fastboot::SetActive},
      {"oem gpt-init", &Fastboot::GptInit},
      {"oem add-staged-bootloader-file", &Fastboot::OemAddStagedBootloaderFile},
      {"oem efi-getvarinfo", &Fastboot::EfiGetVarInfo},
      {"oem efi-getvarnames", &Fastboot::EfiGetVarNames},
      {"oem efi-getvar", &Fastboot::EfiGetVar},
      {"oem efi-dumpvars", &Fastboot::EfiDumpVars},
  };

  return cmd_entries;
}

zx::result<> Fastboot::Reboot(std::string_view cmd, fastboot::Transport *transport) {
  return DoReboot(RebootMode::kNormal, cmd, transport);
}

zx::result<> Fastboot::RebootBootloader(std::string_view cmd, fastboot::Transport *transport) {
  return DoReboot(RebootMode::kBootloader, cmd, transport);
}

zx::result<> Fastboot::RebootRecovery(std::string_view cmd, fastboot::Transport *transport) {
  return DoReboot(RebootMode::kRecovery, cmd, transport);
}

zx::result<> Fastboot::DoReboot(RebootMode reboot_mode, std::string_view cmd,
                                fastboot::Transport *transport) {
  if (!SetRebootMode(reboot_mode)) {
    return SendResponse(ResponseType::kFail, "Failed to set reboot mode", transport,
                        zx::error(ZX_ERR_INTERNAL));
  }

  // ResetSystem() below won't return. Thus sends a OKAY response first in case we succeed.
  zx::result<> res = SendResponse(ResponseType::kOkay, "", transport);
  if (res.is_error()) {
    return res;
  }

  efi_status status =
      gEfiSystemTable->RuntimeServices->ResetSystem(EfiResetCold, EFI_SUCCESS, 0, NULL);
  if (status != EFI_SUCCESS) {
    printf("Failed to reboot: %s\n", EfiStatusToString(status));
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok();
}

zx::result<> Fastboot::SetActive(std::string_view cmd, fastboot::Transport *transport) {
  CommandArgs args;
  ExtractCommandArgs(cmd, ":", args);

  if (args.num_args < 2) {
    return SendResponse(ResponseType::kFail, "missing slot name", transport);
  }

  std::optional<AbrSlotIndex> idx = ParseAbrSlotStr(args.args[1], false);
  if (!idx) {
    return SendResponse(ResponseType::kFail, "slot name is invalid", transport);
  }

  AbrOps abr_ops = GetAbrOps();
  AbrResult res = AbrMarkSlotActive(&abr_ops, *idx);
  if (res != kAbrResultOk) {
    return SendResponse(ResponseType::kFail, "Failed to set slot", transport,
                        zx::error(ZX_ERR_INTERNAL));
  }

  return SendResponse(ResponseType::kOkay, "", transport);
}

// Used to allow multiple single-type lambdas in std::visit
// instead of a single lambda with multiple constexpr if branches
template <class... Ts>
struct overload : Ts... {
  using Ts::operator()...;
};
template <class... Ts>
overload(Ts...) -> overload<Ts...>;

zx::result<> Fastboot::GetVar(std::string_view cmd, fastboot::Transport *transport) {
  CommandArgs args;
  ExtractCommandArgs(cmd, ":", args);
  if (args.num_args < 2) {
    return SendResponse(ResponseType::kFail, "Not enough arguments", transport);
  }
  auto var_table = GetVariableTable();
  for (const VariableEntry &ele : var_table) {
    if (args.args[1] == ele.name) {
      return std::visit(overload{
                            [this, &args, transport](const VarFuncAndArgs &entry) {
                              return (this->*(entry.func))(args, transport, DefaultResponder);
                            },
                            [transport](std::string_view arg) {
                              return SendResponse(ResponseType::kOkay, arg, transport);
                            },
                        },
                        ele.var);
    }
  }

  return SendResponse(ResponseType::kFail, "Unknown variable", transport);
}

// Inspired by python's str.join()
// Take a span of string_views
// (really could be any iterable/range of string-like objects),
// a delimiter character, and an output buffer, and put into the output buffer
// the concatenation of all the input strings, in order, with a single delimiter
// between each string.
//
// Note: empty strings are not treated specially. If any input strings are empty,
// the output will contain consecutive delimiters.
constexpr size_t StringJoin(std::span<const std::string_view> names, char delimiter,
                            std::span<char> buffer) {
  constexpr auto len_reducer = [](size_t sum, std::string_view val) { return sum + val.size(); };
  size_t total_length = std::reduce(names.begin(), names.end(), 0, len_reducer) + names.size() - 1;
  ZX_ASSERT(total_length <= buffer.size());

  size_t pos = 0;
  for (auto n : names) {
    pos += n.copy(&buffer[pos], n.size());
    // Don't add a trailing separator for the last string
    if (pos < total_length) {
      buffer[pos++] = delimiter;
    }
  }

  return total_length;
}

class GetVarAllResponder {
 public:
  explicit GetVarAllResponder(fbl::Vector<std::string_view> &args) : args_(args) {}

  zx::result<> operator()(Fastboot::ResponseType resp, std::string_view msg,
                          fastboot::Transport *transport) {
    if (resp == Fastboot::ResponseType::kFail) {
      return Fastboot::SendResponse(resp, msg, transport, zx::error(ZX_ERR_INTERNAL));
    }

    args_.push_back(msg);
    char buffer[fastboot::kMaxCommandPacketSize - sizeof("INFO")];
    std::span<char> buffer_span(buffer, sizeof(buffer));
    size_t payload_len = StringJoin(args_, ':', buffer_span);
    args_.pop_back();
    buffer_span = {buffer, payload_len};

    return Fastboot::SendResponse(Fastboot::ResponseType::kInfo,
                                  std::string_view(buffer_span.data(), buffer_span.size()),
                                  transport);
  }

 private:
  fbl::Vector<std::string_view> &args_;
};

zx::result<> Fastboot::GetVarAll(const CommandArgs &, fastboot::Transport *transport,
                                 const Responder &resp) {
  for (const auto &var_entry : GetVariableTable()) {
    if (var_entry.name == "all") {
      continue;
    }

    fbl::Vector<std::string_view> strs = {var_entry.name};
    GetVarAllResponder responder(strs);

    if (std::holds_alternative<std::string_view>(var_entry.var)) {
      zx::result result =
          responder(ResponseType::kInfo, std::get<std::string_view>(var_entry.var), transport);
      if (result.is_error()) {
        return result;
      }
    } else if (std::holds_alternative<VarFuncAndArgs>(var_entry.var)) {
      const VarFuncAndArgs &vfa = std::get<VarFuncAndArgs>(var_entry.var);
      CommandArgs args = {.args = {"getvar", var_entry.name}, .num_args = 2};

      if (vfa.arg_list.empty()) {
        zx::result result = (this->*(vfa.func))(args, transport, responder);
        if (result.is_error()) {
          return result;
        }
      } else {
        args.num_args = 3;
        for (auto const arg : vfa.arg_list) {
          args.args[2] = arg;
          strs.push_back(arg);
          zx::result result = (this->*(vfa.func))(args, transport, responder);
          strs.pop_back();
          if (result.is_error()) {
            return result;
          }
        }
      }
    } else {
      ZX_ASSERT_MSG(false, "non-exhaustive visitation!");
    }
  }

  return resp(ResponseType::kOkay, "", transport);
}

zx::result<> Fastboot::GetVarMaxDownloadSize(const CommandArgs &, fastboot::Transport *transport,
                                             const Responder &resp) {
  char size_str[16] = {0};
  snprintf(size_str, sizeof(size_str), "0x%08zx", download_buffer_.size());
  return resp(ResponseType::kOkay, size_str, transport);
}

zx::result<> Fastboot::GetVarCurrentSlot(const CommandArgs &, fastboot::Transport *transport,
                                         const Responder &resp) {
  AbrOps abr_ops = GetAbrOps();

  char const *slot_str;
  AbrSlotIndex slot = AbrGetBootSlot(&abr_ops, false, nullptr);
  switch (slot) {
    case kAbrSlotIndexA:
      slot_str = "a";
      break;
    case kAbrSlotIndexB:
      slot_str = "b";
      break;
    case kAbrSlotIndexR:
      slot_str = "r";
      break;
    default:
      slot_str = "";
      break;
  }

  return resp(ResponseType::kOkay, slot_str, transport);
}

zx::result<> Fastboot::GetVarSlotLastSetActive(const CommandArgs &, fastboot::Transport *transport,
                                               const Responder &resp) {
  AbrOps abr_ops = GetAbrOps();
  AbrSlotIndex slot;
  AbrResult res = AbrGetSlotLastMarkedActive(&abr_ops, &slot);
  if (res != kAbrResultOk) {
    return resp(ResponseType::kFail, "Failed to get slot last set active", transport);
  }
  // The slot is guaranteed not to be r if the result is okay.
  const char *slot_str = slot == kAbrSlotIndexA ? "a" : "b";

  return resp(ResponseType::kOkay, slot_str, transport);
}

zx::result<> Fastboot::GetVarSlotRetryCount(const CommandArgs &args, fastboot::Transport *transport,
                                            const Responder &resp) {
  if (args.num_args < 3) {
    return resp(ResponseType::kFail, "Not enough arguments", transport);
  }

  std::optional<AbrSlotIndex> idx = ParseAbrSlotStr(args.args[2], false);
  if (!idx) {
    return resp(ResponseType::kFail, "slot name is invalid", transport);
  }

  AbrOps abr_ops = GetAbrOps();
  AbrSlotInfo info;
  AbrResult res = AbrGetSlotInfo(&abr_ops, *idx, &info);
  if (res != kAbrResultOk) {
    return resp(ResponseType::kFail, "Failed to get slot retry count", transport);
  }

  char retry_str[16] = {0};
  snprintf(retry_str, sizeof(retry_str), "%u", info.num_tries_remaining);

  return resp(ResponseType::kOkay, retry_str, transport);
}

zx::result<> Fastboot::GetVarSlotSuccessful(const CommandArgs &args, fastboot::Transport *transport,
                                            const Responder &resp) {
  if (args.num_args < 3) {
    return resp(ResponseType::kFail, "Not enough arguments", transport);
  }

  std::optional<AbrSlotIndex> idx = ParseAbrSlotStr(args.args[2], true);
  if (!idx) {
    return resp(ResponseType::kFail, "slot name is invalid", transport);
  }

  AbrOps abr_ops = GetAbrOps();
  AbrSlotInfo info;
  AbrResult res = AbrGetSlotInfo(&abr_ops, *idx, &info);
  if (res != kAbrResultOk) {
    return resp(ResponseType::kFail, "Failed to get slot successful", transport);
  }

  return resp(ResponseType::kOkay, info.is_marked_successful ? "yes" : "no", transport);
}

zx::result<> Fastboot::GetVarSlotUnbootable(const CommandArgs &args, fastboot::Transport *transport,
                                            const Responder &resp) {
  if (args.num_args < 3) {
    return resp(ResponseType::kFail, "Not enough arguments", transport);
  }

  std::optional<AbrSlotIndex> idx = ParseAbrSlotStr(args.args[2], true);
  if (!idx) {
    return resp(ResponseType::kFail, "slot name is invalid", transport);
  }

  AbrOps abr_ops = GetAbrOps();
  AbrSlotInfo info;
  AbrResult res = AbrGetSlotInfo(&abr_ops, *idx, &info);
  if (res != kAbrResultOk) {
    return resp(ResponseType::kFail, "Failed to get slot unbootable", transport);
  }

  return resp(ResponseType::kOkay, info.is_bootable ? "no" : "yes", transport);
}

zx::result<> Fastboot::GetVarSlotUnbootableReason(const CommandArgs &args,
                                                  fastboot::Transport *transport,
                                                  const Responder &resp) {
  if (args.num_args < 3) {
    return resp(ResponseType::kFail, "Not enough arguments", transport);
  }

  std::optional<AbrSlotIndex> idx = ParseAbrSlotStr(args.args[2], true);
  if (!idx) {
    return resp(ResponseType::kFail, "slot name is invalid", transport);
  }

  AbrOps abr_ops = GetAbrOps();
  AbrSlotInfo info;
  AbrResult res = AbrGetSlotInfo(&abr_ops, *idx, &info);
  if (res != kAbrResultOk) {
    return resp(ResponseType::kFail, "Failed to get slot unbootable reason", transport);
  }

  if (info.is_bootable) {
    return resp(ResponseType::kOkay, "N/A", transport);
  }

  const char *str = "unknown";
  // Cast to `AbrUnbootableReason` so the compiler checks that we've handled all the known cases
  // (but it still might not be one of these values if the metadata writer had a newer libabr
  // version with additional reasons we don't know about).
  switch (static_cast<AbrUnbootableReason>(info.unbootable_reason)) {
    case kAbrUnbootableReasonNone:
      str = "none given";
      break;
    case kAbrUnbootableReasonNoMoreTries:
      str = "no more attempts";
      break;
    case kAbrUnbootableReasonOsRequested:
      str = "OS requested";
      break;
    case kAbrUnbootableReasonVerificationFailure:
      str = "verification failure";
      break;
  }

  // Give a human-readable string as well as the raw value for programmatic parsing and in case of
  // unknown values.
  char buffer[64];
  snprintf(buffer, sizeof(buffer), "%d (%s)", info.unbootable_reason, str);
  return resp(ResponseType::kOkay, buffer, transport);
}

class GptDataHolder {
 public:
  explicit GptDataHolder(FuchsiaFirmwareStorage &ops) : ops_(ops) {}
  ~GptDataHolder() { ZX_ASSERT(FuchsiaFirmwareStorageFreeGptData(&ops_, &data_)); }
  GptData &Data() { return data_; }

 private:
  GptData data_;
  FuchsiaFirmwareStorage &ops_;
};

zx::result<> Fastboot::Flash(std::string_view cmd, fastboot::Transport *transport) {
  CommandArgs args;
  ExtractCommandArgs(cmd, ":", args);
  if (args.num_args < 2) {
    return SendResponse(ResponseType::kFail, "Not enough argument", transport);
  }

  auto device = FindEfiGptDevice();
  if (device.is_error() || device->Load().is_error()) {
    return SendResponse(ResponseType::kFail, "Failed to get block device", transport,
                        zx::error(ZX_ERR_INTERNAL));
  }

  ZX_ASSERT(args.args[1].size() < fastboot::kMaxCommandPacketSize);
  char part_name_data[fastboot::kMaxCommandPacketSize] = {};
  memcpy(part_name_data, args.args[1].data(), args.args[1].size());
  std::string_view part_name = MaybeMapPartitionName(*device, part_name_data);

  FuchsiaFirmwareStorage ops = device->GenerateStorageOps();
  GptDataHolder gpt_data(ops);
  if (!FuchsiaFirmwareStorageSyncGpt(&ops, &gpt_data.Data())) {
    return SendResponse(ResponseType::kFail, "Failed to sync gpt data", transport,
                        zx::error(ZX_ERR_INTERNAL));
  }

  bool res;
  if (FuchsiaIsSparseImage(download_buffer_.data(), total_download_size())) {
    res = FuchsiaWriteSparseImage(&ops, &gpt_data.Data(), part_name.data(), download_buffer_.data(),
                                  total_download_size());
  } else {
    res = FuchsiaFirmwareStorageGptWrite(&ops, &gpt_data.Data(), part_name.data(), 0,
                                         total_download_size(), download_buffer_.data());
  }

  if (!res) {
    return SendResponse(ResponseType::kFail, "Failed to write to partition", transport,
                        zx::error(ZX_ERR_INTERNAL));
  }

  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<> Fastboot::GptInit(std::string_view cmd, fastboot::Transport *transport) {
  auto device = FindEfiGptDevice();
  if (!device.is_ok()) {
    return SendResponse(ResponseType::kFail, "Failed to get block device", transport,
                        zx::error(ZX_ERR_INTERNAL));
  }

  auto res = device.value().Reinitialize();
  if (!res.is_ok()) {
    return SendResponse(ResponseType::kFail, "Failed to reinitialize partiions", transport,
                        zx::error(ZX_ERR_INTERNAL));
  }

  return SendResponse(ResponseType::kOkay, "", transport);
}

[[gnu::format(printf, 1, 0)]]
fbl::Vector<char> StringPrintf(const char *fmt, va_list va) {
  fbl::Vector<char> buf{'\0'};
  va_list va2;
  va_copy(va2, va);
  const size_t len = vsnprintf(buf.data(), 0, fmt, va);
  buf.resize(len + 1);
  vsnprintf(buf.data(), buf.size(), fmt, va2);
  va_end(va2);
  buf.pop_back();
  return buf;
}

void Fastboot::InfoSend(fastboot::Transport *transport, const char *fmt, va_list va) {
  fbl::Vector<char> s = StringPrintf(fmt, va);
  if (Fastboot::SendResponse(Fastboot::ResponseType::kInfo, s.data(), transport).is_error()) {
    printf("Failed to send Info response.\n");
  }
}

zx::result<> Fastboot::EfiGetVarInfo(std::string_view cmd, fastboot::Transport *transport) {
  auto info = efi_variables_->EfiQueryVariableInfo();
  auto transport_scope = RegisterTransport(transport);

  if (info.is_error()) {
    return SendResponse(ResponseType::kFail, "QueryVariableInfo() failed", transport);
  }

  printer_(
      "\n"
      "  Max Storage Size: %" PRIu64
      "\n"
      "  Remaining Variable Storage Size: %" PRIu64
      "\n"
      "  Max Variable Size: %" PRIu64 "\n",
      info.value().max_var_storage_size, info.value().remaining_var_storage_size,
      info.value().max_var_size);

  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<> Fastboot::EfiGetVarNames(std::string_view cmd, fastboot::Transport *transport) {
  auto transport_scope = RegisterTransport(transport);

  for (auto v_id : *efi_variables_) {
    if (!v_id.IsValid()) {
      return SendResponse(ResponseType::kFail, "EfiVariableName iteration failed", transport);
    }

    const auto &guid = v_id.vendor_guid;
    printer_("%s %s\n", ToStr(guid).data(), v_id.name.c_str());
  }
  printer_("\n");

  return SendResponse(ResponseType::kOkay, "", transport);
}

void hexdump_printer_printf(void *printf_arg, const char *fmt, ...) {
  Fastboot *obj = static_cast<Fastboot *>(printf_arg);
  va_list args;
  va_start(args, fmt);
  obj->vprinter_(fmt, args);
  va_end(args);
}

// Hex dump function with shortened output line length. Mostly taken from:
// http://cs/fuchsia/zircon/system/ulib/pretty/hexdump.cc
// void hexdump8_very_ex(const void* ptr, size_t len, uint64_t disp_addr, hexdump_printf_func_t*
// printf_func, void* printf_arg);
//
// It differs from original in:
//  - offset column removed
//  - spaces between quartets of bytes are removed
//
// Output format looks like:
// ```
//  01010102 00000100 00000000 00000001 |................
//  00                                  |.
// ```
//
// Short hexdump version is required to make it readable when send via kInfo channel.
// In order to make it easier to use with kInfo channel this function calls `printf_func` only to
// print full line.
void hexdump8_short(const void *ptr, size_t len, hexdump_printf_func_t *printf_func,
                    void *printf_arg) {
  const uint8_t *address = reinterpret_cast<const uint8_t *>(ptr);
  char line_buffer[fastboot::kMaxCommandPacketSize + 1];

  for (size_t count = 0; count < len; count += 16) {
    std::span<char> remaining(line_buffer);
    size_t used = 0;

    size_t i;
    for (i = 0; i < std::min(len - count, size_t{16}); i++) {
      used = snprintf(remaining.data(), remaining.size(), "%02hhx%s", *(address + i),
                      (i % 4 == 3) ? " " : "");
      remaining = remaining.subspan(used);
    }

    for (; i < 16; i++) {
      used = snprintf(remaining.data(), remaining.size(), "%s", (i % 4 == 3) ? "   " : "  ");
      remaining = remaining.subspan(used);
    }

    used = snprintf(remaining.data(), remaining.size(), "|");
    remaining = remaining.subspan(used);

    for (i = 0; i < std::min(len - count, size_t{16}); i++) {
      char c = address[i];
      used = snprintf(remaining.data(), remaining.size(), "%c", isprint(c) ? c : '.');
      remaining = remaining.subspan(used);
    }

    used = snprintf(remaining.data(), remaining.size(), "\n");
    remaining = remaining.subspan(used);

    printf_func(printf_arg, line_buffer);
    address += 16;
  }
}

// This function expects `cmd` in following format:
// oem efi-getvar <var_name> [<guid>]
//
// E.g.
//  oem efi-getvar BootOrder
//  oem efi-getvar BootOrder 8be4df61-93ca-11d2-aa0d-00e098032b8c
zx::result<> Fastboot::EfiGetVar(std::string_view cmd, fastboot::Transport *transport) {
  auto transport_scope = RegisterTransport(transport);

  CommandArgs args;
  ExtractCommandArgs(cmd, " ", args);

  if (args.num_args < 3 || args.num_args > 4) {
    return SendResponse(ResponseType::kFail,
                        "Bad number of arguments."
                        " Expected format: oem efi-getvar <var_name> [<guid>]",
                        transport);
  }

  const std::string_view in_var_name = args.args[2];
  efi::String var_name = efi::String(in_var_name);
  if (!var_name.IsValid()) {
    printer_("UTF8->UC2 convertion failed for variable name: '%s'\n", in_var_name.data());
    return SendResponse(ResponseType::kFail, "UTF8->UC2 convertion failed for variable name",
                        transport);
  }

  efi_guid guid;
  if (args.num_args == 4) {
    std::string_view guid_str = args.args[3];
    auto res_guid = ToGuid(guid_str);
    if (res_guid.is_error()) {
      printer_("Vendor GUID parsing failed (%s) for: '%s'\n", xefi_strerror(res_guid.error_value()),
               guid_str.data());
      return SendResponse(ResponseType::kFail, "Vendor GUID parsing failed", transport);
    }
    guid = res_guid.value();
  } else if (args.num_args == 3) {
    auto res_guid = efi_variables_->GetGuid(var_name);
    if (res_guid.is_error()) {
      printer_("Vendor GUID search failed (%s) for: '%s'\n", xefi_strerror(res_guid.error_value()),
               var_name.c_str());
      if (res_guid.error_value() == EFI_INVALID_PARAMETER) {
        return SendResponse(ResponseType::kFail,
                            "Multiple entries found with specified name. Please provide GUID",
                            transport);
      } else {
        return SendResponse(ResponseType::kFail, "Vendor GUID search failed", transport);
      }
    }
    guid = res_guid.value();
  }

  // Print VariableName
  efi::VariableId v_id{std::move(var_name), guid};
  if (!v_id.name.IsValid()) {
    const auto err_str = "Failed to convert UCS2 variable name to UTF8\n";
    printer_(err_str);
    return SendResponse(ResponseType::kFail, err_str, transport);
  }
  printer_("%s %s:\n", ToStr(guid).data(), v_id.name.c_str());

  // Get and print Value
  auto res_get_var = efi_variables_->EfiGetVariable(v_id);
  if (res_get_var.is_error()) {
    return SendResponse(ResponseType::kFail, "GetVariable() failed", transport);
  }
  hexdump8_short(res_get_var.value().data(), res_get_var.value().size(), hexdump_printer_printf,
                 this);

  return SendResponse(ResponseType::kOkay, "", transport);
}

int Fastboot::printer_(const char *fmt, ...) {
  va_list args;
  va_start(args, fmt);
  int res = vprinter_(fmt, args);
  va_end(args);
  return res;
}

zx::result<> Fastboot::EfiDumpVars(std::string_view cmd, fastboot::Transport *transport) {
  auto transport_scope = RegisterTransport(transport);
  for (const auto &v_id : *efi_variables_) {
    if (!v_id.IsValid())
      return SendResponse(ResponseType::kFail, "EfiVariableName iteration failed", transport);

    const auto &guid = v_id.vendor_guid;
    printer_("%s %s:\n", ToStr(guid).data(), v_id.name.c_str());

    auto res = efi_variables_->EfiGetVariable(v_id);
    if (res.is_error()) {
      printer_("Failed to get variable\n");
      continue;
    }

    auto &val = res.value();
    hexdump8_short(val.data(), val.size(), hexdump_printer_printf, this);
  }
  printer_("\n");

  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<> Fastboot::BootRam(std::string_view cmd, fastboot::Transport *transport) {
  // `fastboot boot <zbi>`: boot the image sitting in the download buffer without
  // writing anything to storage. This is the iteration loop Android developers
  // expect, and it is the one path that needs no provisioned disk at all.
  //
  // Unverified images must not be booted on a locked device.
  bool is_locked = true;
  if (zb_ops_.verified_boot_read_is_device_locked == nullptr ||
      !zb_ops_.verified_boot_read_is_device_locked(&zb_ops_, &is_locked) || is_locked) {
    return SendResponse(ResponseType::kFail, "Device is locked - fastboot boot not allowed",
                        transport);
  }

  if (total_download_size() == 0) {
    return SendResponse(ResponseType::kFail, "No image downloaded", transport);
  }
  if (total_download_size() < sizeof(zbi_header_t)) {
    return SendResponse(ResponseType::kFail, "Download too small to be a ZBI", transport);
  }

  zbi_header_t *image = reinterpret_cast<zbi_header_t *>(download_buffer_.data());
  if (image->type != ZBI_TYPE_CONTAINER || image->magic != ZBI_ITEM_MAGIC ||
      image->extra != ZBI_CONTAINER_MAGIC) {
    return SendResponse(ResponseType::kFail, "Downloaded image is not a ZBI container", transport);
  }
  if (image->length > total_download_size() - sizeof(zbi_header_t)) {
    return SendResponse(ResponseType::kFail,
                        "Downloaded ZBI container length exceeds download size", transport);
  }
  if (zbi_result_t res = zbi_check(image, nullptr); res != ZBI_RESULT_OK) {
    return SendResponse(ResponseType::kFail, "Malformed ZBI image", transport);
  }

  // Append the items gigaboot normally injects on a disk boot (ACPI RSDP,
  // framebuffer, cmdline, ...). Spare room is the unused tail of the download
  // buffer, which is sized for the largest flashable partition. A null slot is
  // correct here: a RAM boot belongs to no A/B slot, and AddGigabootZbiItems
  // skips the slot item when it is null.
  size_t capacity = download_buffer_.size();
  if (zb_ops_.add_zbi_items == nullptr ||
      !zb_ops_.add_zbi_items(&zb_ops_, image, capacity, nullptr)) {
    return SendResponse(ResponseType::kFail, "Failed to add ZBI items", transport,
                        zx::error(ZX_ERR_INTERNAL));
  }

  // Answer before booting. Boot() exits boot services and never returns on
  // success, so a host waiting for OKAY would otherwise just see the connection
  // drop and report a transport error on a successful boot.
  if (auto res = SendResponse(ResponseType::kOkay, "", transport); res.is_error()) {
    return res;
  }

  printf("fastboot: booting downloaded ZBI (%zu bytes) from RAM\n", total_download_size());
  zb_ops_.boot(&zb_ops_, image, capacity);

  // Only reached if the image was not bootable.
  return SendResponse(ResponseType::kFail, "Failed to boot downloaded image", transport,
                      zx::error(ZX_ERR_INTERNAL));
}

zx::result<> Fastboot::Continue(std::string_view cmd, fastboot::Transport *transport) {
  continue_ = true;
  return SendResponse(ResponseType::kOkay, "", transport);
}

zx::result<> Fastboot::OemAddStagedBootloaderFile(std::string_view cmd,
                                                  fastboot::Transport *transport) {
  CommandArgs args;
  ExtractCommandArgs(cmd, " ", args);
  if (args.num_args != 3) {
    return SendResponse(ResponseType::kFail, "Not enough argument", transport);
  }

  zbi_result_t res =
      AddBootloaderFiles(args.args[2].data(), download_buffer_.data(), total_download_size());
  if (res != ZBI_RESULT_OK) {
    printf("Failed to append zbi_files: %d\n", res);
    return SendResponse(ResponseType::kFail, "Failed to initialize zbi file", transport,
                        zx::error(ZX_ERR_INTERNAL));
  }

  return SendResponse(ResponseType::kOkay, "", transport);
}

using FastbootState = fastboot::FastbootBase::State;

// The transport implementation for a TCP fastboot packet.
class PacketTransport : public fastboot::Transport {
 public:
  PacketTransport(TcpTransportInterface &interface, size_t packet_size, Fastboot &fastboot)
      : interface_(&interface),
        packet_size_(packet_size),
        fastboot_(&fastboot),
        last_state_(fastboot.state()) {}

  zx::result<size_t> ReceivePacket(void *dst, size_t capacity) override {
    if (packet_size_ > capacity) {
      return zx::error(ZX_ERR_BUFFER_TOO_SMALL);
    }

    if (!interface_->Read(dst, packet_size_)) {
      return zx::error(ZX_ERR_INTERNAL);
    }

    if (last_state_ == FastbootState::kCommand) {
      printf("[fb] %.*s ... ", static_cast<int>(packet_size_), static_cast<char *>(dst));
    }

    return zx::ok(packet_size_);
  }

  // Peek the size of the next packet.
  size_t PeekPacketSize() override { return packet_size_; }

  zx::result<> Send(std::string_view packet) override {
    // The first packet within the data phase is logged so the DATA response is
    // shown. Subsequent packets are not logged until it reenters the command
    // phase.
    auto state = fastboot_->state();
    if (state == FastbootState::kCommand || last_state_ == FastbootState::kCommand) {
      printf("%.*s\n", static_cast<int>(packet.size()), packet.data());
      last_state_ = state;
    }

    // Prepend a length prefix.
    size_t size = packet.size();
    uint64_t be_size = ToBigEndian(size);
    if (!interface_->Write(&be_size, sizeof(be_size))) {
      return zx::error(ZX_ERR_INTERNAL);
    }

    if (!interface_->Write(packet.data(), size)) {
      return zx::error(ZX_ERR_INTERNAL);
    }

    return zx::ok();
  }

 private:
  TcpTransportInterface *interface_ = nullptr;
  size_t packet_size_ = 0;
  Fastboot *fastboot_ = nullptr;
  FastbootState last_state_ = FastbootState::kCommand;
};

void FastbootTcpSession(TcpTransportInterface &interface, Fastboot &fastboot) {
  // Whatever we receive, sends a handshake message first to improve performance.
  printf("Fastboot connection established\n");
  if (!interface.Write("FB01", 4)) {
    printf("Failed to write handshake message\n");
    return;
  }

  char handshake_buffer[kFastbootHandshakeMessageLength + 1] = {0};
  if (!interface.Read(handshake_buffer, kFastbootHandshakeMessageLength)) {
    printf("Failed to read handshake message\n");
    return;
  }

  // We expect "FBxx", where xx is a numeric value
  if (strncmp(handshake_buffer, "FB", 2) != 0 || !isdigit(handshake_buffer[2]) ||
      !isdigit(handshake_buffer[3])) {
    printf("Invalid handshake message %s\n", handshake_buffer);
    return;
  }

  while (true) {
    // Each fastboot packet is a length-prefixed data sequence. Read the length
    // prefix first.
    uint64_t packet_length = 0;
    if (!interface.Read(&packet_length, sizeof(packet_length))) {
      printf("Failed to read length prefix. Remote client might be disconnected\n");
      return;
    }

    // Process the length prefix. Convert big-endian to integer.
    packet_length = BigToHostEndian(packet_length);

    // Construct and pass a packet transport to fastboot.
    PacketTransport packet(interface, packet_length, fastboot);
    zx::result<> ret = fastboot.ProcessPacket(&packet);
    if (ret.is_error()) {
      printf("Failed to process fastboot packet\n");
      return;
    }

    if (fastboot.IsContinue()) {
      printf("Resuming boot...\n");
      return;
    }
  }
}

}  // namespace gigaboot
