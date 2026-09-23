// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_LIB_ZIRCON_BOOT_INCLUDE_LIB_ZIRCON_BOOT_ZIRCON_BOOT_H_
#define SRC_FIRMWARE_LIB_ZIRCON_BOOT_INCLUDE_LIB_ZIRCON_BOOT_ZIRCON_BOOT_H_

// Standard library may not be available for all platforms. If that's the case,
// device should provide their own header that implements the equivalents.
//
// The library uses ABR library in this sdk, where there is also a switch to
// decide whether to use standard library. It should be adjusted accordingly as well.
#ifdef ZIRCON_BOOT_CUSTOM_SYSDEPS_HEADER
#include <zircon_boot_sysdeps.h>
#else
#include <stddef.h>
#endif

// This should point to the abr.h in the abr library in firmware sdk.
#include <lib/abr/abr.h>
// This should point to the zbi.h in the zbi library in firmware sdk.
#include <lib/zbi/zbi.h>
#include <lib/zircon_boot/zbi_utils.h>

#include <libavb_atx/libavb_atx.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef enum ZirconBootResult {
  kBootResultOK = 0,

  kBootResultErrorGeneric,
  kBootResultErrorInvalidArguments,
  kBootResultErrorMarkUnbootable,
  kBootResultErrorReadHeader,
  kBootResultErrorInvalidZbi,
  kBootResultErrorReadImage,
  kBootResultErrorSlotFail,
  kBootResultErrorNoValidSlot,
  kBootResultErrorIsSlotSupprotedByFirmware,
  kBootResultErrorMismatchedFirmwareSlot,
  kBootResultRebootReturn,
  kBootResultBootReturn,

  kBootResultErrorInvalidSlotIdx,
  kBootResultErrorImageTooLarge,

  kBootResultErrorAppendZbiItems,
  kBootResultErrorSlotVerification,
} ZirconBootResult;

// Partition names for slotless boot.
//
// These currently live here rather than zircon/hw/gpt.h because they aren't
// fully-supported Fuchsia partitions, but instead are intended for two narrow
// use cases:
//   1. very early bring-up, before A/B/R partitions might be available
//   2. RAM-booting, in which case these aren't actually partitions on-disk but
//      just used for internal mapping to indicate the RAM buffer.
//
// Some OS functionality, in particular OTA/paving, may behave strangely or not
// work at all during slotless boot so should not be relied upon for normal
// operation.
#define GPT_ZIRCON_SLOTLESS_NAME "zircon"
#define GPT_VBMETA_SLOTLESS_NAME "vbmeta"

struct ZirconBootOps;
typedef struct ZirconBootOps ZirconBootOps;

// Firmware specific operations required to use the library.
struct ZirconBootOps {
  // Device-specific data.
  void* context;

  // Reads from a partition.
  //
  // @ops: Pointer to the host |ZirconBootOps|.
  // @part: Name of the partition.
  // @offset: Offset in the partition to read from.
  // @size: Number of bytes to read.
  // @dst: Output buffer.
  // @read_size: Output pointer for storing the actual number of bytes read. The libary uses
  // this value to verify the integrity of read data. It expects it to always equal the
  // requested number of bytes.
  //
  // Returns true on success.
  bool (*read_from_partition)(ZirconBootOps* ops, const char* part, size_t offset, size_t size,
                              void* dst, size_t* read_size);

  // Writes data to partition.
  //
  // @ops: Pointer to the host |ZirconBootOps|.
  // @part: Name of the partition.
  // @offset: Offset in the partition to write to.
  // @size: Number of bytes to write.
  // @src: Payload buffer.
  // @write_size: Output pointer for storing the actual number of bytes written. The library
  // uses this value to verify the integrity of write data. It expects it to always equal the
  // number of bytes requested to be written.
  //
  // Returns true on success.
  bool (*write_to_partition)(ZirconBootOps* ops, const char* part, size_t offset, size_t size,
                             const void* src, size_t* write_size);

  // Boots image in memory.
  //
  // @ops: Pointer to the host |ZirconBootOps|.
  // @image: Pointer to the zircon kernel image in memory. It is expected to be a zbi
  //        container.
  // @capacity: Capacity of the container.
  //
  // The function is not expected to return if boot is successful.
  void (*boot)(ZirconBootOps* ops, zbi_header_t* image, size_t capacity);

