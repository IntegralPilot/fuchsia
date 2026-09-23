// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_GIGABOOT_SRC_ABR_H_
#define SRC_FIRMWARE_GIGABOOT_SRC_ABR_H_

#include <stddef.h>
#include <stdint.h>

uint32_t AbrCrc32(const void* buf, size_t buf_size);

#endif  // SRC_FIRMWARE_GIGABOOT_SRC_ABR_H_
