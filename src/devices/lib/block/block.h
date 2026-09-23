// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_BLOCK_BLOCK_H_
#define SRC_DEVICES_LIB_BLOCK_BLOCK_H_

#include "src/storage/lib/block_protocol/block-fifo.h"

static_assert(sizeof(BlockFifoRequest) == sizeof(BlockFifoResponse),
              "FIFO messages are the same size in both directions");

#define BLOCK_FIFO_ESIZE (sizeof(BlockFifoRequest))
#define BLOCK_FIFO_MAX_DEPTH (4096 / BLOCK_FIFO_ESIZE)

#endif  // SRC_DEVICES_LIB_BLOCK_BLOCK_H_
