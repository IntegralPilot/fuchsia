// Copyright 2017 The Chromium Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_MEDIA_BASE_VIDEO_COLOR_SPACE_H_
#define SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_MEDIA_BASE_VIDEO_COLOR_SPACE_H_

// Fuchsia change: Remove libraries in favor of "chromium_utils.h"
#include "chromium_utils.h"

namespace gfx {
class ColorSpace {
 public:
  enum class PrimaryID : uint8_t {
    INVALID,
    BT709,
    BT470M,
    BT470BG,
    SMPTE170M,
    SMPTE240M,
    FILM,
    BT2020,
    SMPTEST428_1,
    SMPTEST431_2,
    P3,
    SMPTEST432_1 = P3,
    EBU_3213_E,
    kMaxValue = EBU_3213_E,
  };

  enum class TransferID : uint8_t {
    INVALID,
    BT709,
    BT709_APPLE,
    GAMMA22,
    GAMMA28,
    SMPTE170M,
    SMPTE240M,
    LINEAR,
    LOG,
    LOG_SQRT,
    IEC61966_2_4,
    BT1361_ECG,
    SRGB,
    BT2020_10,
    BT2020_12,
    PQ,
    SMPTEST428_1,
    HLG,
    kMaxValue = HLG,
  };

  enum class MatrixID : uint8_t {
    INVALID,
    RGB,
    BT709,
    FCC,
    BT470BG,
    SMPTE170M,
    SMPTE240M,
    YCOCG,
    BT2020_NCL,
    YDZDX,
    GBR,
    kMaxValue = GBR,
  };

  enum class RangeID : uint8_t {
    INVALID,
    LIMITED,
    FULL,
    DERIVED,
    kMaxValue = DERIVED,
  };

  ColorSpace() = default;
  ColorSpace(PrimaryID primary_id,
             TransferID transfer_id,
             MatrixID matrix_id,
             RangeID range_id)
      : primary_id_(primary_id),
        transfer_id_(transfer_id),
        matrix_id_(matrix_id),
        range_id_(range_id) {}

  PrimaryID GetPrimaryID() const { return primary_id_; }
  TransferID GetTransferID() const { return transfer_id_; }
  MatrixID GetMatrixID() const { return matrix_id_; }
  RangeID GetRangeID() const { return range_id_; }

  bool IsValid() const {
    return primary_id_ != PrimaryID::INVALID &&
           transfer_id_ != TransferID::INVALID &&
           matrix_id_ != MatrixID::INVALID && range_id_ != RangeID::INVALID;
  }

  bool IsHDR() const {
    return transfer_id_ == TransferID::PQ || transfer_id_ == TransferID::HLG;
  }

  bool operator==(const ColorSpace& other) const {
    return primary_id_ == other.primary_id_ &&
           transfer_id_ == other.transfer_id_ &&
           matrix_id_ == other.matrix_id_ && range_id_ == other.range_id_;
  }

  bool operator!=(const ColorSpace& other) const { return !(*this == other); }

 private:
  PrimaryID primary_id_ = PrimaryID::INVALID;
  TransferID transfer_id_ = TransferID::INVALID;
  MatrixID matrix_id_ = MatrixID::INVALID;
  RangeID range_id_ = RangeID::INVALID;
};
}  // namespace gfx

namespace media {

// Described in ISO 23001-8:2016
class MEDIA_EXPORT VideoColorSpace {
 public:
  // Table 2
  //
  // TODO(https://crbug.com/380457000): Delete this enum and use
  // `SkNamedPrimaries::CicpId` instead.
  enum class PrimaryID : uint8_t {
    INVALID = 0,
    BT709 = 1,
    UNSPECIFIED = 2,
    BT470M = 4,
    BT470BG = 5,
    SMPTE170M = 6,
    SMPTE240M = 7,
    FILM = 8,
    BT2020 = 9,
    SMPTEST428_1 = 10,
    SMPTEST431_2 = 11,
    SMPTEST432_1 = 12,
    EBU_3213_E = 22,
    kMaxValue = EBU_3213_E,
  };

  // Table 3
  //
  // TODO(https://crbug.com/380457000): Delete this enum and use
  // `SkNamedTransferFn::CicpId` instead.
  enum class TransferID : uint8_t {
    INVALID = 0,
    BT709 = 1,
    UNSPECIFIED = 2,
    GAMMA22 = 4,
    GAMMA28 = 5,
    SMPTE170M = 6,
    SMPTE240M = 7,
    LINEAR = 8,
    LOG = 9,
    LOG_SQRT = 10,
    IEC61966_2_4 = 11,
    BT1361_ECG = 12,
    IEC61966_2_1 = 13,
    BT2020_10 = 14,
    BT2020_12 = 15,
    SMPTEST2084 = 16,
    SMPTEST428_1 = 17,

    // Not yet standardized
    ARIB_STD_B67 = 18,  // AKA hybrid-log gamma, HLG.

    kMaxValue = ARIB_STD_B67,
  };

  // Table 4
  enum class MatrixID : uint8_t {
    RGB = 0,
    BT709 = 1,
    UNSPECIFIED = 2,
    FCC = 4,
    BT470BG = 5,
    SMPTE170M = 6,
    SMPTE240M = 7,
    YCOCG = 8,
    BT2020_NCL = 9,
    // NOTE: BT2020_CL is no longer supported (b/333906350).
    BT2020_CL = 10,
    YDZDX = 11,
    INVALID = 255,
    kMaxValue = INVALID,
  };

  VideoColorSpace();
  VideoColorSpace(int primaries,
                  int transfer,
                  int matrix,
                  gfx::ColorSpace::RangeID range);
  VideoColorSpace(PrimaryID primaries,
                  TransferID transfer,
                  MatrixID matrix,
                  gfx::ColorSpace::RangeID range);
  bool operator==(const VideoColorSpace& other) const;
  bool operator!=(const VideoColorSpace& other) const;

  // Returns true if all of the fields have a value other
  // than INVALID or UNSPECIFIED.
  bool IsSpecified() const;

  // Returns true if the transfer function is HDR (PQ or HLG).
  bool IsHDR() const;

  // These will return INVALID if the number you give it
  // is not a valid enum value.
  static PrimaryID GetPrimaryID(int primary);
  static TransferID GetTransferID(int transfer);
  static MatrixID GetMatrixID(int matrix);

  static VideoColorSpace REC709();
  static VideoColorSpace REC601();
  static VideoColorSpace JPEG();

  gfx::ColorSpace ToGfxColorSpace() const;

  // Similar to ToGfxColorSpace(), but attempts to guess a gfx::ColorSpace from
  // a fully or partially specified VideoColorSpace. E.g., a completely invalid
  // VideoColorSpace will return a BT.709 gfx::ColorSpace.
  gfx::ColorSpace GuessGfxColorSpace() const;

  std::string ToString() const;

  static VideoColorSpace FromGfxColorSpace(const gfx::ColorSpace& color_space);

  PrimaryID primaries() const;
  TransferID transfer() const;
  MatrixID matrix() const;
  gfx::ColorSpace::RangeID range() const { return color_space_.GetRangeID(); }

 private:
  VideoColorSpace(gfx::ColorSpace color_space,
                  bool primaries_unspecified,
                  bool transfer_unspecified,
                  bool matrix_unspecified);

  gfx::ColorSpace color_space_;
  bool primaries_unspecified_ = true;
  bool transfer_unspecified_ = true;
  bool matrix_unspecified_ = true;
};

}  // namespace media

#endif  // SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_MEDIA_BASE_VIDEO_COLOR_SPACE_H_
