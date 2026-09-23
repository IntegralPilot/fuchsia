// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef REGION_ALLOC_REGION_H_
#define REGION_ALLOC_REGION_H_

#include <stdint.h>

struct ralloc_region_t {
  uint64_t base;
  uint64_t size;
};

#endif  // REGION_ALLOC_REGION_H_
