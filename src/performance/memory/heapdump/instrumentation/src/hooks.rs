// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use heapdump_vmo::stack_trace_compression;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use zx_libc::sanitizer::fast_backtrace;

use crate::{PerThreadData, Profiler, with_profiler};

const STACK_TRACE_MAXIMUM_DEPTH: usize = 64;
const STACK_TRACE_MAXIMUM_COMPRESSED_SIZE: usize =
    stack_trace_compression::max_compressed_size(STACK_TRACE_MAXIMUM_DEPTH);

/// Makes the hooks return immediately without doing any operation.
///
/// It can only be set from false (initial value) to true, and it must be
/// considered just a hint.
///
/// Setting this flag is inherently racy, as some other thread might have
/// already checked the flag but not entered the main part of a hook. Therefore,
/// the Profiler needs to duplicate the same logic, but protected by Mutex.
static QUICK_EARLY_RETURN: AtomicBool = AtomicBool::new(false);

pub fn enable_quick_early_return() {
    QUICK_EARLY_RETURN.store(true, Ordering::Relaxed);
}

// Like `with_profiler`, but pass the current timestamp and the compressed call stack too.
//
// Note: This is function is `inline(always)` so that it doesn't appear in the stack trace.
#[inline(always)]
fn with_profiler_and_call_site(
    f: impl FnOnce(&Profiler, &mut PerThreadData, zx::MonotonicInstant, &[u8]),
) {
    // Collect the timestamp as early as possible.
    let timestamp = zx::MonotonicInstant::get();

    // Collect stack trace outside of the recursion guard to avoid including it in the stack trace.
    let mut stack_buf = [0; STACK_TRACE_MAXIMUM_DEPTH];
    let stack = fast_backtrace(&mut stack_buf);

    with_profiler(|profiler, thread_data| {
        // Compress the stack trace.
        let mut compressed_stack_buf = [0; STACK_TRACE_MAXIMUM_COMPRESSED_SIZE];
        let compressed_stack_len =
            stack_trace_compression::compress_into(stack, &mut compressed_stack_buf);
        let compressed_stack = &compressed_stack_buf[..compressed_stack_len];

        f(profiler, thread_data, timestamp, compressed_stack)
    })
}

// Called by Scudo after new memory has been allocated by malloc/calloc/...
#[unsafe(no_mangle)]
pub extern "C" fn __scudo_allocate_hook(ptr: *mut c_void, size: usize) {
    if QUICK_EARLY_RETURN.load(Ordering::Relaxed) {
        return;
    }

    with_profiler_and_call_site(|profiler, thread_data, timestamp, compressed_stack_trace| {
        profiler.record_allocation(
            thread_data,
            ptr as u64,
            size as u64,
            compressed_stack_trace,
            timestamp,
        );
    });
}

// Called by Scudo before memory is deallocated by free.
#[unsafe(no_mangle)]
pub extern "C" fn __scudo_deallocate_hook(ptr: *mut c_void) {
    if QUICK_EARLY_RETURN.load(Ordering::Relaxed) {
        return;
    }

    with_profiler(|profiler, thread_data| {
        if ptr != std::ptr::null_mut() {
            profiler.forget_allocation(thread_data, ptr as u64);
        }
    });
}

// Called by Scudo at the beginning of realloc.
#[unsafe(no_mangle)]
pub extern "C" fn __scudo_realloc_deallocate_hook(_old_ptr: *mut c_void) {
    // We don't do anything at this stage. All our work happens in __scudo_realloc_allocate_hook.
}

// Called by Scudo at the end of realloc.
#[unsafe(no_mangle)]
pub extern "C" fn __scudo_realloc_allocate_hook(
    old_ptr: *mut c_void,
    new_ptr: *mut c_void,
    size: usize,
) {
    if QUICK_EARLY_RETURN.load(Ordering::Relaxed) {
        return;
    }

    with_profiler_and_call_site(|profiler, thread_data, timestamp, compressed_stack_trace| {
        // Has the memory block been reallocated in-place?
        if old_ptr == new_ptr {
            profiler.update_allocation(
                thread_data,
                old_ptr as u64,
                size as u64,
                compressed_stack_trace,
                timestamp,
            );
        } else {
            profiler.record_allocation(
                thread_data,
                new_ptr as u64,
                size as u64,
                compressed_stack_trace,
                timestamp,
            );
            profiler.forget_allocation(thread_data, old_ptr as u64);
        }
    });
}
