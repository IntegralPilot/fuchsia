// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_EDID_EDID_H_
#define SRC_GRAPHICS_DISPLAY_LIB_EDID_EDID_H_

#include <lib/fit/result.h>
#include <lib/zx/result.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <span>
#include <string>

#include <fbl/vector.h>
#include <hwreg/bitfields.h>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/edid/internal/iterators.h"

// References
//
// The code contains references to the following documents.
//
// - VESA Enhanced Extended Display Identification Data (E-EDID) Standard,
//   Video Electronics Standards Association (VESA), Release A, Revision 2,
//   dated September 25, 2006, revised December 31, 2020.
//   Referenced as "E-EDID standard".
//   Available at https://vesa.org/vesa-standards/ .

namespace edid {

// The size of an EDID block;
static constexpr uint32_t kBlockSize = 128;

// The maximum number of blocks in an E-EDID.
//
// E-EDID standard, Section 2.2.1 "EDID Extensions: Order of the Blocks",
// page 16.
static constexpr int kMaxEdidBlockCount = 256;

// Definitions for parsing EDID data.

// EDID 18-byte detailed timing descriptor.
//
// Many of the parameters in the timing descriptor are split across
// multiple fields, so we define various accessors for reading them.
//
// See "Table 3.21 - Detailed Timing Definition - Part 1" (in Release
// A, Revision 2 of the EDID spec, 2006).
struct __PACKED DetailedTimingDescriptor {
  uint32_t horizontal_addressable() const {
    return horizontal_addressable_low | (horizontal_addressable_high() << 8);
  }
  uint32_t horizontal_blanking() const {
    return horizontal_blanking_low | (horizontal_blanking_high() << 8);
  }
  uint32_t vertical_addressable() const {
    return vertical_addressable_low | (vertical_addressable_high() << 8);
  }
  uint32_t vertical_blanking() const {
    return vertical_blanking_low | (vertical_blanking_high() << 8);
  }
  uint32_t horizontal_front_porch() const {
    return horizontal_front_porch_low | (horizontal_front_porch_high() << 8);
  }
  uint32_t horizontal_sync_pulse_width() const {
    return horizontal_sync_pulse_width_low | (horizontal_sync_pulse_width_high() << 8);
  }
  uint32_t vertical_front_porch() const {
    return vertical_front_porch_low() | (vertical_front_porch_high() << 4);
  }
  uint32_t vertical_sync_pulse_width() const {
    return vertical_sync_pulse_width_low() | (vertical_sync_pulse_width_high() << 4);
  }

  // Offset 0
  uint16_t pixel_clock_10khz;
  // Offset 2
  uint8_t horizontal_addressable_low;
  uint8_t horizontal_blanking_low;
  uint8_t horizontal_fields1;
  DEF_SUBFIELD(horizontal_fields1, 7, 4, horizontal_addressable_high);
  DEF_SUBFIELD(horizontal_fields1, 3, 0, horizontal_blanking_high);
  // Offset 5
  uint8_t vertical_addressable_low;
  uint8_t vertical_blanking_low;
  uint8_t vertical_fields1;
  DEF_SUBFIELD(vertical_fields1, 7, 4, vertical_addressable_high);
  DEF_SUBFIELD(vertical_fields1, 3, 0, vertical_blanking_high);
  // Offset 8
  uint8_t horizontal_front_porch_low;
  uint8_t horizontal_sync_pulse_width_low;
  // Offset 10
  uint8_t vertical_fields2;
  DEF_SUBFIELD(vertical_fields2, 7, 4, vertical_front_porch_low);
  DEF_SUBFIELD(vertical_fields2, 3, 0, vertical_sync_pulse_width_low);
  // Offset 11
  uint8_t combined;
  DEF_SUBFIELD(combined, 7, 6, horizontal_front_porch_high);
  DEF_SUBFIELD(combined, 5, 4, horizontal_sync_pulse_width_high);
  DEF_SUBFIELD(combined, 3, 2, vertical_front_porch_high);
  DEF_SUBFIELD(combined, 1, 0, vertical_sync_pulse_width_high);
  uint8_t rest[5];  // Fields that we don't need to read yet.
  uint8_t features;
  DEF_SUBBIT(features, 7, interlaced);
  DEF_SUBFIELD(features, 4, 3, type);
  DEF_SUBBIT(features, 2, vsync_polarity);
  DEF_SUBBIT(features, 1, hsync_polarity);
};
#define TYPE_ANALOG 0
#define TYPE_ANALOG_BIPOLAR 1
#define TYPE_DIGITAL_COMPOSITE 2
#define TYPE_DIGITAL_SEPARATE 3

static_assert(sizeof(DetailedTimingDescriptor) == 18, "Size check for EdidTimingDesc");

union __PACKED Descriptor {
  DetailedTimingDescriptor timing;
  struct Monitor {
    uint16_t generic_tag;
    uint8_t padding;
    uint8_t type;
    static constexpr uint8_t kDummyType = 0x10;
    static constexpr uint8_t kName = 0xfc;
    static constexpr uint8_t kSerial = 0xff;

