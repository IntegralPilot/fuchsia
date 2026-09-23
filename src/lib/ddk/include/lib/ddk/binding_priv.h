// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DDK_INCLUDE_LIB_DDK_BINDING_PRIV_H_
#define SRC_LIB_DDK_INCLUDE_LIB_DDK_BINDING_PRIV_H_

#include <assert.h>
#include <stdalign.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <zircon/compiler.h>

__BEGIN_CDECLS

// LINT.IfChange
// global binding variables at 0x00XX
// BIND_FLAGS was 0x0000, the value of the flags register
// #define BIND_PROTOCOL 0x0001  // primary protocol of the device
// BIND_AUTOBIND was 0x0002, if this is an automated bind/load
// BIND_COMPOSITE was 0x003, whether this is a composite device

// pci binding variables at 0x01XX
// BIND_PCI_VID was 0x0100
// BIND_PCI_DID was 0x0101
// BIND_PCI_CLASS was 0x0102
// BIND_PCI_SUBCLASS was 0x0103
// BIND_PCI_INTERFACE was 0x0104
// BIND_PCI_REVISION was 0x0105
// BIND_PCI_TOPO was 0x0107

#define BIND_PCI_TOPO_PACK(bus, dev, func) (((bus) << 8) | (dev << 3) | (func))

// usb binding variables at 0x02XX
// these are used for both ZX_PROTOCOL_USB_INTERFACE and ZX_PROTOCOL_USB_FUNCTION
// BIND_USB_VID as 0x0200
// BIND_USB_PID as 0x0201
// BIND_USB_CLASS as 0x0202
// BIND_USB_SUBCLASS as 0x0203
// BIND_USB_PROTOCOL as 0x0204
// BIND_USB_INTERFACE_NUMBER as 0x0205

// Platform bus binding variables at 0x03XX
// BIND_PLATFORM_DEV_VID was 0x0300
// BIND_PLATFORM_DEV_PID was 0x0301
// BIND_PLATFORM_DEV_DID was 0x0302
// BIND_PLATFORM_DEV_INSTANCE_ID was 0x0304
// BIND_PLATFORM_DEV_INTERRUPT_ID was 0x0305

// ACPI binding variables at 0x04XX
// BIND_ACPI_BUS_TYPE was 0x0400
// Internal use only.
// BIND_ACPI_ID was 0x0401

// Intel HDA Codec binding variables at 0x05XX
// BIND_IHDA_CODEC_VID as 0x0500
// BIND_IHDA_CODEC_DID as 0x0501
// BIND_IHDA_CODEC_MAJOR_REV as 0x0502
// BIND_IHDA_CODEC_MINOR_REV as 0x0503
// BIND_IHDA_CODEC_VENDOR_REV as 0x0504
// BIND_IHDA_CODEC_VENDOR_STEP as 0x0505

// Serial binding variables at 0x06XX
// BIND_SERIAL_CLASS was 0x0600
// BIND_SERIAL_VID was 0x0601
// BIND_SERIAL_PID was 0x0602

// NAND binding variables at 0x07XX
// BIND_NAND_CLASS was 0x0700

// SDIO binding variables at 0x09XX
// BIND_SDIO_VID was 0x0900
// BIND_SDIO_PID was 0x0901
// BIND_SDIO_FUNCTION was 0x0902

// I2C binding variables at 0x0A0X
// BIND_I2C_CLASS was 0x0A00
// BIND_I2C_BUS_ID was 0x0A01
// BIND_I2C_ADDRESS was 0x0A02
// BIND_I2C_VID was 0x0A03
// BIND_I2C_DID was 0x0A04

// GPIO binding variables at 0x0A1X
// BIND_GPIO_PIN was 0x0A10
// BIND_GPIO_CONTROLLER was 0x0A11

// POWER binding variables at 0x0A2X
// BIND_POWER_DOMAIN was 0x0A20
// BIND_POWER_DOMAIN_COMPOSITE was 0x0A21

// POWER binding variables at 0x0A3X
// BIND_CLOCK_ID was 0x0A30

// SPI binding variables at 0x0A4X
// BIND_SPI_BUS_ID was 0x0A41
// BIND_SPI_CHIP_SELECT was 0x0A42

// PWM binding variables at 0x0A5X
// BIND_PWM_ID was 0x0A50

// Init Step binding variables at 0x0A6X
// BIND_INIT_STEP was 0x0A60

// Codec binding variables at 0x0A7X
// BIND_CODEC_INSTANCE was 0x0A70

// 0x0A80 was BIND_REGISTER_ID which is now deprecated.

// Power sensor binding variables at 0x0A9X
// BIND_POWER_SENSOR_DOMAIN was 0x0A90
// LINT.ThenChange(/sdk/lib/driver/legacy-bind-constants/legacy-bind-constants.h)

#define ZIRCON_NOTE_NAME "Zircon"
#define ZIRCON_NOTE_DRIVER 0x31565244  // DRV1

