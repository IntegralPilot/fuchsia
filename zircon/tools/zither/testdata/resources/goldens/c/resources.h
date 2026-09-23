// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library
//   zither.resources (//zircon/tools/zither/testdata/resources/resources.test.fidl)
// by zither, a Fuchsia platform tool.

#ifndef RESOURCES_C_RESOURCES_H_
#define RESOURCES_C_RESOURCES_H_

#include <stdint.h>

#if defined(__cplusplus)
extern "C" {
#endif

typedef uint32_t zither_resources_subtype_t;

#define ZITHER_RESOURCES_SUBTYPE_A ((zither_resources_subtype_t)(0u))
#define ZITHER_RESOURCES_SUBTYPE_B ((zither_resources_subtype_t)(1u))

// This is a handle.
typedef uint32_t zither_resources_handle_t;

typedef struct {
  zither_resources_handle_t untyped_handle;
  zither_resources_handle_t handle_a;
  zither_resources_handle_t handle_b;
} zither_resources_struct_with_handle_members_t;

#if defined(__cplusplus)
}
#endif

#endif  // RESOURCES_C_RESOURCES_H_
