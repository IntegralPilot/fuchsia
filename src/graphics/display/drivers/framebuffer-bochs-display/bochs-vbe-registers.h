// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_FRAMEBUFFER_BOCHS_DISPLAY_BOCHS_VBE_REGISTERS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_FRAMEBUFFER_BOCHS_DISPLAY_BOCHS_VBE_REGISTERS_H_

#include <zircon/assert.h>

#include <cstdint>
#include <limits>

#include <hwreg/bitfields.h>

namespace bochs_vbe {

// Bochs VBE Display Registers
//
// The Bochs display engine (also known as Bochs Graphics Adapter, BGA) is a
// display engine that supports the VESA Bios Extensions (VBE) standard. The
// emulated display engine communicates with the display driver using the
// registers defined by the Bochs VBE Display API. The API is also referred to
// as "DISPI" (Bochs Display Interface) in the Bochs documentation and register
// names.
//
// The API consists of 16-bit registers. The Bochs Display API does not
// explicitly provide register names, instead it refers to the registers
// using the constants that convey the addresses. These constants are all
// prefixed with `VBE_DISPI_INDEX_` (VBE Display Interface register Index).
//
// Other emulators may implement Bochs-compatible display engines. For example,
// the QEMU emulator implements a display engine `bochs-display` that is
// fully compatible with the Bochs VBE Display API.
//
// The Bochs VBE Display API is defined in
// Jeroen Janssen, VBE Display API, version 0.6, Nov 23 2002.
// https://github.com/bochs-emu/VGABIOS/blob/v0_9b/vgabios/vbe_display_api.txt

// Minimum Bochs VBE display engine API version assumed by the class
// definitions in this header.
//
// The Bochs VBE Display API is not fully backward compatible. Registers in
// API versions lower than the `kMinimumSupportedDisplayEngineApiVersion` may
// be defined in a way not compatible with the class definitions in this file.
//
// We chose to not support the old API versions as there's no modern emulator
// supporting them.
constexpr uint16_t kMinimumSupportedDisplayEngineApiVersion = 0xB0C2;

// VBE_DISPI_BANK_ADDRESS
//
// Address of the current active video memory bank, if the display engine is in
// banked mode.
//
// Supported by all API versions.
constexpr uintptr_t kVideoMemoryBankAddress = 0xA0000;

// VBE_DISPI_BANK_SIZE_KB
//
// Size of a video memory bank in KiB (1024 bytes), if the display engine is in
// banked mode.
//
// Supported by all API versions.
constexpr int kVideoMemoryBankSizeKb = 64;
constexpr int kVideoMemoryBankSizeBytes = kVideoMemoryBankSizeKb * 1024;

// VBE_DISPI_INDEX_ID
//
// Supported by all Bochs / Bochs-like VBE display engine hardware.
//
// Writing to this register notifies the display engine hardware of the
// display engine API version that the driver supports.
//
// Reading from this register returns the display engine API version that the
// display engine hardware supports.
class DisplayEngineApiVersion : public hwreg::RegisterBase<DisplayEngineApiVersion, uint16_t> {
 public:
  static hwreg::RegisterAddr<DisplayEngineApiVersion> Get() { return {0x00 * sizeof(uint16_t)}; }

  // VBE_DISPI_ID0
  static constexpr uint16_t kVersion0 = 0xB0C0;
  // VBE_DISPI_ID1
  static constexpr uint16_t kVersion1 = 0xB0C1;
  // VBE_DISPI_ID2
  static constexpr uint16_t kVersion2 = 0xB0C2;
  // VBE_DISPI_ID3
  static constexpr uint16_t kVersion3 = 0xB0C3;
  // VBE_DISPI_ID4
  static constexpr uint16_t kVersion4 = 0xB0C4;
  // VBE_DISPI_ID5
  static constexpr uint16_t kVersion5 = 0xB0C5;