  // Checks whether the currently running firmware can be used to boot the target kernel slot.
  // If set, zircon_boot will call this before attempting to load/boot/decrease retry counter for
  // the current active slot. If |out| is true, it proceeds. Otherwise, it will calls the reboot
  // method below to trigger a device reboot. The typical use case of this method is to extend
  // A/B/R booting to firmware (firmware ABR booting). Specifically, user can check whether current
  // firmware slot is compatible with the active kernel slot to boot. If the same, boot can proceeds
  // Otherwise the library will trigger a reboot and expect that the device can reboot into a
  // firmware slot that is compatible with the active slot according to current abr metadata, i.e
  // the slot expected to be returned by AbrGetBootSlot(). The typical approach is to have earlier
  // stage firmware also boot according to A/B/R. Boards without firmware A/B/R can just leave this
  // function unimplemented.
  //
  // @ops: Pointer to the host |ZirconBootOps|
  // @kernel_slot: Target slot for kernel.
  // @out: An output pointer for returning whether current slot can boot the target `kernel_slot`
  //
  // Returns true on success, false otherwise.
  bool (*firmware_can_boot_kernel_slot)(ZirconBootOps* ops, AbrSlotIndex kernel_slot, bool* out);

  // Reboots the device.
  //
  // @ops: Pointer to the host |ZirconBootOps|
  // @force_recovery: Enable/Disable force recovery.
  //
  // The function is not expected to return if reboot is successful.
  void (*reboot)(ZirconBootOps* ops, bool force_recovery);

  // Adds device-specific ZBI items based on available boot information. The method is optional and
  // may be set to NULL, in which case no zbi items will be appended to the boot image.
  //
  // @ops: Pointer to the host |ZirconBootOps|
  // @image: The loaded kernel image as a ZBI container. Items should be appended to it.
  // @capacity: Capacity of the ZBI container.
  // @slot: A/B/R slot of the loaded image, or NULL for a kZirconBootFlagsSlotless boot.
  //
  // Returns true on success.
  bool (*add_zbi_items)(ZirconBootOps* ops, zbi_header_t* image, size_t capacity,
                        const AbrSlotIndex* slot);

  // Following are operations required to perform zircon verified boot.
  // Verified boot implemented in this library is based on libavb. The library use the following
  // provided operations to interact with libavb for kernel verification.
  // If any of the following function pointers is set to NULL, verified boot is bypassed.

  // Gets the size of a partition with the name in |part|
  // (NUL-terminated UTF-8 string). Returns the value in
  // |out|.
  //
  // @ops: Pointer to the ZirconBootOps the device implements.
  // @part: Name of the partition.
  // @out: Output pointer storing the result.
  //
  // Returns true on success.
  bool (*verified_boot_get_partition_size)(ZirconBootOps* ops, const char* part, size_t* out);

  // Gets the rollback index corresponding to the location given by
  // |rollback_index_location|. The value is returned in
  // |out_rollback_index|.
  //
  // @ops: Pointer to the ZirconBootOps the device implements.
  // @rollback_index_location: Location to write rollback index.
  // @out_rollback_index: Output pointer for storing the index value.
  //
  // A device may have a limited amount of rollback index locations (say,
  // one or four) so may error out if |rollback_index_location| exceeds
  // this number.
  //
  // Returns true on success.
  bool (*verified_boot_read_rollback_index)(ZirconBootOps* ops, size_t rollback_index_location,
                                            uint64_t* out_rollback_index);

  // Sets the rollback index corresponding to the location given by
  // |rollback_index_location| to |rollback_index|.
  //
  // @ops: Pointer to the ZirconBootOps the device implements.
  // @rollback_index_location: Location to rollback index to write.
  // @rollback_index: Value of the rollback index to write.
  //
  // A device may have a limited amount of rollback index locations (say,
  // one or four) so may error out if |rollback_index_location| exceeds
  // this number.
  //
  // Returns true on success.
  bool (*verified_boot_write_rollback_index)(ZirconBootOps* ops, size_t rollback_index_location,
                                             uint64_t rollback_index);

