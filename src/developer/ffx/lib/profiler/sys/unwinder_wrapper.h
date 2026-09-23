// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FFX_LIB_PROFILER_SYS_UNWINDER_WRAPPER_H_
#define SRC_DEVELOPER_FFX_LIB_PROFILER_SYS_UNWINDER_WRAPPER_H_

#include <stddef.h>
#include <stdint.h>

extern "C" {

// LINT.IfChange
typedef struct ffi_unwinder_t ffi_unwinder_t;
// LINT.ThenChange(src/developer/ffx/lib/profiler/src/unwinder.rs)

ffi_unwinder_t* ffi_unwinder_new();
void ffi_unwinder_free(ffi_unwinder_t* unwinder);

// Add a memory chunk to the unwinder's memory view. These are used when Unwind is called.
void ffi_unwinder_add_memory(ffi_unwinder_t* unwinder, uint64_t base, const uint8_t* data,
                             size_t size);

// Clear the memory chunks added via ffi_unwinder_add_memory.
void ffi_unwinder_clear_memory(ffi_unwinder_t* unwinder);

// Add an ELF module to the unwinder. The path should point to the unstripped/debug ELF file on the
// host.
void ffi_unwinder_add_module(ffi_unwinder_t* unwinder, uint64_t load_address, const char* file_path,
                             size_t file_path_len);

// LINT.IfChange
typedef struct {
  uint64_t pc;
  uint64_t sp;
} ffi_frame_t;
// LINT.ThenChange(src/developer/ffx/lib/profiler/src/unwinder.rs)

// Unwinds the stack and populates output_frames with up to max_depth frames. Returns the number of
// frames populated.
size_t ffi_unwinder_unwind(ffi_unwinder_t* unwinder, const uint8_t* regs_data, size_t regs_size,
                           ffi_frame_t* output_frames, size_t max_depth);
}

#endif  // SRC_DEVELOPER_FFX_LIB_PROFILER_SYS_UNWINDER_WRAPPER_H_