  DEF_FIELD(15, 0, version);
};

// VBE_DISPI_INDEX_XRES
//
// The horizontal resolution of the display engine hardware, in pixels.
//
// Supported by all API versions.
//
// This register must be written only if the display engine is disabled.
class DisplayHorizontalResolution
    : public hwreg::RegisterBase<DisplayHorizontalResolution, uint16_t> {
 public:
  // VBE_DISPI_MAX_XRES
  //
  // This was designed as part of the Bochs VBE display engine API but actually
  // all the VBE implementations have different values:
  // - The "VBE Display API" specifies that VBE_DISPI_MAX_XRES is 1024.
  // - The Bochs 2.8 implementation defines VBE_DISPI_MAX_XRES to be 2560.
  // - The QEMU 9.0.0 implementation defines VBE_DISPI_MAX_XRES to be 16000.
  // We choose the QEMU definition here.
  static constexpr int kMaximumAllowedHorizontalResolutionPx = 16'000;

  static hwreg::RegisterAddr<DisplayHorizontalResolution> Get() {
    return {0x01 * sizeof(uint16_t)};
  }

  // `pixels` and `set_pixels` helpers are preferred over direct field
  // manipulations.
  DEF_FIELD(15, 0, pixels_raw);

  int pixels() const { return int{pixels_raw()}; }

  // `pixels` must be non-negative and must not exceed
  // `kMaximumAllowedHorizontalResolutionPx`.
  DisplayHorizontalResolution& set_pixels(int pixels) {
    ZX_DEBUG_ASSERT(pixels >= 0);
    ZX_DEBUG_ASSERT(pixels <= kMaximumAllowedHorizontalResolutionPx);
    // `kMaximumAllowedHorizontalResolutionPx` <= UINT16_MAX. The static_cast
    // won't overflow and cause an undefined behavior.
    return set_pixels_raw(static_cast<uint16_t>(pixels));
  }
};

// VBE_DISPI_INDEX_YRES
//
// The vertical resolution of the display engine hardware, in pixels.
//
// Supported by all API versions.
//
// This register must be written only if the display engine is disabled.
class DisplayVerticalResolution : public hwreg::RegisterBase<DisplayVerticalResolution, uint16_t> {
 public:
  // VBE_DISPI_MAX_YRES
  //
  // This was designed as part of the Bochs VBE display engine API but actually
  // all the VBE implementations have different values:
  // - The "VBE Display API" specifies that VBE_DISPI_MAX_YRES is 768.
  // - The Bochs 2.8 implementation defines VBE_DISPI_MAX_YRES to be 1600.
  // - The QEMU 9.0.0 implementation defines VBE_DISPI_MAX_YRES to be 12000.
  static constexpr int kMaximumAllowedVerticalResolutionPx = 12'000;

  static hwreg::RegisterAddr<DisplayVerticalResolution> Get() { return {0x02 * sizeof(uint16_t)}; }

  // `pixels` and `set_pixels` helpers are preferred over direct field
  // manipulations.
  DEF_FIELD(15, 0, pixels_raw);

  int pixels() const { return int{pixels_raw()}; }

  // `pixels` must be non-negative and must not exceed
  // `kMaximumAllowedVerticalResolutionPx`.
  DisplayVerticalResolution& set_pixels(int pixels) {
    ZX_DEBUG_ASSERT(pixels >= 0);
    ZX_DEBUG_ASSERT(pixels <= kMaximumAllowedVerticalResolutionPx);
    // `kMaximumAllowedVerticalResolutionPx` <= UINT16_MAX. The static_cast
    // won't overflow and cause an undefined behavior.
    return set_pixels_raw(static_cast<uint16_t>(pixels));
  }
};

// VBE_DISPI_INDEX_BPP
//
// Number of bits per pixel stored in the video memory.
//
// This register definition only applies to display engine API versions 2 and
// above. The API version 1 and below has a different definition for this
// register. We chose to not support the old API version as there's no modern
// emulator that supports it.
//
// This register must be written only if the display engine is disabled.
class DisplayBitsPerPixel : public hwreg::RegisterBase<DisplayBitsPerPixel, uint16_t> {
 public:
  // Documented values for `bits_per_pixel_selection`.
  enum class BitsPerPixelSelection : uint8_t {
    k8ForBackwardCompatibility = 0,
    k8 = 8,
    k15 = 15,
    k16 = 16,
    k24 = 24,
    k32 = 32,
  };

