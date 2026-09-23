// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_DEBUGLOG_INCLUDE_LIB_DEBUGLOG_TYPES_H_
#define ZIRCON_KERNEL_LIB_DEBUGLOG_INCLUDE_LIB_DEBUGLOG_TYPES_H_

#include <stdint.h>
#include <zircon/syscalls/log.h>
#include <zircon/types.h>

// This structure is designed to be copied into a zx_log_record_t from
// zircon/syscalls/log.h.
//
// The size, type, and offset of these fields must match those of
// zx_log_record_t.
typedef struct dlog_header {
  // When inside a debuglog, the |preamble| contains both the record's true size
  // (|DLOG_HDR_READLEN|) and the record's size when padded out to live in the
  // FIFO (|DLOG_HDR_FIFOLEN|).
  //
  // After being read out of a debuglog, the |preamble| field is 0.
  uint32_t preamble;
  uint16_t datalen;
  uint8_t severity;
  uint8_t flags;
  zx_instant_boot_t timestamp;
  uint64_t pid;
  uint64_t tid;
  // Each log record is assigned a sequence number at the time it enters the
  // debuglog. A record's sequence number will be exactly one greater than the
  // record that preceeded it. The purpose of |sequence| is to enable debuglog
  // readers to detect dropped message.
  uint64_t sequence;
} dlog_header_t;

constexpr size_t DLOG_MAX_RECORD = 256;
constexpr size_t DLOG_MAX_DATA = DLOG_MAX_RECORD - sizeof(dlog_header_t);

typedef struct dlog_record {
  dlog_header_t hdr;
  char data[DLOG_MAX_DATA];
} dlog_record_t;

#endif  // ZIRCON_KERNEL_LIB_DEBUGLOG_INCLUDE_LIB_DEBUGLOG_TYPES_H_