    uint8_t padding2;
    uint8_t data[13];
  } monitor;
};
static_assert(sizeof(Descriptor) == 18, "bad struct");

struct __PACKED StandardTimingDescriptor {
  uint32_t horizontal_resolution() const { return (byte1 + 31) * 8; }
  uint32_t vertical_resolution(uint8_t edid_version, uint8_t edid_revision) const {
    if (aspect_ratio() == 0) {
      if (edid_version < 1 || (edid_version == 1 && edid_revision < 3)) {
        return horizontal_resolution();
      } else {
        return horizontal_resolution() * 10 / 16;
      }
    } else if (aspect_ratio() == 1) {
      return horizontal_resolution() * 3 / 4;
    } else if (aspect_ratio() == 2) {
      return horizontal_resolution() * 4 / 5;
    } else if (aspect_ratio() == 3) {
      return horizontal_resolution() * 9 / 16;
    } else {
      ZX_DEBUG_ASSERT(false);
      return 0;
    }
  }

  uint8_t byte1;
  uint8_t byte2;
  DEF_SUBFIELD(byte2, 7, 6, aspect_ratio);
  DEF_SUBFIELD(byte2, 5, 0, vertical_freq);
};

// This covers the "base" EDID data -- the first 128 bytes (block 0).  In
// many cases, that is all the display provides, but there may be more data
// in extension blocks.
//
// See "Table 3.1 - EDID Structure Version 1, Revision 4" (in Release
// A, Revision 2 of the EDID spec, 2006).
struct __PACKED BaseEdid {
  bool validate() const;
  // Not actually a tag, but the first byte will always be this
  static constexpr uint8_t kTag = 0x00;

  // Offset 0
  uint8_t header[8];
  uint8_t manufacturer_id1;
  uint8_t manufacturer_id2;
  uint16_t product_code;
  uint32_t serial_number;
  uint8_t unused1[2];
  uint8_t edid_version;
  uint8_t edid_revision;
  uint8_t video_input_definition;
  DEF_SUBBIT(video_input_definition, 7, digital);
  uint8_t horizontal_size_cm;
  uint8_t vertical_size_cm;
  uint8_t features_bitmap;
  DEF_SUBBIT(features_bitmap, 2, standard_srgb);

  uint8_t various[14];  // Fields that we don't need to read yet.
  StandardTimingDescriptor standard_timings[8];
  Descriptor detailed_descriptors[4];
  uint8_t num_extensions;
  uint8_t checksum_byte;
};

static_assert(offsetof(BaseEdid, edid_version) == 0x12, "Layout check");
static_assert(offsetof(BaseEdid, standard_timings) == 0x26, "Layout check");
static_assert(offsetof(BaseEdid, detailed_descriptors) == 0x36, "Layout check");
static_assert(offsetof(BaseEdid, num_extensions) == 0x7e, "Layout check");

// Version 3 of the CEA EDID Timing Extension
struct __PACKED CeaEdidTimingExtension {
  static constexpr uint8_t kTag = 0x02;
  bool validate() const;

  uint8_t tag;
  uint8_t revision_number;
  uint8_t dtd_start_idx;

  uint8_t combined;
  DEF_SUBBIT(combined, 7, underscan);
  DEF_SUBBIT(combined, 6, basic_audio);
  DEF_SUBBIT(combined, 5, ycbcr_444);
  DEF_SUBBIT(combined, 4, ycbcr_422);
  DEF_SUBFIELD(combined, 3, 0, native_format_dtds);

  uint8_t payload[123];
  uint8_t checksum_byte;
};

// Short audio descriptor from CEA EDID timing extension's data block collection.
struct ShortAudioDescriptor {
  static constexpr uint8_t kType = 1;

