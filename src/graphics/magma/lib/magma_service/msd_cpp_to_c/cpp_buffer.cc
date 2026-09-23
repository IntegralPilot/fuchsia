// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "cpp_buffer.h"

#include <lib/magma/util/macros.h>

namespace msd {
CppBuffer::CppBuffer(struct MsdBuffer* buffer) : buffer_(buffer) { MAGMA_DASSERT(buffer_); }

CppBuffer::~CppBuffer() { msd_buffer_release(buffer_); }

}  // namespace msd
