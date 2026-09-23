// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_IMAGE_FORMAT_IMAGE_FORMAT_H_
#define LIB_IMAGE_FORMAT_IMAGE_FORMAT_H_

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#if defined(FIDL_ALLOW_DEPRECATED_C_BINDINGS)
#include <fuchsia/sysmem/c/fidl.h>
#endif
#include <lib/fpromise/result.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <zircon/availability.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
#include <safemath/safe_math.h>
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

// Forward-declared; defined in <lib/zbi-format/graphics.h>.
using zbi_pixel_format_t = uint32_t;

// Iff this returns true, the two pixel formats are equal.
bool ImageFormatIsPixelFormatEqual(const PixelFormatAndModifier& a,
                                   const PixelFormatAndModifier& b);
bool ImageFormatIsPixelFormatEqual(const PixelFormatAndModifier& a,
                                   const PixelFormatAndModifier& b);
bool ImageFormatIsPixelFormatEqual(const fuchsia_sysmem::wire::PixelFormat& a,
                                   const fuchsia_sysmem::wire::PixelFormat& b);

// true - The color_space is potentially compatible with the PixelFormat,
// assuming the correct variant of the ColorSpace is used (with correct bpp).
//
// false - The pixel_format bpp is not supported by the given ColorSpace.  For
// example BT.2100 specifies 10 or 12 bpp, while NV12 specifies 8 bpp, so NV12
// is not compatible with BT.2100.  Or, the system does not support the
// combination of ColorSpace and PixelFormat (even if they are hypothetically
// compatible; in this case support might be added later).
//
// The first parameter can also accept fuchsia_images2::wire::ColorSpace since
// the type is the same.
bool ImageFormatIsSupportedColorSpaceForPixelFormat(const fuchsia_images2::ColorSpace& color_space,
                                                    const PixelFormatAndModifier& pixel_format);
bool ImageFormatIsSupportedColorSpaceForPixelFormat(
    const fuchsia_sysmem::wire::ColorSpace& color_space,
    const fuchsia_sysmem::wire::PixelFormat& pixel_format);

// If this returns true, the remainder of the functions in this header can be
// called with pixel_format.  If this returns false, calling any other method of
// this header file may abort() and/or return a meaningless value.
bool ImageFormatIsSupported(const PixelFormatAndModifier& pixel_format);
bool ImageFormatIsSupported(const fuchsia_sysmem::wire::PixelFormat& pixel_format);

// Returns the number of bits per pixel for the given PixelFormat.  This is the
// bits per pixel (RGB pixel or Y pixel) in the overall image across all bytes
// that contain pixel data.
//
// For example, NV12 is 12 bits per pixel.  This accounts for sub-sampling in
// both horizontal and vertical.
uint32_t ImageFormatBitsPerPixel(const PixelFormatAndModifier& pixel_format);
uint32_t ImageFormatBitsPerPixel(const fuchsia_sysmem::wire::PixelFormat& pixel_format);

// This is the number of stride bytes per pixel of width (RGB pixel width or Y
// pixel width) of plane 0.  For formats such as NV12 that separate Y and UV
// data, this is the number of stride bytes of the Y plane (plane 0).  NV12 has
// the same stride for the UV data.  This function doesn't return stride
// information for planes beyond plane 0.
uint32_t ImageFormatStrideBytesPerWidthPixel(const PixelFormatAndModifier& pixel_format);
uint32_t ImageFormatStrideBytesPerWidthPixel(const fuchsia_sysmem::wire::PixelFormat& pixel_format);

// This would be height * stride, if it weren't for formats like NV12, where it
// isn't.  The return value is in bytes.
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
safemath::CheckedNumeric<uint64_t> ImageFormatImageSizeChecked(
    const fuchsia_images2::ImageFormat& image_format);
safemath::CheckedNumeric<uint64_t> ImageFormatImageSizeChecked(
    const fuchsia_images2::wire::ImageFormat& image_format);
safemath::CheckedNumeric<uint64_t> ImageFormatImageSizeChecked(
    const fuchsia_sysmem::wire::ImageFormat2& image_format);
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)
uint64_t ImageFormatImageSize(const fuchsia_images2::ImageFormat& image_format);
uint64_t ImageFormatImageSize(const fuchsia_images2::wire::ImageFormat& image_format);
uint64_t ImageFormatImageSize(const fuchsia_sysmem::wire::ImageFormat2& image_format);

uint32_t ImageFormatSurfaceWidthMinDivisor(const PixelFormatAndModifier& pixel_format);
uint32_t ImageFormatCodedWidthMinDivisor(const fuchsia_sysmem::wire::PixelFormat& pixel_format);

uint32_t ImageFormatSurfaceHeightMinDivisor(const PixelFormatAndModifier& pixel_format);
uint32_t ImageFormatCodedHeightMinDivisor(const fuchsia_sysmem::wire::PixelFormat& pixel_format);

// The start of image data must be at least this aligned.
//
// The plane 0 stride is required to be divisible by this alignment.  Generally
// the stride of planes beyond plane 0 (if any) will have a known fixed
// relationship with the plane 0 stride.
uint32_t ImageFormatSampleAlignment(const PixelFormatAndModifier& pixel_format);
uint32_t ImageFormatSampleAlignment(const fuchsia_sysmem::wire::PixelFormat& pixel_format);