  static hwreg::RegisterAddr<DisplayBitsPerPixel> Get() { return {0x03 * sizeof(uint16_t)}; }

  // `GetBitsPerPixel` and `SetBitsPerPixel` helpers are preferred over direct
  // field manipulations.
  DEF_ENUM_FIELD(BitsPerPixelSelection, 15, 0, bits_per_pixel_selection);

  int bits_per_pixel() const {
    switch (bits_per_pixel_selection()) {
      case BitsPerPixelSelection::k8ForBackwardCompatibility:
      case BitsPerPixelSelection::k8:
        return 8;
      case BitsPerPixelSelection::k15:
        return 15;
      case BitsPerPixelSelection::k16:
        return 16;
      case BitsPerPixelSelection::k24:
        return 24;
      case BitsPerPixelSelection::k32:
        return 32;
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid bits per pixel selection: %d",
                        static_cast<int>(bits_per_pixel_selection()));
    return static_cast<int>(bits_per_pixel_selection());
  }

  DisplayBitsPerPixel& set_bits_per_pixel(int bits_per_pixel) {
    switch (bits_per_pixel) {
      case 8:
        return set_bits_per_pixel_selection(BitsPerPixelSelection::k8);
      case 15:
        return set_bits_per_pixel_selection(BitsPerPixelSelection::k15);
      case 16:
        return set_bits_per_pixel_selection(BitsPerPixelSelection::k16);
      case 24:
        return set_bits_per_pixel_selection(BitsPerPixelSelection::k24);
      case 32:
        return set_bits_per_pixel_selection(BitsPerPixelSelection::k32);
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid number of bits per pixel: %d", bits_per_pixel);
    return set_bits_per_pixel_selection(static_cast<BitsPerPixelSelection>(bits_per_pixel));
  }
};

// VBE_DISPI_INDEX_ENABLE
//
// Enables / disables features of the display engine.
//
// Supported by all API versions.
//
// This register has bits that are reserved but not MBZ (must be zero). So, it
// can only be safely updated via read-modify-write operations.
class DisplayFeatureControl : public hwreg::RegisterBase<DisplayFeatureControl, uint16_t> {
 public:
  static hwreg::RegisterAddr<DisplayFeatureControl> Get() { return {0x04 * sizeof(uint16_t)}; }

  // VBE_DISPI_NOCLEARMEM
  //
  // The video memory is not cleared when the display engine is enabled iff
  // true.
  //
  // Supported by API versions 2 and above.
  DEF_BIT(7, video_memory_preserved_on_enable);

  // VBE_DISPI_LFB_ENABLED
  //
  // If true, the display engine hardware lays out the video memory as a linear
  // frame buffer (LFB).
  //
  // Supported by API versions 2 and above.
  DEF_BIT(6, linear_frame_buffer_enabled);

  // VBE_DISPI_8BIT_DAC
  //
  // If true, the palette digital-to-analog converter (DAC) is in 8-bit mode.
  // Otherwise, the palette DAC is in 6-bit mode.
  //
  // Supported by API versions 3 and above.
  DEF_BIT(5, palette_dac_in_8bit_mode);

  // VBE_DISPI_GETCAPS
  //
  // If true, reading the `DisplayHorizontalResolution`,
  // `DisplayVerticalResolution`, and `DisplayBitsPerPixel` registers returns
  // the emulator's GUI capability.
  // Otherwise, reading these registers returns the current values set by the
  // driver.
  //
  // Supported by API versions 3 and above.
  DEF_BIT(1, read_display_capabilities);