  uint8_t format_and_channels;
  DEF_SUBFIELD(format_and_channels, 6, 3, format);
  static constexpr uint8_t kLPcm = 1;
  DEF_SUBFIELD(format_and_channels, 2, 0, num_channels_minus_1);
  uint8_t sampling_frequencies;
  static constexpr uint8_t kHz192 = (1 << 6);
  static constexpr uint8_t kHz176 = (1 << 5);
  static constexpr uint8_t kHz96 = (1 << 4);
  static constexpr uint8_t kHz88 = (1 << 3);
  static constexpr uint8_t kHz48 = (1 << 2);
  static constexpr uint8_t kHz44 = (1 << 1);
  static constexpr uint8_t kHz32 = (1 << 0);
  uint8_t bitrate;
  static constexpr uint8_t kLpcm_24 = (1 << 2);
  static constexpr uint8_t kLpcm_20 = (1 << 1);
  static constexpr uint8_t kLpcm_16 = (1 << 0);
  DEF_SUBBIT(bitrate, 2, lpcm_24);
  DEF_SUBBIT(bitrate, 1, lpcm_20);
  DEF_SUBBIT(bitrate, 0, lpcm_16);
};
static_assert(sizeof(ShortAudioDescriptor) == 3, "Bad size for ShortAudioDescriptor");

// Short video descriptor from CEA EDID timing extension's data block collection.
struct ShortVideoDescriptor {
  static constexpr uint8_t kType = 2;

  uint8_t data;
  DEF_SUBBIT(data, 7, native);
  DEF_SUBFIELD(data, 6, 0, standard_mode_idx);
};
static_assert(sizeof(ShortVideoDescriptor) == 1, "Bad size for ShortVideoDescriptor");

// Vendor specific block from CEA EDID timing extension's data block collection.
struct VendorSpecificBlock {
  static constexpr uint8_t kType = 3;

  uint8_t vendor_number[3];
  uint8_t physical_addr_low;
  uint8_t physical_addr_high;
  // The payload contains vendor defined data. It is only valid up to the
  // index specified by the data block's length.
  uint8_t payload[26];
};
static_assert(sizeof(VendorSpecificBlock) == 31, "Bad size for VendorSpecificBlock");

// Short speaker descriptor from CEA EDID timing extension's data block collection.
struct ShortSpeakerDescriptor {
  static constexpr uint8_t kType = 4;

  uint8_t features;
  DEF_SUBBIT(features, 6, rear_left_right_center);
  DEF_SUBBIT(features, 5, front_left_right_center);
  DEF_SUBBIT(features, 4, rear_center);
  DEF_SUBBIT(features, 3, rear_left_right);
  DEF_SUBBIT(features, 2, front_center);
  DEF_SUBBIT(features, 1, lfe);
  DEF_SUBBIT(features, 0, front_left_right);
  uint8_t reserved;
  uint8_t reserved2;
};
static_assert(sizeof(ShortSpeakerDescriptor) == 3, "Bad size for ShortSpeakerDescriptor");

// Data block from CEA EDID timing extension's data block collection. Although this
// struct is 32 bytes long, only the first length+1 bytes are actually valid.
struct DataBlock {
  uint8_t header;
  DEF_SUBFIELD(header, 7, 5, type);
  DEF_SUBFIELD(header, 4, 0, length);

  union {
    ShortAudioDescriptor audio[10];
    // Only valid up to the index specified by length;
    ShortVideoDescriptor video[31];
    VendorSpecificBlock vendor;
    ShortSpeakerDescriptor speaker;
  } payload;
};
static_assert(sizeof(DataBlock) == 32, "Bad size for DataBlock");

// Returns an EISA vendor name based on its 2 bytes manufacturer name code.
// Returned string is a statically allocated constant. Returns NULL if no match is found.
const char* GetEisaVendorName(uint16_t manufacturer_name_code);

class timing_iterator;

class Edid {
 public:
  // Factory function preferred for production use.
  //
  // Creates and validates an Edid from raw bytes.
  //
  // On error, returns a `const char*` string of static storage duration
  // containing the error message.
  static fit::result<const char*, Edid> Create(std::span<const uint8_t> bytes);

