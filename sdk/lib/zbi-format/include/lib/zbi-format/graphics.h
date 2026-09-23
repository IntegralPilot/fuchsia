// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library
//   zbi (//sdk/fidl/zbi/graphics.fidl)
// by zither, a Fuchsia platform tool.

#ifndef LIB_ZBI_FORMAT_GRAPHICS_H_
#define LIB_ZBI_FORMAT_GRAPHICS_H_

#include <stdint.h>

#if defined(__cplusplus)
extern "C" {
#endif

// Gives a pixel format representation.
//
// Bits [23:16] (i.e., the third byte) encode the number of bytes per pixel
// in the representation.
typedef uint32_t zbi_pixel_format_t;

#define ZBI_PIXEL_FORMAT_NONE ((zbi_pixel_format_t)(0x00000000u))
#define ZBI_PIXEL_FORMAT_RGB_565 ((zbi_pixel_format_t)(0x00020001u))
#define ZBI_PIXEL_FORMAT_RGB_332 ((zbi_pixel_format_t)(0x00010002u))
#define ZBI_PIXEL_FORMAT_RGB_2220 ((zbi_pixel_format_t)(0x00010003u))
#define ZBI_PIXEL_FORMAT_ARGB_8888 ((zbi_pixel_format_t)(0x00040004u))
#define ZBI_PIXEL_FORMAT_RGB_X888 ((zbi_pixel_format_t)(0x00040005u))
#define ZBI_PIXEL_FORMAT_MONO_8 ((zbi_pixel_format_t)(0x00010007u))
#define ZBI_PIXEL_FORMAT_NV12 ((zbi_pixel_format_t)(0x00010008u))
#define ZBI_PIXEL_FORMAT_I420 ((zbi_pixel_format_t)(0x00010009u))
#define ZBI_PIXEL_FORMAT_RGB_888 ((zbi_pixel_format_t)(0x00030009u))
#define ZBI_PIXEL_FORMAT_ABGR_8888 ((zbi_pixel_format_t)(0x0004000au))
#define ZBI_PIXEL_FORMAT_BGR_888_X ((zbi_pixel_format_t)(0x0004000bu))
#define ZBI_PIXEL_FORMAT_ARGB_2_10_10_10 ((zbi_pixel_format_t)(0x0004000cu))
#define ZBI_PIXEL_FORMAT_ABGR_2_10_10_10 ((zbi_pixel_format_t)(0x0004000du))

// ZBI_TYPE_FRAMEBUFFER payload.
typedef struct {
  // Physical memory address.
  uint64_t base;

  // Pixel layout and format.
  uint32_t width;
  uint32_t height;
  uint32_t stride;
  zbi_pixel_format_t format;
} zbi_swfb_t;

#if defined(__cplusplus)
}
#endif

#endif  // LIB_ZBI_FORMAT_GRAPHICS_H_
