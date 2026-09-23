// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_MINFS_STORAGE_BITMAP_H_
#define SRC_STORAGE_MINFS_STORAGE_BITMAP_H_

#include <bitmap/raw-bitmap.h>

#ifdef __Fuchsia__
#include "src/storage/lib/buffer/vmo_bitmap_storage.h"
#else
#include <bitmap/storage.h>
#endif

namespace minfs {

#ifdef __Fuchsia__
using StorageBitmap = bitmap::RawBitmapGeneric<storage::VmoBitmapStorage>;
#else
using StorageBitmap = bitmap::RawBitmapGeneric<bitmap::DefaultStorage>;
#endif

}  // namespace minfs

#endif  // SRC_STORAGE_MINFS_STORAGE_BITMAP_H_
