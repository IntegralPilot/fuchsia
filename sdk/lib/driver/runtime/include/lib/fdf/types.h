// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FDF_TYPES_H_
#define LIB_FDF_TYPES_H_

#include <zircon/types.h>

__BEGIN_CDECLS

// fdf_handle_t is a zx_handle_t with the LSB zero.
typedef zx_handle_t fdf_handle_t;

#define FDF_HANDLE_INVALID ZX_HANDLE_INVALID
#define FDF_HANDLE_FIXED_BITS_MASK ZX_HANDLE_FIXED_BITS_MASK

typedef zx_txid_t fdf_txid_t;

// Defined in <lib/fdf/arena.h>
struct fdf_arena;

typedef struct fdf_channel_call_args {
  struct fdf_arena* wr_arena;
  void* wr_data;
  uint32_t wr_num_bytes;
  zx_handle_t* wr_handles;
  uint32_t wr_num_handles;
  struct fdf_arena** rd_arena;
  void** rd_data;
  uint32_t* rd_num_bytes;
  zx_handle_t** rd_handles;
  uint32_t* rd_num_handles;
} fdf_channel_call_args_t;

// Scheduler Role options

/// This flag will prevent any dispatchers from being created on the role that allow sync calls.
static const uint32_t FDF_SCHEDULER_ROLE_OPTION_NO_SYNC_CALLS = 1u << 0;

// Dispatcher creation options

/// This flag disallows parallel calls into callbacks set in the dispatcher.
static const uint32_t FDF_DISPATCHER_OPTION_SYNCHRONIZED = 0u << 0;
/// This flag allows parallel calls into callbacks set in the dispatcher.
/// Cannot be set in conjunction with `FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS`.
static const uint32_t FDF_DISPATCHER_OPTION_UNSYNCHRONIZED = 1u << 0;
/// This flag indicates that the dispatcher may not share zircon threads with other drivers.
/// Cannot be set in conjunction with `FDF_DISPATCHER_OPTION_UNSYNCHRONIZED`.
static const uint32_t FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS = 1u << 1;
/// This flag indicates that the dispatcher must not have its thread migrated at
/// runtime. It can only be used if the dispatcher's scheduler role has the
/// `FDF_SCHEDULER_ROLE_OPTION_NO_SYNC_CALLS` option set.
static const uint32_t FDF_DISPATCHER_OPTION_NO_THREAD_MIGRATION = 1u << 2;

static const uint32_t FDF_DISPATCHER_OPTION_SYNCHRONIZATION_MASK = 1u << 0;

/// This flag forces a channel wait to call its callback on cancellation,
/// even if the wait starts on a synchronized dispatcher. This allows
/// for safe cancellation of the wait from a different dispatcher than the one
/// it started on.
static const uint32_t FDF_CHANNEL_WAIT_OPTION_FORCE_ASYNC_CANCEL = 1u << 0;

/// This flag indicates that the channel wait was scheduled on an always-on
/// dispatcher.
static const uint32_t FDF_CHANNEL_WAIT_OPTION_ALWAYS_ON = 1u << 1;

__END_CDECLS

#endif  // LIB_FDF_TYPES_H_
