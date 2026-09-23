// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_BUFFER_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_BUFFER_H_

#include <lib/magma_service/msd.h>
#include <lib/magma_service/msd_c.h>

namespace msd {
class CppBuffer : public msd::Buffer {
 public:
  explicit CppBuffer(struct MsdBuffer* buffer);

  ~CppBuffer() override;

  MsdBuffer* buffer() { return buffer_; }

 private:
  struct MsdBuffer* buffer_;

  CppBuffer(const CppBuffer&) = delete;
  CppBuffer& operator=(const CppBuffer&) = delete;
};
}  // namespace msd

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_BUFFER_H_
