// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library
//   zither.experimental.zx.types
//   (//zircon/tools/zither/testdata/experimental_zx_types/experimental_zx_types.test.fidl)
// by zither, a Fuchsia platform tool.

#ifndef EXPERIMENTAL_ZX_TYPES_C_EXPERIMENTAL_ZX_TYPES_H_
#define EXPERIMENTAL_ZX_TYPES_C_EXPERIMENTAL_ZX_TYPES_H_

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#if defined(__cplusplus)
extern "C" {
#endif

// 'a'
#define ZITHER_EXPERIMENTAL_ZX_TYPES_CHAR_CONST ((char)(97u))

#define ZITHER_EXPERIMENTAL_ZX_TYPES_SIZE_CONST ((size_t)(100u))

#define ZITHER_EXPERIMENTAL_ZX_TYPES_UINTPTR_CONST ((uintptr_t)(0x1234abcd5678ffffu))

typedef struct {
  char char_field;
  size_t size_field;
  uintptr_t uintptr_field;
} zither_experimental_zx_types_struct_with_primitives_t;

typedef uint8_t zither_experimental_zx_types_uint8_alias_t;

typedef struct {
  uint64_t* u64ptr;
  char* charptr;
  size_t* usizeptr;
  uint8_t* byteptr;
  void* voidptr;
  zither_experimental_zx_types_uint8_alias_t* aliasptr;
} zither_experimental_zx_types_struct_with_pointers_t;

typedef struct {
  char str[10];
  char strs[6][4];
} zither_experimental_zx_types_struct_with_string_arrays_t;

typedef struct {
  uint64_t value;
} zither_experimental_zx_types_overlay_struct_variant_t;

#define ZITHER_EXPERIMENTAL_ZX_TYPES_OVERLAY_WITH_EQUALLY_SIZED_VARIANTS_A ((uint64_t)(1u))
#define ZITHER_EXPERIMENTAL_ZX_TYPES_OVERLAY_WITH_EQUALLY_SIZED_VARIANTS_B ((uint64_t)(2u))
#define ZITHER_EXPERIMENTAL_ZX_TYPES_OVERLAY_WITH_EQUALLY_SIZED_VARIANTS_C ((uint64_t)(3u))
#define ZITHER_EXPERIMENTAL_ZX_TYPES_OVERLAY_WITH_EQUALLY_SIZED_VARIANTS_D ((uint64_t)(4u))

typedef struct {
  uint64_t discriminant;
  union {
    uint64_t a;
    int64_t b;
    zither_experimental_zx_types_overlay_struct_variant_t c;
    uint64_t d;
  };
} zither_experimental_zx_types_overlay_with_equally_sized_variants_t;

#define ZITHER_EXPERIMENTAL_ZX_TYPES_OVERLAY_WITH_DIFFERENTLY_SIZED_VARIANTS_A ((uint64_t)(1u))
#define ZITHER_EXPERIMENTAL_ZX_TYPES_OVERLAY_WITH_DIFFERENTLY_SIZED_VARIANTS_B ((uint64_t)(2u))
#define ZITHER_EXPERIMENTAL_ZX_TYPES_OVERLAY_WITH_DIFFERENTLY_SIZED_VARIANTS_C ((uint64_t)(3u))

typedef struct {
  uint64_t discriminant;
  union {
    zither_experimental_zx_types_overlay_struct_variant_t a;
    uint32_t b;
    bool c;
  };
} zither_experimental_zx_types_overlay_with_differently_sized_variants_t;

typedef struct {
  zither_experimental_zx_types_overlay_with_equally_sized_variants_t overlay1;
  zither_experimental_zx_types_overlay_with_differently_sized_variants_t overlay2;
} zither_experimental_zx_types_struct_with_overlay_members_t;

#if defined(__cplusplus)
}
#endif

#endif  // EXPERIMENTAL_ZX_TYPES_C_EXPERIMENTAL_ZX_TYPES_H_
