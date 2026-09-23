// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_ZBI_H_
#define ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_ZBI_H_

#include <phys/arch/zbi.h>

#ifdef __ASSEMBLER__

#include <zbi/asm/zbi.h>

#else

#include <lib/zbi-format/kernel.h>
#include <lib/zbi-format/zbi.h>

#endif  // #ifdef __ASSEMBLER__

#ifdef __ASSEMBLER__  // clang-format off

/// Define the layout of a ZBI item header.
///
/// See zbi_header_t in <lib/zbi-format/zbi.h>.
///
/// Parameters
///
///   * name
///     - Required: Symbol name to define
///
///   * type
///     - Required: The ZBI item type. See ZBI_TYPE_* in <lib/zbi-format/zbi.h>.
///
///   * length
///     - Required: The length of the item's payload.
///
///   * extra
///     - Optional: The "extra" value of the item.
///     - Default: 0
///
///   * align
///     - Optional: alignment of the header. Must be greater than or equal to
///       ZBI_ALIGNMENT.
///     - Default: ZBI_ALIGNMENT
///
.macro .zbi.header.object name, type, length, extra=0, align=ZBI_ALIGNMENT
  .if \align < ZBI_ALIGNMENT
    .error "alignment must be >= ZBI_ALIGNMENT"
  .endif
  .object \name, rodata, align=\align, nosection=nosection
    .int \type              // type
    .int \length            // length
    .int \extra             // extra
    .int ZBI_FLAGS_VERSION  // flags
    .int 0, 0               // reserved0, reserved1
    .int ZBI_ITEM_MAGIC     // magic
    .int ZBI_ITEM_NO_CRC32  // crc32
  .end_object
.endm  // .zbi.header.object

/// Define a ZBI container header.
///
/// Parameters
///
///   * name
///     - Required: Symbol name to define
///
///   * length
///     - Required: The length of the container's payload.
///
.macro .zbi.container.object name, length, align=ZBI_ALIGNMENT
  .zbi.header.object \name, type=ZBI_TYPE_CONTAINER, length=\length, extra=ZBI_CONTAINER_MAGIC, align=\align
.endm  // .zbi.container.object

/// Defines a kernel ZBI.
///
/// Parameters
///
///   * name
///     - Required: Symbol name to define
///
///   * end
///     - Required: Name of the symbol pointing to the end of the kernel payload.
///
.macro .zbi.kernel.image.object name, end
  .zbi.container.object \name, length=(\end - .L.zbi.kernel.image.object.header.\name\().\@), align=ARCH_ZBI_KERNEL_ALIGNMENT
  .zbi.header.object .L.zbi.kernel.image.object.header.\name\().\@, type=ARCH_ZBI_KERNEL_TYPE, length=(\end - .L.zbi.kernel.image.object.payload.\name\().\@)
  .label .L.zbi.kernel.image.object.payload.\name\().\@, type=object
.endm //.zbi.kernel.image.object

#else  // clang-format on

constexpr uint32_t kArchZbiKernelType = ARCH_ZBI_KERNEL_TYPE;

constexpr uintptr_t kArchZbiKernelAlignment = ARCH_ZBI_KERNEL_ALIGNMENT;

constexpr uintptr_t kArchZbiDataAlignment = ARCH_ZBI_DATA_ALIGNMENT;

// Represents (the headers of) a bootable ZBI, loaded into memory by the boot
// loader.
struct ZbiKernelImage {
  zbi_header_t hdr_file;
  zbi_header_t hdr_kernel;
  zbi_kernel_t data_kernel;
};

#endif  // ifdef __ASSEMBLER__

#endif  // ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_ZBI_H_
