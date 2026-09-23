// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_GIGABOOT_CPP_UTILS_H_
#define SRC_FIRMWARE_GIGABOOT_CPP_UTILS_H_

#include <lib/abr/ops.h>
#include <lib/zx/time.h>
#include <xefi.h>

#include <optional>
#include <span>
#include <string_view>

#include <efi/boot-services.h>
#include <efi/protocol/block-io.h>
#include <efi/protocol/device-path.h>
#include <efi/protocol/disk-io.h>
#include <efi/protocol/file.h>
#include <efi/protocol/graphics-output.h>
#include <efi/protocol/managed-network.h>
#include <efi/protocol/serial-io.h>
#include <efi/protocol/simple-text-output.h>
#include <efi/protocol/tcg2.h>
#include <efi/system-table.h>
#include <efi/types.h>
#include <fbl/vector.h>
#include <phys/efi/main.h>
#include <phys/efi/protocol.h>

template <>
inline constexpr const efi_guid& kEfiProtocolGuid<efi_device_path_protocol> = DevicePathProtocol;

template <>
inline constexpr const efi_guid& kEfiProtocolGuid<efi_block_io_protocol> = BlockIoProtocol;

template <>
inline constexpr const efi_guid& kEfiProtocolGuid<efi_disk_io_protocol> = DiskIoProtocol;

template <>
inline constexpr const efi_guid& kEfiProtocolGuid<efi_tcg2_protocol> = Tcg2Protocol;

template <>
inline constexpr const efi_guid& kEfiProtocolGuid<efi_graphics_output_protocol> =
    GraphicsOutputProtocol;

template <>
inline constexpr const efi_guid& kEfiProtocolGuid<efi_serial_io_protocol> = SerialIoProtocol;

template <>
inline constexpr const efi_guid& kEfiProtocolGuid<efi_managed_network_protocol> =
    ManagedNetworkProtocol;

namespace gigaboot {

// This is a utility function useful for calculating e.g. the number of pages to allocate to back a
// certain number of bytes, or the number of disk blocks to back a partition, rounding up for a
// margin of safety.
template <typename T>
auto constexpr DivideRoundUp(T t1, T t2) -> decltype(t1 + t2) {
  return (t1 + t2 - 1) / t2;
}

// This calls into the LocateProtocol() boot service. It returns a null pointer if operation fails.
template <class Protocol>
inline fit::result<efi_status, EfiProtocolPtr<Protocol>> EfiLocateProtocol() {
  void* ptr = nullptr;
  efi_status status =
      gEfiSystemTable->BootServices->LocateProtocol(&kEfiProtocolGuid<Protocol>, nullptr, &ptr);
  if (status != EFI_SUCCESS) {
    return fit::error{status};
  }
  return fit::ok(EfiProtocolPtr<Protocol>(static_cast<Protocol*>(ptr)));
}

// A wrapper type for the list of handles returned by LocateHandleBuffer() boot service. It owns
// the memory backing the list and will free it upon destruction
class HandleBuffer {
 public:
  HandleBuffer(efi_handle* handles, size_t count) : handles_(handles), count_(count) {}
  std::span<efi_handle> AsSpan() { return std::span<efi_handle>{handles_.get(), count_}; }

 private:
  // The deleter frees the list by calling the FreePool() boot service.
  struct HandlePtrDeleter {
    void operator()(efi_handle* ptr) { gEfiSystemTable->BootServices->FreePool(ptr); }
  };

