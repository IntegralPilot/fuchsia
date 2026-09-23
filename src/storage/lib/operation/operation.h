// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_OPERATION_OPERATION_H_
#define SRC_STORAGE_LIB_OPERATION_OPERATION_H_

#include <lib/stdcompat/span.h>

#include <cstdint>
#include <ostream>

namespace storage {

using vmoid_t = uint16_t;

enum class OperationType {
  kRead,
  kWrite,
  kTrim,
  kWriteFua,
  kMaxValue = kWriteFua,  // For FuzzedDataProvider
};

const char* OperationTypeToString(OperationType type);

using TraceFlowId = uint64_t;

// A mapping of an in-memory buffer to an on-disk location.
//
// All units are in filesystem-size blocks.
struct Operation {
  OperationType type;
  uint64_t vmo_offset = 0;
  uint64_t dev_offset = 0;
  uint64_t length = 0;
  TraceFlowId trace_flow_id = 0;
};

// An operation paired with a source vmoid or a pointer to the data for host side code.
//
// This vmoid is a token that represents a buffer that is attached to the
// underlying storage device.
struct BufferedOperation {
#ifdef __Fuchsia__
  vmoid_t vmoid;
#else
  void* data;
#endif
  Operation op;
};

std::ostream& operator<<(std::ostream&, const BufferedOperation&);
std::ostream& operator<<(std::ostream&, const cpp20::span<const BufferedOperation>&);

// Sums the |length| of all requests. It will assert if overflow occurs; the caller is responsible
// for making sure this does not happen.
uint64_t BlockCount(cpp20::span<const BufferedOperation> operations);

}  // namespace storage

#endif  // SRC_STORAGE_LIB_OPERATION_OPERATION_H_
