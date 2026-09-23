// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_THREADS_H_
#define ZIRCON_THREADS_H_

#include <threads.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// Get the zx_handle_t corresponding to the thrd_t. This handle is still owned
// by the thread, and will not persist after the thread exits.  Callers must
// duplicate the handle, therefore, if they wish the thread handle to outlive
// the execution of the thread.
zx_handle_t thrd_get_zx_handle(thrd_t t);

// This describes handles used by the current thread when it creates a new
// thread, whether via thrd_create or pthread_create or C++ std::thread, etc.
//
// The initial thread starts with some default handles.  They can be fetched by
// thrd_get_create_zx_handles() or replaced by thrd_set_create_zx_handles().  A
// new thread inherits the same handles used to create it (the ones last set on
// the _creating_ thread before it called thrd_create).  The zx_handle_t values
// are trivially copied--handles are not duplicated with zx_handle_duplicate().
//
// These handles are not "owned" by the thread, and will not be duplicated.
// The user is responsible for ensuring they remain valid as needed.
typedef struct {
  // This ZX_OBJ_TYPE_PROCESS handle is simply passed to zx_thread_create()
  // when creating new threads.  At startup the initial thread's value is what
  // zx_process_self() returns.  The handle is only used by thread creation
  // itself, so it does not have to remain valid when thrd_create et al are not
  // being called.
  //
  // The thrd_get_zx_process() and thrd_set_zx_process() calls are a second
  // interface to fetch or change this same handle without the others.
  zx_handle_t process;

  // These ZX_OBJ_TYPE_VMAR handles are used to map the memory needed to create
  // a new thread.  Each one is used in a zx_vmar_allocate() call to create a
  // small VMAR within.  It must be have rights allowing creation of a child
  // VMAR placed by the system and capable of mapping for read and write.  Each
  // child VMAR is created to contain a single mapping with (optional) guard
  // pages reserved around it; the new VMAR handles are then immediately
  // dropped so that the mappings cannot be changed and the guard regions
  // remain unmapped.
  //
  // The _parent_ VMAR handle values here are stored for the lifetime of the
  // new thread, separately from what thrd_set_zx_create_handles() changes in
  // either the new thread its creator thread.  However, the _handles_ are not
  // duplicated.  They **must** stay valid until the thread exits (and is
  // joined, if not detached); each VMAR handle will be used to unmap each
  // whole region (guards included) mapped in it when the thread was created.
  //
  // * The machine stack has a size determined by the thread attributes
  //   (rounded up to whole pages), and a guard size (zero or more pages)
  //   reserved as inaccessible below it to catch overflow of downward growth.
  //
  //  * The "security" stack has the same size as the machine stack.
  //    * On x86, the https://clang.llvm.org/docs/SafeStack.html "unsafe stack"
  //      is the "less secure" stack and the machine stack is more secure.  It
  //      grows down like the machine stack with the same guard size below.
  //    * Elsewhere, the https://clang.llvm.org/docs/ShadowCallStack.html is
  //      the "more secure" stack and the machine stack is the less secure.  It
  //      uses the same guard size but grows up with the guard above.
  //
  //  * The thread block has a size determined both by C library implementation
  //    details subject to change, and by thread_local space in the program.
  //    It's always surrounded both above and below by one-page guard regions.
  //
  // These can be different handles to map each into different VMARs, or all
  // the same VMAR or any combination.  At startup the initial thread's values
  // for all of these are just what zx_vmar_root_self() returns.
  zx_handle_t machine_stack_vmar;
  zx_handle_t security_stack_vmar;
  zx_handle_t thread_block_vmar;
} thrd_zx_create_handles_t;

// Sets the handles to use for subsequent thread creations by this thread, and
// returns the old values.  No handle ownership is implied, and these new
// handle values will not be consulted until the next thread creation.  But
// thereafter the VMAR handles must stay valid as described above.
thrd_zx_create_handles_t thrd_set_zx_create_handles(thrd_zx_create_handles_t handles);

// Returns the handles last set by thrd_set_zx_create_handles() or the initial
// values inherited from this thread's own creation.  This is just copying the
// handle values and no handle ownership transfer is implied.  The same handles
// will be used for the next thread creation.
thrd_zx_create_handles_t thrd_get_zx_create_handles(void);

// These are redundant with the `process` member of thrd_zx_create_handles_t.
zx_handle_t thrd_set_zx_process(zx_handle_t proc_handle);
zx_handle_t thrd_get_zx_process(void);

// Converts a threads.h-style status value to an |zx_status_t|.
static inline zx_status_t __PURE thrd_status_to_zx_status(int thrd_status) {
  switch (thrd_status) {
    case thrd_success:
      return ZX_OK;
    case thrd_nomem:
      return ZX_ERR_NO_MEMORY;
    case thrd_timedout:
      return ZX_ERR_TIMED_OUT;
    case thrd_busy:
      return ZX_ERR_SHOULD_WAIT;
    default:
    case thrd_error:
      return ZX_ERR_INTERNAL;
  }
}

__END_CDECLS

#ifdef __cplusplus

#if __has_include(<thread>)

#include <thread>

// Get the zx_handle_t corresponding to the std::thread::native_handle() value.
// See `thrd_get_zx_handle` (above) for constraints on the returned handle.
// Using this API avoids any assumptions about std::thread::native_handle_type
// corresponding exactly to thrd_t or any other particular type.
extern "C" zx_handle_t native_thread_get_zx_handle(std::thread::native_handle_type);

#endif  // __has_include(<thread>)

#endif  // __cplusplus

#endif  // ZIRCON_THREADS_H_