  // Gets whether the device is locked. The value is returned in
  // |out_is_locked| (true if locked, false otherwise).
  //
  // @ops: Pointer to the ZirconBootOps the device implements.
  // @out_is_locked: Output pointer for storing the status.
  //
  // Returns true on success.
  bool (*verified_boot_read_is_device_locked)(ZirconBootOps* ops, bool* out_is_locked);

  // Reads permanent |attributes| data. There are no restrictions on where this
  // data is stored.
  //
  // @ops: Pointer to the ZirconBootOps the device implements.
  // @attribute: Output pointer for storing the permanent attribute.
  //
  // Returns true on success.
  bool (*verified_boot_read_permanent_attributes)(ZirconBootOps* ops,
                                                  AvbAtxPermanentAttributes* attribute);

  // Reads a |hash| of permanent attributes. This hash MUST be retrieved from a
  // permanently read-only location (e.g. fuses) when a device is LOCKED.
  //
  // @ops: Pointer to the ZirconBootOps the device implements.
  // @hash: Output buffer that stores the hash values. The buffer must be able to hold
  //        |AVB_SHA256_DIGEST_SIZE| bytes.
  //
  // Returns true on success.
  bool (*verified_boot_read_permanent_attributes_hash)(ZirconBootOps* ops, uint8_t* hash);

  // Generate requested number of random bytes. The function is used for generating unlock
  // challenge.
  //
  // @ops: Pointer to the ZirconBootOps the device implements.
  // @num_bytes: Number of requested random bytes to generate.
  // @output: Output buffer that stores the generated random bytes.
  //
  // Returns true on success.
  bool (*verified_boot_get_random)(ZirconBootOps* ops, size_t num_bytes, uint8_t* output);

  // Returns a buffer of at least 'size' bytes.
  // This function will be called during boot to locate the buffer into which the
  // ZBI will be loaded from disk. This buffer may be static or dynamically
  // allocated, and should provide enough extra space to accommodate any additional
  // ZBI items that the bootloader will append before booting.
  //
  // @ops: Pointer to the ZirconBootOps that host this ZirconVBootOps object.
  // @request_size: As an input parameter, size of kernel load buffer to return.
  //                As an output, describes the actual size of the buffer in bytes.
  //                If returned buffer is not NULL, this must be at least
  //                as large as the size requested.
  //
  // Returns a pointer to a buffer at least 'size' bytes long, or NULL on error.
  uint8_t* (*get_kernel_load_buffer)(ZirconBootOps* ops, size_t* size);
};

// Flags to control boot behavior.
//
// Default behavior with no flags is to boot from {zircon,vbmeta}_{a,b,r}
// partitions based on the state of the A/B/R metadata loaded from disk.
//
// Multiple flags can be combined except where indicated in the documentation.
typedef enum ZirconBootFlags {
  // No flags, use default behavior.
  kZirconBootFlagsNone = 0,

  // Boot from {zircon,vbmeta}_r partitions, no A/B/R metadata used.
  // Cannot be combined with `kZirconBootFlagsSlotless`.
  kZirconBootFlagsForceRecovery = (1 << 0),

  // Boot from {zircon,vbmeta} partitions, no A/B/R metadata used.
  // Cannot be combined with `kZirconBootFlagsForceRecovery`.
  kZirconBootFlagsSlotless = (1 << 1),

  // Support Android Boot Image format in addition to ZBI format.
  //
  // In this case only the kernel data will be extracted, any other data in the
  // Android Boot Image will be ignored.
  //
  // The kernel data is expected to contain a ZBI, optionally with some trailing
  // data which will be ignored. If the caller needs this trailing data it has
  // to be extracted separately.
  //
  // If vbmeta verification is supported in the given `ZirconBootOps`, the
  // vbmeta image will only verify the contained ZBI; the Android boot image
  // as a whole is not verified as it's only used to locate the ZBI.
  kZirconBootFlagsAndroidBootImage = (1 << 2)
} ZirconBootFlags;