  // VBE_DISPI_ENABLED
  //
  // Supported by all API versions.
  DEF_BIT(0, display_engine_enabled);
};

// VBE_DISPI_INDEX_BANK
//
// Index of the current active memory bank of the video memory.
//
// Supported by all API versions.
//
// The Bochs display engine supports banked memory access. The video memory
// is divided in banks of `kVideoMemoryBankSizeBytes` in size.
//
// A display driver may access a certain video memory bank by writing its
// index to the `VideoMemoryBankIndex` register, and then read / write at the
// memory address `kVideoMemoryBankAddress`.
class VideoMemoryBankIndex : public hwreg::RegisterBase<VideoMemoryBankIndex, uint16_t> {
 public:
  static hwreg::RegisterAddr<VideoMemoryBankIndex> Get() { return {0x05 * sizeof(uint16_t)}; }

  // Must be less than `VideoMemorySize::GetBankCount()`.
  DEF_FIELD(15, 0, bank_index);
};

// Virtual Display
//
// The Bochs display engine allows a virtual display canvas that is larger than
// the physical display.
//
// Updates on the frame buffer are reflected on the rectangular region on the
// virtual display canvas, with the size of the physical display and the offset
// specified by the `VirtualDisplay{Horizontal,Vertical}Offset` registers.
//
// The virtual display feature is only supported by API versions 1 and above.

// VBE_DISPI_INDEX_VIRT_WIDTH
//
// The width of the virtual display canvas in pixels.
//
// Supported by API versions >= DisplayEngineApiVersion::kVersion1.
class VirtualDisplayWidth : public hwreg::RegisterBase<VirtualDisplayWidth, uint16_t> {
 public:
  static hwreg::RegisterAddr<VirtualDisplayWidth> Get() { return {0x06 * sizeof(uint16_t)}; }

  // `pixels` and `set_pixels` helpers are preferred over direct field
  // manipulations.
  DEF_FIELD(15, 0, pixels_raw);

  int pixels() const { return int{pixels_raw()}; }

  // `pixels` must be non-negative and must not exceed (2^16 - 1) = 65,535.
  VirtualDisplayWidth& set_pixels(int pixels) {
    ZX_DEBUG_ASSERT(pixels >= 0);
    ZX_DEBUG_ASSERT(pixels <= std::numeric_limits<uint16_t>::max());
    // The static_cast won't overflow and cause an undefined behavior.
    return set_pixels_raw(static_cast<uint16_t>(pixels));
  }
};

// VBE_DISPI_INDEX_VIRT_HEIGHT
//
// The height of the virtual display canvas in pixels.
//
// Supported by API versions 1 and above.
class VirtualDisplayHeight : public hwreg::RegisterBase<VirtualDisplayHeight, uint16_t> {
 public:
  static hwreg::RegisterAddr<VirtualDisplayHeight> Get() { return {0x07 * sizeof(uint16_t)}; }

  // `pixels` and `set_pixels` helpers are preferred over direct field
  // manipulations.
  DEF_FIELD(15, 0, pixels_raw);

  int pixels() const { return int{pixels_raw()}; }

