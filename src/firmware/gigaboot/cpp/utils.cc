// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "utils.h"

#include <bootbyte.h>
#include <ctype.h>
#include <lib/abr/abr.h>
#include <lib/zbi-format/memory.h>
#include <lib/zbi/zbi.h>
#include <stdio.h>
#include <zircon/hw/gpt.h>

#include <algorithm>
#include <string_view>

#include <efi/global-variable.h>
#include <efi/string/string.h>
#include <efi/types.h>
#include <fbl/string_printf.h>

#include "fbl/vector.h"
#include "gpt.h"
#include "log.h"

namespace gigaboot {

namespace {

const std::string_view kCommandlineBootToFastboot = "boot_mode=fastboot";

std::optional<RebootMode> ParseByteToRebootMode(uint8_t b) {
  switch (b) {
    case 0x1:
      return RebootMode::kNormal;
    case 0x2:
      return RebootMode::kRecovery;
    case 0x4:
      return RebootMode::kBootloader;
    case 0xFF:
      return RebootMode::kBootloaderDefault;
    default:
      return std::nullopt;
  }
}

}  // namespace

const char* EfiStatusToString(efi_status status) {
  switch (status) {
#define ERR_ENTRY(x) \
  case x: {          \
    return #x;       \
  }
    ERR_ENTRY(EFI_SUCCESS);
    ERR_ENTRY(EFI_LOAD_ERROR);
    ERR_ENTRY(EFI_INVALID_PARAMETER);
    ERR_ENTRY(EFI_UNSUPPORTED);
    ERR_ENTRY(EFI_BAD_BUFFER_SIZE);
    ERR_ENTRY(EFI_BUFFER_TOO_SMALL);
    ERR_ENTRY(EFI_NOT_READY);
    ERR_ENTRY(EFI_DEVICE_ERROR);
    ERR_ENTRY(EFI_WRITE_PROTECTED);
    ERR_ENTRY(EFI_OUT_OF_RESOURCES);
    ERR_ENTRY(EFI_VOLUME_CORRUPTED);
    ERR_ENTRY(EFI_VOLUME_FULL);
    ERR_ENTRY(EFI_NO_MEDIA);
    ERR_ENTRY(EFI_MEDIA_CHANGED);
    ERR_ENTRY(EFI_NOT_FOUND);
    ERR_ENTRY(EFI_ACCESS_DENIED);
    ERR_ENTRY(EFI_NO_RESPONSE);
    ERR_ENTRY(EFI_NO_MAPPING);
    ERR_ENTRY(EFI_TIMEOUT);
    ERR_ENTRY(EFI_NOT_STARTED);
    ERR_ENTRY(EFI_ALREADY_STARTED);
    ERR_ENTRY(EFI_ABORTED);
    ERR_ENTRY(EFI_ICMP_ERROR);
    ERR_ENTRY(EFI_TFTP_ERROR);
    ERR_ENTRY(EFI_PROTOCOL_ERROR);
    ERR_ENTRY(EFI_INCOMPATIBLE_VERSION);
    ERR_ENTRY(EFI_SECURITY_VIOLATION);
    ERR_ENTRY(EFI_CRC_ERROR);
    ERR_ENTRY(EFI_END_OF_MEDIA);
    ERR_ENTRY(EFI_END_OF_FILE);
    ERR_ENTRY(EFI_INVALID_LANGUAGE);
    ERR_ENTRY(EFI_COMPROMISED_DATA);
    ERR_ENTRY(EFI_IP_ADDRESS_CONFLICT);
    ERR_ENTRY(EFI_HTTP_ERROR);
    ERR_ENTRY(EFI_CONNECTION_FIN);
    ERR_ENTRY(EFI_CONNECTION_RESET);
    ERR_ENTRY(EFI_CONNECTION_REFUSED);
#undef ERR_ENTRY
  }

  return "<Unknown error>";
}

// Converts an EFI memory type to a zbi_mem_range_t type.
uint32_t EfiToZbiMemRangeType(uint32_t efi_mem_type) {
  switch (efi_mem_type) {
    case EfiLoaderCode:
    case EfiLoaderData:
    case EfiBootServicesCode:
    case EfiBootServicesData:
    case EfiConventionalMemory:
      return ZBI_MEM_TYPE_RAM;
  }
  return ZBI_MEM_TYPE_RESERVED;
}

uint64_t ToBigEndian(uint64_t val) {
#if __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
  return __builtin_bswap64(val);
#else
  return val;
#endif
}

uint64_t BigToHostEndian(uint64_t val) { return ToBigEndian(val); }

efi_status PrintTpm2Capability() {
  auto tpm2_protocol = gigaboot::EfiLocateProtocol<efi_tcg2_protocol>();
  if (tpm2_protocol.is_error()) {
    return tpm2_protocol.error_value();
  }

  printf("Found TPM 2.0 EFI protocol.\n");

  // Log TPM capability
  efi_tcg2_boot_service_capability capability;
  efi_status status = tpm2_protocol->GetCapability(tpm2_protocol.value().get(), &capability);
  if (status != EFI_SUCCESS) {
    return status;
  }

  printf("TPM 2.0 Capabilities:\n");

#define PRINT_NAMED_VAL(field, format) printf(#field " = " format "\n", (field))

  // Structure version
  PRINT_NAMED_VAL(capability.StructureVersion.Major, "0x%02x");
  PRINT_NAMED_VAL(capability.StructureVersion.Minor, "0x%02x");

  // Protocol version
  PRINT_NAMED_VAL(capability.ProtocolVersion.Major, "0x%02x");
  PRINT_NAMED_VAL(capability.ProtocolVersion.Minor, "0x%02x");

#define PRINT_NAMED_BIT(flags, bit) printf(#flags "." #bit "= %d\n", ((flags) & (bit)) ? 1 : 0)

  // Supported hash algorithms
  PRINT_NAMED_BIT(capability.HashAlgorithmBitmap, EFI_TCG2_BOOT_HASH_ALG_SHA1);
  PRINT_NAMED_BIT(capability.HashAlgorithmBitmap, EFI_TCG2_BOOT_HASH_ALG_SHA256);
  PRINT_NAMED_BIT(capability.HashAlgorithmBitmap, EFI_TCG2_BOOT_HASH_ALG_SHA384);
  PRINT_NAMED_BIT(capability.HashAlgorithmBitmap, EFI_TCG2_BOOT_HASH_ALG_SHA512);
  PRINT_NAMED_BIT(capability.HashAlgorithmBitmap, EFI_TCG2_BOOT_HASH_ALG_SM3_256);

  // Supported event logs
  PRINT_NAMED_BIT(capability.SupportedEventLogs, EFI_TCG2_EVENT_LOG_FORMAT_TCG_1_2);
  PRINT_NAMED_BIT(capability.SupportedEventLogs, EFI_TCG2_EVENT_LOG_FORMAT_TCG_2);

  // Others
  PRINT_NAMED_VAL(capability.ProtocolVersion.Minor, "0x%02x");
  PRINT_NAMED_VAL(capability.TPMPresentFlag, "0x%02x");
  PRINT_NAMED_VAL(capability.MaxCommandSize, "0x%04x");
  PRINT_NAMED_VAL(capability.MaxResponseSize, "0x%04x");
  PRINT_NAMED_VAL(capability.ManufacturerID, "0x%08x");
  PRINT_NAMED_VAL(capability.NumberOfPcrBanks, "0x%08x");
  PRINT_NAMED_VAL(capability.ActivePcrBanks, "0x%08x");

#undef PRINT_NAMED_VAL
#undef PRINT_NAMED_BIT

  return EFI_SUCCESS;
}

fit::result<efi_status, bool> IsSecureBootOn() {
  size_t size = 1;
  uint8_t value;
  char16_t name[] = u"SecureBoot";
  efi_guid global_var_guid = GlobalVariableGuid;
  efi_status status =
      gEfiSystemTable->RuntimeServices->GetVariable(name, &global_var_guid, NULL, &size, &value);
  if (status != EFI_SUCCESS) {
    return fit::error(status);
  }

  return fit::ok(value);
}

std::string_view MaybeMapPartitionName(const EfiGptBlockDevice& device,
                                       std::string_view partition) {
  struct partition_names {
    std::string_view legacy;
    std::string_view modern;
  };
  constexpr partition_names names[]{
      {GUID_ABR_META_NAME, GPT_DURABLE_BOOT_NAME},
      {GUID_ZIRCON_A_NAME, GPT_ZIRCON_A_NAME},
      {GUID_ZIRCON_B_NAME, GPT_ZIRCON_B_NAME},
      {GUID_ZIRCON_R_NAME, GPT_ZIRCON_R_NAME},
      {GUID_FVM_NAME, GPT_FVM_NAME},
  };

  auto name_entry =
      std::find_if(std::begin(names), std::end(names),
                   [&partition](const auto& entry) { return entry.modern == partition; });
  if (name_entry == std::end(names)) {
    // This is some other partition without a legacy naming scheme that we care about.
    return partition;
  }

  for (const auto& p : device.ListPartitionNames()) {
    if (p.data() == name_entry->legacy) {
      return name_entry->legacy;
    }
    if (p.data() == name_entry->modern) {
      return name_entry->modern;
    }
  }

  // Should never reach here.
  return partition;
}

namespace {

std::optional<fbl::Vector<char>> GetCommandlineFromLoadOptions() {
  if (!gEfiLoadedImage) {
    DLOG("No EFI Loaded Image protocol found\n");
    return std::nullopt;
  }

  if (!gEfiLoadedImage->LoadOptions) {
    DLOG("EFI Loaded Image didn't provide any load options\n");
    return std::nullopt;
  }

  // The UEFI spec allows arbitrary binary data in LoadOptions, but for Gigaboot our only use case
  // is QEMU passing us commandline args which will always be UCS-2.
  if (reinterpret_cast<uintptr_t>(gEfiLoadedImage->LoadOptions) % alignof(char16_t) != 0 ||
      gEfiLoadedImage->LoadOptionsSize % sizeof(char16_t) != 0) {
    printf("Warning: Gigaboot commandline is not aligned UTF-16; ignoring\n");
    return std::nullopt;
  }

  fbl::Vector<char> utf8 = efi::String::ToUtf8(
      std::u16string_view(reinterpret_cast<char16_t*>(gEfiLoadedImage->LoadOptions),
                          gEfiLoadedImage->LoadOptionsSize / sizeof(char16_t)));
  if (utf8.is_empty()) {
    printf("Warning: Gigaboot commandline is not valid UTF-16; ignoring\n");
    return std::nullopt;
  }

  return utf8;
}

std::optional<fbl::Vector<char>> GetCommandlineFromDisk() {
  auto gpt_device = FindEfiGptDevice();
  if (gpt_device.is_error()) {
    DLOG("No GPT device found\n");
    return std::nullopt;
  }

  if (gpt_device->Load().is_error()) {
    DLOG("Failed to load GPT from disk\n");
    return std::nullopt;
  }

  fbl::Vector<char> commandline;
  commandline.resize(kDiskCommandlineMaxSize + 1);
  if (gpt_device
          ->ReadPartition(kDiskCommandlinePartitionName, 0, kDiskCommandlineMaxSize,
                          commandline.data())
          .is_error()) {
    DLOG("Failed to read from '%s' partition", kDiskCommandlinePartitionName);
    return std::nullopt;
  }
  // Terminate so that we don't buffer-overflow if the disk contents happen to be garbage.
  *(commandline.end() - 1) = '\0';

  return commandline;
}

}  // namespace

std::optional<gigaboot::RebootMode> GetCommandlineRebootMode(bool allow_disk_fallback) {
  auto commandline = GetCommandlineFromLoadOptions();
  // Right now we only look on-disk if we didn't find a loaded image commandline. It may be useful
  // in the future to look in both places and combine the commandline, but for now we don't need it.
  if (!commandline.has_value() && allow_disk_fallback) {
    commandline = GetCommandlineFromDisk();
  }

  if (!commandline.has_value()) {
    printf("No Gigaboot commandline was found\n");
    return std::nullopt;
  }

  printf("Gigaboot commandline: %s\n", commandline->data());

  // Just do super basic matching to look for the only commandline arg we care about.
  if (std::string_view(commandline->data()).find(kCommandlineBootToFastboot) !=
      std::string_view::npos) {
    printf("Found commandline fastboot trigger\n");
    return gigaboot::RebootMode::kBootloader;
  }

  return std::nullopt;
}

// TODO(b/285053546) 'BootByte' usage should be removed in favour of ABR Metadata
bool SetRebootMode(RebootMode mode) {
  return gEfiSystemTable != nullptr &&
         set_bootbyte(gEfiSystemTable->RuntimeServices, RebootModeToByte(mode)) == EFI_SUCCESS;
}

std::optional<RebootMode> GetOneShotRebootMode(AbrDataOneShotFlags one_shot_flags) {
  if (AbrIsOneShotRecoveryBootSet(one_shot_flags)) {
    return RebootMode::kRecovery;
  }
  if (AbrIsOneShotBootloaderBootSet(one_shot_flags)) {
    return RebootMode::kBootloader;
  }

  // TODO(b/285053546) 'BootByte' usage should be removed in favour of ABR Metadata
  uint8_t bootbyte;
  efi_status status = get_bootbyte(gEfiSystemTable->RuntimeServices, &bootbyte);
  if (status != EFI_SUCCESS) {
    return std::nullopt;
  }

  return ParseByteToRebootMode(bootbyte);
}

// See `ToStr()` for format details
// Expected input string should be in following format: "aabbccdd-eeff-gghh-iijj-kkllmmnnoopp"
fit::result<efi_status, efi_guid> ToGuid(std::string_view guid_str) {
  efi_guid guid;

  auto ParseByte = [](std::string_view& str, uint8_t& output) -> bool {
    if (str.size() < kByteToHexLen) {
      return false;
    }

    if (!std::all_of(str.begin(), str.begin() + kByteToHexLen, isxdigit)) {
      return false;
    }

    char c_str[kByteToHexLen + 1];
    str.copy(c_str, kByteToHexLen);
    c_str[kByteToHexLen] = '\0';
    output = static_cast<uint8_t>(strtoul(c_str, nullptr, 16));
    str = str.substr(kByteToHexLen);
    return true;
  };

  auto ParseDash = [](std::string_view& str) -> bool {
    constexpr size_t kInputLen = 1;
    if (str.size() < kInputLen) {
      return false;
    }
    if (str[0] != '-') {
      return false;
    }
    str = str.substr(kInputLen);
    return true;
  };

  std::span<uint8_t> buf(reinterpret_cast<uint8_t*>(&guid), sizeof(guid));
  if (ParseByte(guid_str, buf[3]) &&   // |   aa   |    3      |
      ParseByte(guid_str, buf[2]) &&   // |   bb   |    2      |
      ParseByte(guid_str, buf[1]) &&   // |   cc   |    1      |
      ParseByte(guid_str, buf[0]) &&   // |   dd   |    0      |
      ParseDash(guid_str) &&           // |   -    |    -      |
      ParseByte(guid_str, buf[5]) &&   // |   ee   |    5      |
      ParseByte(guid_str, buf[4]) &&   // |   ff   |    4      |
      ParseDash(guid_str) &&           // |   -    |    -      |
      ParseByte(guid_str, buf[7]) &&   // |   gg   |    7      |
      ParseByte(guid_str, buf[6]) &&   // |   hh   |    6      |
      ParseDash(guid_str) &&           // |   -    |    -      |
      ParseByte(guid_str, buf[8]) &&   // |   ii   |    8      |
      ParseByte(guid_str, buf[9]) &&   // |   jj   |    9      |
      ParseDash(guid_str) &&           // |   -    |    -      |
      ParseByte(guid_str, buf[10]) &&  // |   kk   |   10      |
      ParseByte(guid_str, buf[11]) &&  // |   ll   |   11      |
      ParseByte(guid_str, buf[12]) &&  // |   mm   |   12      |
      ParseByte(guid_str, buf[13]) &&  // |   nn   |   13      |
      ParseByte(guid_str, buf[14]) &&  // |   oo   |   14      |
      ParseByte(guid_str, buf[15]) &&  // |   pp   |   15      |
      guid_str.empty()) {
    return fit::ok(guid);
  }

  return fit::error(EFI_INVALID_PARAMETER);
}

// String format is specified at https://www.rfc-editor.org/rfc/rfc4122
// And also described here: https://uefi.org/specs/UEFI/2.10/Apx_A_GUID_and_Time_Formats.html
// This specification also defines a standard text representation of the GUID. This format is also
// sometimes called the “registry format”. It consists of 36 characters, as follows:
//
//  `aabbccdd-eeff-gghh-iijj-kkllmmnnoopp`
//
// The pairs aa - pp are two characters in the range ‘0’ -‘9’, ‘a’ -‘f’ or ‘A’ - F’, with each pair
// representing a single byte hexadecimal value.
//
// The following table describes the relationship between the text representation and a 16 - byte
// buffer, the structure defined in EFI GUID Format and the EFI_GUID structure.
//
// +--------+-----------+--------------------------------+-----------------+
// | String | Offset In | Relationship to EFI GUID       | Relationship To |
// |        | Buffer    | Format                         | EFI_GUID        |
// +--------+-----------+--------------------------------+-----------------+
// |   aa   |    3      | TimeLow[24:31]                 | Data1[24:31]    |
// |   bb   |    2      | TimeLow[16:23]                 | Data1[16:23]    |
// |   cc   |    1      | TimeLow[8:15]                  | Data1[8:15]     |
// |   dd   |    0      | TimeLow[0:7]                   | Data1[0:7]      |
// |   ee   |    5      | TimeMid[8:15]                  | Data2[8:15]     |
// |   ff   |    4      | TimeMid[0:7]                   | Data2[0:7]      |
// |   gg   |    7      | TimeHigh And Version[8:15]     | Data3[8:15]     |
// |   hh   |    6      | TimeHigh And Version[0:7]      | Data3[0:7]      |
// |   ii   |    8      | ClockSeqHigh And Reserved[0:7] | Data4[0:7]      |
// |   jj   |    9      | ClockSeqLow[0:7]               | Data4[8:15]     |
// |   kk   |   10      | Node[0:7]                      | Data4[16:23]    |
// |   ll   |   11      | Node[8:15]                     | Data4[24:31]    |
// |   mm   |   12      | Node[16:23]                    | Data4[32:39]    |
// |   nn   |   13      | Node[24:31]                    | Data4[40:47]    |
// |   oo   |   14      | Node[32:39]                    | Data4[48:55]    |
// |   pp   |   15      | Node[40:47]                    | Data4[56:63]    |
// +--------+-----------+--------------------------------+-----------------+
//
// First 4 blocks are in little endian.
fbl::Vector<char> ToStr(const efi_guid& g) {
  fbl::Vector<char> res;
  res.resize(kEfiGuidStrLen + 1);

  std::span<const uint8_t> buf(reinterpret_cast<const uint8_t*>(&g), sizeof(g));

  snprintf(res.data(), res.size(),
           "%02x%02x%02x%02x-"          // aabbccdd-
           "%02x%02x-"                  // eeff-
           "%02x%02x-"                  // gghh-
           "%02x%02x-"                  // iijj-
           "%02x%02x%02x%02x%02x%02x",  // kkllmmnnoopp
           buf[3],                      // |   aa   |    3      |
           buf[2],                      // |   bb   |    2      |
           buf[1],                      // |   cc   |    1      |
           buf[0],                      // |   dd   |    0      |
           buf[5],                      // |   ee   |    5      |
           buf[4],                      // |   ff   |    4      |
           buf[7],                      // |   gg   |    7      |
           buf[6],                      // |   hh   |    6      |
           buf[8],                      // |   ii   |    8      |
           buf[9],                      // |   jj   |    9      |
           buf[10],                     // |   kk   |   10      |
           buf[11],                     // |   ll   |   11      |
           buf[12],                     // |   mm   |   12      |
           buf[13],                     // |   nn   |   13      |
           buf[14],                     // |   oo   |   14      |
           buf[15]);                    // |   pp   |   15      |

  return res;
}

fit::result<efi_status> Timer::SetTimer(efi_timer_delay type, zx::duration timeout) {
  if (type == TimerCancel) {
    return fit::error(EFI_INVALID_PARAMETER);
  }
  if (timeout == zx::duration::infinite()) {
    state_ = State::kInfinite;
    return fit::ok();
  }
  if (timeout == zx::duration(0)) {
    state_ = State::kZero;
    return fit::ok();
  }

  state_ = State::kNormal;
  if (!timer_event_) {
    efi_status status =
        sys_->BootServices->CreateEvent(EVT_TIMER, 0, nullptr, nullptr, &timer_event_);
    if (status != EFI_SUCCESS) {
      return fit::error(status);
    }
  }

  // Timer ticks are in 100ns.
  efi_status res = sys_->BootServices->SetTimer(timer_event_, type, timeout.to_usecs() * 10);
  if (res == EFI_SUCCESS) {
    return fit::ok();
  }

  return fit::error(res);
}

fit::result<efi_status> Timer::Cancel() {
  if (!timer_event_) {
    return fit::ok();
  }

  efi_status res = sys_->BootServices->SetTimer(timer_event_, TimerCancel, 0);
  if (res == EFI_SUCCESS) {
    return fit::ok();
  }

  return fit::error(res);
}

Timer::Status Timer::CheckTimer() {
  if (state_ == State::kZero) {
    return Status::kReady;
  }
  if (state_ == State::kInfinite) {
    return Status::kWaiting;
  }
  if (!timer_event_) {
    return Status::kError;
  }

  efi_status res = sys_->BootServices->CheckEvent(timer_event_);
  switch (res) {
    case EFI_SUCCESS:
      return Status::kReady;
    case EFI_NOT_READY:
      return Status::kWaiting;
    default:
      return Status::kError;
  }
}

}  // namespace gigaboot