// Loads kernel image into memory and boots it. if ops.get_firmware_slot is set, the function
// boots according to firwmare ABR. Otherwise it boots according to OS ABR.
//
// @ops: Required operations.
// @boot_flags: boot flags as defined in ZirconBootFlags.
//
// The function is not expected to return if boot is successful.
ZirconBootResult LoadAndBoot(ZirconBootOps* ops, uint32_t boot_flags);

// Loads and verifies kernel image from RAM.
//
// The provided image can be any of:
//   * a ZBI
//   * a ZBI immediately followed by a vbmeta image
//   * an Android boot image containing either of the above
//
// This function will verify the ZBI image according to the lock state, and
// on success copy it to the kernel load buffer as in LoadAndBoot(), but will
// return rather than booting. This is to allow callers to take action based
// on whether it succeeds or fails (e.g. complete a fastboot message).
//
// @ops: Required operations.
// @image: Image in RAM in one of the supported formats listed above.
// @size: Image size.
// @zbi: On success, will be set to the kernel load buffer with the ZBI image.
//       This is the same buffer provided by the get_kernel_load_buffer() op,
//       provided here for convenience so the implementation doesn't have to
//       store the buffer address elsewhere.
// @zbi_capacity: On success, will be set to the kernel load buffer capacity.
//
// Returns the boot result. The caller must only proceed to boot the kernel
// on kBootResultOK.
ZirconBootResult LoadFromRam(ZirconBootOps* ops, const void* image, size_t size, zbi_header_t** zbi,
                             size_t* zbi_capacity);

// Gets the slot that will be selected to boot according to current A/B/R metadata.
// Specifically, this is the slot that will be booted by LoadAndBoot() without any
// boot flags, assuming the slot passes all verification.
AbrSlotIndex GetActiveBootSlot(ZirconBootOps* ops);

// Creates operations for libabr from a ZirconBootOps.
AbrOps GetAbrOpsFromZirconBootOps(ZirconBootOps* ops);

// Returns the zircon partition name of a given slot, the slotless partition
// name if |slot| is NULL.
const char* GetSlotPartitionName(const AbrSlotIndex* slot);

// The API can be used in the firmware (i.e. fastboot) to generate an unlock challenge
// for the host side avb tooling to perform authenticated unlocking.
//
// @ops: Pointer to the ZirconBootOps the device implements.
// @out_unlock_challenge: Pointer to the output unlock challenge .
//
// Returns true if successful.
bool ZirconVbootGenerateUnlockChallenge(ZirconBootOps* ops,
                                        AvbAtxUnlockChallenge* out_unlock_challenge);

// The API is used in the firmware to validate the authenticated unlock credential sent from
// the host.
//
// @ops: Pointer to the ZirconBootOps the device implements.
// @unlock_credential: Pointer to the input unlock credential data.
//
// Returns true if credential is verified successfully without error.
bool ZirconVbootValidateUnlockCredential(ZirconBootOps* ops,
                                         const AvbAtxUnlockCredential* unlock_credential);

// Loads firmware from bootloader_a/b/r partition according to current ABR metadata. The API
// internally calls GetActiveBootSlot() and uses the result to decide which of
// bootloader_a/b/r partition to load the firmware.
//
// Note: The API will not modify ABR metadata on the storage. In the case of one-shot-recovery,
// the API will load from bootloader_r but will not clear the flag. It is strongly suggested that
// the next stage firmware use `LoadAndBoot()` with `ZirconBootOps::firmware_can_boot_kernel_slot()`
// implemented to continue boting. This makes sure that ABR metadata is properly updated.
// `firmware_can_boot_kernel_slot()` shall indicate which OS slot a firmware slot is allowed to boot
// to. This should usually be exact same slot, i.e. bootloader_a only boots to zircon_a. See
// src/firmware/lib/zircon_boot/gpt_boot_reference.c for intended usage.
//
// @ops: Required operations. Only the `read_from_partition` callback is required.
// @dst: Destination address.
// @size: Firmware image size.
//
// Returns true if succeed.
bool LoadAbrFirmware(ZirconBootOps* ops, void* dst, size_t size);

#ifdef __cplusplus
}
#endif

#endif  // SRC_FIRMWARE_LIB_ZIRCON_BOOT_INCLUDE_LIB_ZIRCON_BOOT_ZIRCON_BOOT_H_