  // Production code must use the factory method `Create()`.
  //
  // `bytes` must not be empty.
  // The size of `bytes` must be divisible by kBlockSize.
  // The size of `bytes` must not exceed kMaxEdidBlockCount * kBlockSize.
  explicit Edid(fbl::Vector<uint8_t> bytes);

  Edid(const Edid&) = delete;
  Edid& operator=(const Edid&) = delete;
  Edid(Edid&&) = default;
  Edid& operator=(Edid&&) = default;

  void Print(void (*print_fn)(const char* str)) const;

  const uint8_t* edid_bytes() const { return bytes_.data(); }
  size_t edid_length() const { return bytes_.size(); }

  uint16_t product_code() const { return base_edid().product_code; }
  bool is_standard_rgb() const { return base_edid().standard_srgb(); }
  bool supports_basic_audio() const;

  // Returns the display manufacturer's ISA / UEFI Plug and Play device
  // identifier (PNPID).
  std::string GetManufacturerId() const;

  // Returns an empty string ("") if the manufacturer is not found.
  //
  // Otherwise, returns the name of the display product manufacturer as
  // registered in the PNP ID Registry from the Unified Extensible Firmware
  // Interface (UEFI) Forum (https://uefi.org/PNP_ID_List).
  //
  // The returned string is guaranteed to have a static storage duration.
  const char* GetManufacturerName() const;

  // Returns the display product name stored in the Display Product Name String
  // Descriptor, defined in the E-EDID standard, Section 3.10.3.4 "Display
  // Product Name (ASCII) String Descriptor Definition (tag #FCh)", page 44.
  std::string GetDisplayProductName() const;

  // Returns the display product serial number stored in the Display Product
  // Serial Number Descriptor, defined in the E-EDID standard, Section 3.10.3.2
  // "Display Product Serial Number Descriptor Definition (tag #FFh)".
  std::string GetDisplayProductSerialNumber() const;

  // Returns all the display timing parameters supported by this display device.
  //
  // The first display timing is guaranteed to be preferred.
  zx::result<fbl::Vector<display::DisplayTiming>> GetSupportedDisplayTimings() const;

  // Guaranteed to be >= 0 and < 2^16.
  int horizontal_size_mm() const;

  // Guaranteed to be >= 0 and < 2^16.
  int vertical_size_mm() const;
  bool is_hdmi() const;

  const BaseEdid& base_edid() const { return *reinterpret_cast<const BaseEdid*>(bytes_.data()); }

 private:
  friend class internal::data_block_iterator;
  friend class internal::descriptor_iterator;
  friend class timing_iterator;

  template <typename T>
  const T* GetBlock(uint8_t block_num) const;

  fit::result<const char*> Validate();

  fbl::Vector<uint8_t> bytes_;
};

template <typename T>
const T* Edid::GetBlock(uint8_t block_num) const {
  const uint8_t* bytes = bytes_.data() + block_num * kBlockSize;
  return bytes[0] == T::kTag ? reinterpret_cast<const T*>(bytes) : nullptr;
}

// Iterator that returns all of the timing modes of the display. The iterator
// **does not** filter out duplicates.
class timing_iterator {
 public:
  explicit timing_iterator(const Edid* edid)
      : edid_(edid), state_(kDtds), state_index_(0), descriptors_(edid), dbs_(edid) {
    ++(*this);
  }

  timing_iterator& operator++();

  const display::DisplayTiming& operator*() const { return display_timing_; }
  const display::DisplayTiming* operator->() const { return &display_timing_; }

  bool is_valid() const { return state_ != kDone; }

 private:
  // The order in which timings are returned is:
  //   1) Detailed timings in order across base EDID and CEA blocks
  //   2) Short video descriptors in CEA data blocks
  //   3) Standard timings in base edid
  // TODO: Standart timings in descriptors
  // TODO: GTF/CVT timings in descriptors/monitor range limits
  // TODO: Established timings
  static constexpr uint8_t kDtds = 0;
  static constexpr uint8_t kSvds = 1;
  static constexpr uint8_t kStds = 2;
  static constexpr uint8_t kDone = 3;

  void Advance();

  display::DisplayTiming display_timing_;

  const Edid* edid_;
  uint8_t state_;
  uint16_t state_index_;
  internal::descriptor_iterator descriptors_;
  internal::data_block_iterator dbs_;
};

}  // namespace edid

#endif  // SRC_GRAPHICS_DISPLAY_LIB_EDID_EDID_H_