// Gets the minimum number of bytes per row possible for an image with a
// specific width and specific constraints. Returns false if the width would not
// be valid.
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
safemath::CheckedNumeric<uint32_t> ImageFormatMinimumRowBytesChecked(
    const fuchsia_sysmem2::ImageFormatConstraints& constraints,
    safemath::CheckedNumeric<uint32_t> width);
safemath::CheckedNumeric<uint32_t> ImageFormatMinimumRowBytesChecked(
    const fuchsia_sysmem2::wire::ImageFormatConstraints& constraints,
    safemath::CheckedNumeric<uint32_t> width);
safemath::CheckedNumeric<uint32_t> ImageFormatMinimumRowBytesChecked(
    const fuchsia_sysmem::wire::ImageFormatConstraints& constraints,
    safemath::CheckedNumeric<uint32_t> width);
#endif
// Non-checked is deprecated; switch to ImageFormatMinimumRowBytesChecked above.
bool ImageFormatMinimumRowBytes(const fuchsia_sysmem2::ImageFormatConstraints& constraints,
                                uint32_t width, uint32_t* minimum_row_bytes_out);
bool ImageFormatMinimumRowBytes(const fuchsia_sysmem2::wire::ImageFormatConstraints& constraints,
                                uint32_t width, uint32_t* minimum_row_bytes_out);
bool ImageFormatMinimumRowBytes(const fuchsia_sysmem::wire::ImageFormatConstraints& constraints,
                                uint32_t width, uint32_t* minimum_row_bytes_out);

fpromise::result<fuchsia_images2::wire::PixelFormat> ImageFormatConvertZbiToSysmemPixelFormat_v2(
    zbi_pixel_format_t zbi_pixel_format);

// Though these are used within the .cc file, they do not need to be exposed for stable API levels.
fpromise::result<fuchsia_images2::ImageFormat> ImageConstraintsToFormat(
    const fuchsia_sysmem2::ImageFormatConstraints& constraints, uint32_t width, uint32_t height);
fpromise::result<fuchsia_images2::wire::ImageFormat> ImageConstraintsToFormat(
    fidl::AnyArena& allocator, const fuchsia_sysmem2::wire::ImageFormatConstraints& constraints,
    uint32_t width, uint32_t height);
fpromise::result<fuchsia_sysmem::wire::ImageFormat2> ImageConstraintsToFormat(
    const fuchsia_sysmem::wire::ImageFormatConstraints& constraints, uint32_t width,
    uint32_t height);

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
safemath::CheckedNumeric<uint64_t> ImageFormatPlaneByteOffsetChecked(
    const fuchsia_images2::ImageFormat& image_format, uint32_t plane);
safemath::CheckedNumeric<uint64_t> ImageFormatPlaneByteOffsetChecked(
    const fuchsia_images2::wire::ImageFormat& image_format, uint32_t plane);
safemath::CheckedNumeric<uint64_t> ImageFormatPlaneByteOffsetChecked(
    const fuchsia_sysmem::wire::ImageFormat2& image_format, uint32_t plane);
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)
bool ImageFormatPlaneByteOffset(const fuchsia_images2::ImageFormat& image_format, uint32_t plane,
                                uint64_t* offset_out);
bool ImageFormatPlaneByteOffset(const fuchsia_images2::wire::ImageFormat& image_format,
                                uint32_t plane, uint64_t* offset_out);
bool ImageFormatPlaneByteOffset(const fuchsia_sysmem::wire::ImageFormat2& image_format,
                                uint32_t plane, uint64_t* offset_out);

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
safemath::CheckedNumeric<uint32_t> ImageFormatPlaneRowBytesChecked(
    const fuchsia_images2::ImageFormat& image_format, uint32_t plane);
safemath::CheckedNumeric<uint32_t> ImageFormatPlaneRowBytesChecked(
    const fuchsia_images2::wire::ImageFormat& image_format, uint32_t plane);
safemath::CheckedNumeric<uint32_t> ImageFormatPlaneRowBytesChecked(
    const fuchsia_sysmem::wire::ImageFormat2& image_format, uint32_t plane);
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)
bool ImageFormatPlaneRowBytes(const fuchsia_images2::ImageFormat& image_format, uint32_t plane,
                              uint32_t* row_bytes_out);
bool ImageFormatPlaneRowBytes(const fuchsia_images2::wire::ImageFormat& image_format,
                              uint32_t plane, uint32_t* row_bytes_out);
bool ImageFormatPlaneRowBytes(const fuchsia_sysmem::wire::ImageFormat2& image_format,
                              uint32_t plane, uint32_t* row_bytes_out);

bool ImageFormatCompatibleWithProtectedMemory(const PixelFormatAndModifier& pixel_format);
bool ImageFormatCompatibleWithProtectedMemory(
    const fuchsia_sysmem::wire::PixelFormat& pixel_format);

#if FUCHSIA_API_LEVEL_AT_LEAST(30)
// This intentionally does not define how planes are indexed or how they're counted, only whether a
// format is both non-tiled and has a single plane. Data like TE or CCS or the like imply not being
// single-plane, as do multi-plane YCbCr formats. MJPEG doesn't count as non-tiled and single-plane.
bool ImageFormatIsNonTiledSinglePlane(const PixelFormatAndModifier& pixel_format_and_modifier);
#endif

#endif  // LIB_IMAGE_FORMAT_IMAGE_FORMAT_H_