typedef struct {
  // Elf64_Nhdr fields:
  uint32_t namesz;
  uint32_t descsz;
  uint32_t type;
  // ELF note name.  namesz is the exact size of the name (including '\0'),
  // but the storage size is always rounded up to a multiple of 4 bytes.
  char name[(sizeof(ZIRCON_NOTE_NAME) + 3) & -4];
} zircon_driver_note_header_t;

#define ZIRCON_DRIVER_NOTE_HEADER_INIT(object)                                \
  {                                                                           \
      /* .namesz = */ sizeof(ZIRCON_NOTE_NAME),                               \
      /* .descsz = */ (sizeof(object) - sizeof(zircon_driver_note_header_t)), \
      /* .type = */ ZIRCON_NOTE_DRIVER,                                       \
      /* .name = */ ZIRCON_NOTE_NAME,                                         \
  }

typedef struct {
  // See flag bits below.
  uint32_t flags;

  // Driver Metadata
  uint32_t reserved0;
  char name[32];
  char vendor[16];
  char version[16];

} zircon_driver_note_payload_t;

// Flag bits in the driver note:

// Driver is built with `-fsanitize=address` and can only be loaded into a
// devhost that supports the ASan runtime.
#define ZIRCON_DRIVER_NOTE_FLAG_ASAN (1u << 0)

#define ZIRCON_DRIVER_NOTE_PAYLOAD_INIT(Driver, VendorName, Version) \
  {                                                                  \
      /* .flags = */ ZIRCON_DRIVER_NOTE_FLAGS,                       \
      /* .reserved0 = */ 0,                                          \
      /* .name = */ #Driver,                                         \
      /* .vendor = */ VendorName,                                    \
      /* .version = */ Version,                                      \
  }

#define ZIRCON_DRIVER_NOTE_FLAGS \
  (__has_feature(address_sanitizer) ? ZIRCON_DRIVER_NOTE_FLAG_ASAN : 0)

typedef struct {
  zircon_driver_note_header_t header;
  zircon_driver_note_payload_t payload;
} zircon_driver_note_t;

static_assert(offsetof(zircon_driver_note_t, payload) == sizeof(zircon_driver_note_header_t),
              "alignment snafu?");

// Without this, ASan will add redzone padding after the object, which
// would make it invalid ELF note format.
#if __has_feature(address_sanitizer)
#define ZIRCON_DRIVER_NOTE_ASAN __attribute__((no_sanitize("address")))
#else
#define ZIRCON_DRIVER_NOTE_ASAN
#endif

// GCC has a quirk about how '__attribute__((visibility("default")))'
// (__EXPORT here) works for const variables in C++.  The attribute has no
// effect when used on the definition of a const variable, and GCC gives a
// warning/error about that.  The attribute must appear on the "extern"
// declaration of the variable instead.

// We explicitly align the note to 4 bytes.  That's its natural alignment
// anyway, but the compilers sometimes like to over-align as an
// optimization while other tools sometimes like to complain if SHT_NOTE
// sections are over-aligned (since this could result in padding being
// inserted that makes it violate the ELF note format).  Standard C11
// doesn't permit alignas(...) on a type but we could use __ALIGNED(4) on
// all the types (i.e. GNU __attribute__ syntax instead of C11 syntax).
// But the alignment of the types is not actually the issue: it's the
// compiler deciding to over-align the individual object regardless of its
// type's alignment, so we have to explicitly set the alignment of the
// object to defeat any compiler default over-alignment.

// C macro for driver binding rules. Unlike the old bytecode format, the
// instructions in the new format are not represented by three uint32 integers.
// To support both formats simultaneously, |binding| is used for the old
// bytecode instructions while |bytecode| is used for the new bytecode
// instructions.
#define ZIRCON_DRIVER_PRIV(Driver, Ops, VendorName, Version)                 \
  zx_driver_rec_t __zircon_driver_rec__ __EXPORT = {                         \
      /* .ops = */ &(Ops),                                                   \
      /* .driver = */ NULL,                                                  \
  };                                                                         \
  extern const struct zircon_driver_note __zircon_driver_note__ __EXPORT;    \
  alignas(4) __SECTION(".note.zircon.driver." #Driver)                       \
      ZIRCON_DRIVER_NOTE_ASAN const struct zircon_driver_note {              \
    zircon_driver_note_t note;                                               \
  } __zircon_driver_note__ = {                                               \
      /* .note = */ {ZIRCON_DRIVER_NOTE_HEADER_INIT(__zircon_driver_note__), \
                     ZIRCON_DRIVER_NOTE_PAYLOAD_INIT(Driver, VendorName, Version)}}

// TODO: if we moved the Ops from the BEGIN() to END() macro we
//      could add a zircon_driver_note_t* to the zx_driver_rec_t,
//      define it in END(), and have only one symbol to dlsym()
//      when loading drivers

__END_CDECLS

#endif  // SRC_LIB_DDK_INCLUDE_LIB_DDK_BINDING_PRIV_H_