  std::unique_ptr<efi_handle, HandlePtrDeleter> handles_;
  const size_t count_ = 0;
};

// This calls into LocateHandleBuffer() with ByProtocol search type and the given protocol.
// It returns a list of efi_handles that support the given protocol
template <class Protocol>
inline fit::result<efi_status, HandleBuffer> EfiLocateHandleBufferByProtocol() {
  size_t count;
  efi_handle* handles;
  efi_status status = gEfiSystemTable->BootServices->LocateHandleBuffer(
      ByProtocol, &kEfiProtocolGuid<Protocol>, nullptr, &count, &handles);
  if (status != EFI_SUCCESS) {
    return fit::error{status};
  }

  return fit::ok(HandleBuffer(handles, count));
}

// Convert a given efi_status code to informative string.
const char* EfiStatusToString(efi_status status);

// Convert efi memory type code to zbi memory type code.
uint32_t EfiToZbiMemRangeType(uint32_t efi_mem_type);

// Convert an integer to big endian byte order
uint64_t ToBigEndian(uint64_t val);

// Convert an given integer, assuming in big endian to little endian order.
uint64_t BigToHostEndian(uint64_t val);

constexpr size_t kUefiPageSize = 4096;

efi_status PrintTpm2Capability();

// Check whether secure boot is turned on by querying the `SecureBoot` global variable.
// Returns error if fail to query `SecureBoot`.
fit::result<efi_status, bool> IsSecureBootOn();

// Can't include gpt.h due to circular includes so declare the type here.
class EfiGptBlockDevice;
std::string_view MaybeMapPartitionName(const EfiGptBlockDevice& device, std::string_view partition);

enum class RebootMode : uint8_t {
  kNormal = 0x1,
  kRecovery = 0x2,
  kBootloader = 0x4,
  kBootloaderDefault = 0xFF,
};

constexpr uint8_t RebootModeToByte(RebootMode m) { return static_cast<uint8_t>(m); }

// Set reboot mode. Returns true if succeeds, false otherwise.
bool SetRebootMode(RebootMode mode);

constexpr const char* kDiskCommandlinePartitionName = "qemu-commandline";
constexpr size_t kDiskCommandlineMaxSize = 1024;

// Gets the reboot mode indicated by the commandline, or std::nullopt if none was found.
//
// If `allow_disk_fallback` is set, then if this function doesn't find a commandline it will also
// search the disk for a commandline to use, as follows:
//   1. locate the boot disk (i.e. the disk that Gigaboot itself came from)
//   2. read the GPT and locate a partition named `kDiskCommandlinePartitionName`
//   3. read `kDiskCommandlineMaxSize` bytes from the partition
//   4. the commandline will be these UTF-8 contents up until the first `\0` terminator
// WARNING: This is unsafe on real devices as these contents are not verified in any way, and should
// only be used for testing with emulators.
std::optional<RebootMode> GetCommandlineRebootMode(bool allow_disk_fallback);

// Get the reboot mode indicated by |one_shot_flags| and/or the bootbyte data.
// Returns std::nullopt on failure
std::optional<RebootMode> GetOneShotRebootMode(AbrDataOneShotFlags one_shot_flags);

// Convert hex string to `efi_guid`
// Input string should be in RFC4122 "registry format"
//  `aabbccdd-eeff-gghh-iijj-kkllmmnnoopp`
fit::result<efi_status, efi_guid> ToGuid(std::string_view guid_str);

// Convert `efi_guid` to string according to https://www.rfc-editor.org/rfc/rfc4122
// Return value is '\0' terminated.
constexpr size_t kByteToHexLen = 2;
constexpr size_t kEfiGuidStrLen = (sizeof(efi_guid) * kByteToHexLen + 4 /*dashes*/);
fbl::Vector<char> ToStr(const efi_guid& g);

// A wrapper around a UEFI timer.
class Timer {
 public:
  enum class Status {
    kReady,
    kWaiting,
    kError,
  };

  explicit Timer(efi_system_table* sys) : sys_(sys) {}
  Timer() = delete;
  Timer(const Timer&) = delete;
  Timer(Timer&&) = delete;
  Timer& operator=(const Timer&) = delete;
  Timer& operator=(Timer&&) = delete;

  ~Timer() {
    // The timer event is lazily constructed, so it may be null.
    if (timer_event_) {
      gEfiSystemTable->BootServices->CloseEvent(timer_event_);
    }
  }

  // Configure the timer's behavior.
  //
  // Both 0 and zx::duration::infinite() are valid values for timeout.
  // See the comment for CheckTimer for their semantics.
  fit::result<efi_status> SetTimer(efi_timer_delay type, zx::duration timeout);

  // Check to see if the timer has expired or if an error has occurred.
  //
  // A timer set with a duration of 0 will ALWAYS return Status::kReady.
  // A timer set with an infinite duration will ALWAYS return Status::kWaiting.
  // Calling CheckTimer on a timer that has not been set will return Status::kError.
  Status CheckTimer();

  // Cancel the timer.
  fit::result<efi_status> Cancel();

 private:
  enum class State {
    kNormal,
    kZero,
    kInfinite,
  };

  State state_ = State::kNormal;
  efi_system_table* sys_;
  efi_event timer_event_ = nullptr;
};

}  // namespace gigaboot

#endif  // SRC_FIRMWARE_GIGABOOT_CPP_UTILS_H_
