// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "abr.h"

#include <lib/cksum.h>
uint32_t AbrCrc32(const void* buf, size_t buf_size) { return crc32(0, buf, buf_size); }
