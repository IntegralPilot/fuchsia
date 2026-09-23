// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_LIB_ZIRCON_BOOT_INCLUDE_LIB_ZIRCON_BOOT_ZBI_UTILS_H_
#define SRC_FIRMWARE_LIB_ZIRCON_BOOT_INCLUDE_LIB_ZIRCON_BOOT_ZBI_UTILS_H_

// This should point to the abr.h in the abr library in firmware sdk.
#include <lib/abr/abr.h>
// This should point to the zbi.h in the zbi library in firmware sdk..
#include <lib/zbi/zbi.h>

__BEGIN_CDECLS

// Appends a cmdline ZBI item containing the current slot information to a ZBI container.
// For example, "zvb.current_slot=_a"
//
// Returns ZBI_RESULT_OK on success, Error code otherwise.
zbi_result_t AppendCurrentSlotZbiItem(zbi_header_t* zbi, size_t capacity, AbrSlotIndex slot);

// Appends a file to a ZBI container. The function creates a new entry of ZBI_TYPE_BOOTLOADER_FILE
// type in the container, containing file name and content. The file will be available by the
// filesystem service in bootsvc.
//
// @zbi: Pointer to the container.
// @capacity: Size of the container.
// @name: Name of the file.
// @file_data: Content of the file.
// @file_data_size: Size of the content.
//
// Returns ZBI_RESULT_OK on success. Error code otherwise.
zbi_result_t AppendZbiFile(zbi_header_t* zbi, size_t capacity, const char* name,
                           const void* file_data, size_t file_data_size);

// Define this to enable APIs using the internal bootfs ZBI type:
// https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/zbi-format/include/lib/zbi-format/internal/bootfs.h.
//
// New boards should not use this type; it is only intended for use internally within the kernel
// and does not have the same stability guarantees as public ZBI types. This is only intended for
// legacy boards that are already using it.
#ifdef ZIRCON_BOOT_USE_INTERNAL_BOOTFS

// A callback function to read a factory file given the name.
//
// @context: Caller data for the function to be called with.
// @name: name of the file
// @capacity: Maximum size of the output buffer.
// @output: Pointer to the output buffer
// @out_len: Output pointer that stores the size of the data written.
//
// Returns true on success, false on failures.
typedef bool (*read_factory_t)(void* context, const char* name, size_t capacity, void* output,
                               size_t* out_len);

// Appends a list of file to a ZBI container as a factory bootfs item. The API will try to read as
// many files as possible. If a file fails to be read and added, it will be skipped.
//
// @zbi: Pointer to the container.
// @capacity: Size of the container.
// @file_names: Names of the files.
// @file_count: Size of `file_names`
// @read_factory: A function pointer that returns the payload according to file name in `file_names`
// @read_factory_context: Caller data that `read_factory` will be called with.
//
// Returns ZBI_RESULT_OK on success. Error code otherwise.
zbi_result_t AppendBootfsFactoryFiles(zbi_header_t* zbi, size_t capacity, const char** file_names,
                                      size_t file_count, read_factory_t read_factory,
                                      void* read_factory_context);

#endif  // ZIRCON_BOOT_USE_INTERNAL_BOOTFS

// Alignment requirement for the buffer to boot zircon kernel. At the time this is written,
// all our products on ARM use 64Kb alignment.
#ifndef ZIRCON_BOOT_KERNEL_ALIGN
#define ZIRCON_BOOT_KERNEL_ALIGN (64 * 1024)
#endif

// Extracts the kernel from `image` and relocate (copy) it to `relocate_addr`. Returns the kernel
// entry point address on success. Only applies to ARM kernel type.
//
// This is needed because zircon booting requires additional scratch memory following the kernel.
// See sdk/lib/zbi-format/include/lib/zbi-format/zbi.h for more details.
//
// @zbi: Pointer to the zbi image.
// @relocate_addr: Pointer to the relocation address.
// @size: As input, it is set to the size of the `relocate_addr`. As output and
//   when buffer is too small, it is set to the minimum required buffer size.
//
// Returns the kernel entry point address on success. NULL if `image` is not a zbi kernel or `size`
// is too small to hold the entire kernel.
void* RelocateKernel(const zbi_header_t* zbi, void* relocate_addr, size_t* size);

// Simimlar to RelocateKernel(), but instead of to a different buffer, it relocates the kernel to
// the end of the ZBI container.
//
// @zbi: Pointer to the zbi image.
// @capacity: As input, it is set to the capacity of the `zbi`. As output and when capacity is not
// enough, it is set to the minimum required capacity.
//
// Returns the kernel entry point address on success. NULL if `image` is not a zbi kernel or
// `capacity` is not enough to relocate the kernel.
void* RelocateKernelToTail(zbi_header_t* zbi, size_t* capacity);

__END_CDECLS

#endif  // SRC_FIRMWARE_LIB_ZIRCON_BOOT_INCLUDE_LIB_ZIRCON_BOOT_ZBI_UTILS_H_