  // `pixels` must be non-negative and must not exceed (2^16 - 1) = 65,535.
  VirtualDisplayHeight& set_pixels(int height_px) {
    ZX_DEBUG_ASSERT(height_px >= 0);
    ZX_DEBUG_ASSERT(height_px <= std::numeric_limits<uint16_t>::max());
    // The static_cast won't overflow and cause an undefined behavior.
    return set_pixels_raw(static_cast<uint16_t>(height_px));
  }
};

// VBE_DISPI_INDEX_X_OFFSET
//
// The horizontal offset of the physical display on the virtual display canvas,
// in pixels.
//
// Supported by API versions 1 and above.
//
// Writing to the register with a new offset triggers a complete screen refresh.
class VirtualDisplayHorizontalOffset
    : public hwreg::RegisterBase<VirtualDisplayHorizontalOffset, uint16_t> {
 public:
  static hwreg::RegisterAddr<VirtualDisplayHorizontalOffset> Get() {
    return {0x08 * sizeof(uint16_t)};
  }

  // `pixels` and `set_pixels` helpers are preferred over direct field
  // manipulations.
  DEF_FIELD(15, 0, pixels_raw);

  int pixels() const { return int{pixels_raw()}; }

  // `pixels` must be non-negative and must not exceed (2^16 - 1) = 65535.
  // `pixels` must not exceed the width of the virtual display.
  VirtualDisplayHorizontalOffset& set_pixels(int pixels) {
    ZX_DEBUG_ASSERT(pixels >= 0);
    ZX_DEBUG_ASSERT(pixels <= std::numeric_limits<uint16_t>::max());
    // The static_cast won't overflow and cause an undefined behavior.
    return set_pixels_raw(static_cast<uint16_t>(pixels));
  }
};

// VBE_DISPI_INDEX_Y_OFFSET
//
// The vertical offset of the physical display on the virtual display canvas,
// in pixels.
//
// Supported by API versions 1 and above.
//
// Writing to the register with a new offset triggers a complete screen refresh.
class VirtualDisplayVerticalOffset
    : public hwreg::RegisterBase<VirtualDisplayVerticalOffset, uint16_t> {
 public:
  static hwreg::RegisterAddr<VirtualDisplayVerticalOffset> Get() {
    return {0x09 * sizeof(uint16_t)};
  }

  // `pixels` and `set_pixels` helpers are preferred over direct field
  // manipulations.
  DEF_FIELD(15, 0, pixels_raw);

  int pixels() const { return int{pixels_raw()}; }

  // `pixels` must be non-negative and must not exceed (2^16 - 1) = 65535.
  // `pixels` must not exceed the width of the virtual display.
  VirtualDisplayVerticalOffset& set_pixels(int pixels) {
    ZX_DEBUG_ASSERT(pixels >= 0);
    ZX_DEBUG_ASSERT(pixels <= std::numeric_limits<uint16_t>::max());
    // The static_cast won't overflow and cause an undefined behavior.
    return set_pixels_raw(static_cast<uint16_t>(pixels));
  }
};

// VBE_DISPI_INDEX_VIDEO_MEMORY_64K
//
// The video memory size, in units of 64 KiB (65,536 bytes).
//
// Supported by API versions 5 and above.
//
// This register is read-only.
class VideoMemorySize : public hwreg::RegisterBase<VideoMemorySize, uint16_t> {
 public:
  // The register index constant `VBE_DISPI_INDEX_VIDEO_MEMORY_64K` is
  // documented in the Bochs VBE Display API but its value is not documented.
  // Experiments showed that the register address is 0x0a on Bochs and QEMU
  // emulators.
  static hwreg::RegisterAddr<VideoMemorySize> Get() { return {0x0a * sizeof(uint16_t)}; }

  // The `GetVideoMemorySizeBytes` helper method is preferred over direct field
  // manipulations.
  DEF_FIELD(15, 0, size_divided_by_64kb);

  int64_t GetBankCount() const {
    int64_t video_memory_size_bytes = GetVideoMemorySizeBytes();
    // bank count = ceil(video_memory_size_bytes / kVideoMemoryBankSizeBytes);
    return (video_memory_size_bytes + kVideoMemoryBankSizeBytes - 1) / kVideoMemoryBankSizeBytes;
  }

  int64_t GetVideoMemorySizeBytes() const {
    static constexpr int64_t kMultiplier = 64 * 1024;
    return kMultiplier * size_divided_by_64kb();
  }
};

}  // namespace bochs_vbe

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_FRAMEBUFFER_BOCHS_DISPLAY_BOCHS_VBE_REGISTERS_H_
