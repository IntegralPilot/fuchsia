// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_STARTUP_H_
#define ZIRCON_STARTUP_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// All Fuchsia ELF executables are position-independent (PIE) executables.
// Most use a dynamic linker via the PT_INTERP mechanism.  To use fully static
// linking (aside from the vDSO), each static PIE must perform its own startup
// dynamic linking, including vDSO references for making Zircon system calls.
//
// The standard entry point function (_start), used for both a static PIE and a
// dynamically-linked executable, calls the __libc_start_main function in the C
// library.  For the dynamic linking case, this is called after the dynamic
// linker has done its work and of necessity has already used a system
// bootstrap protocol on the handle transferred into the process by
// zx_process_start; the __libc_start_main function is found in the shared C
// library.  For a static PIE using the statically-linked C library instead,
// __libc_start_main does the dynamic linking work and then uses the APIs below
// to make use of the process start handle.  These can be overridden by a
// static PIE to support a custom bootstrap protocol instead of the standard
// one supported by the C library.  They cannot be overridden when using the
// shared C library: __libc_start_main will always use the standard protocols
// appropriate for the Fuchsia API level for which the C library was built.
//
// Either all of these three functions or none of them must be defined by a
// static PIE.  They work together.

// The first function is called with, and must use, only the basic machine ABI
// (no thread pointer, no shadow call stack).  Dynamic linking is complete and
// it can use system calls from the Zircon vDSO.
//
// **NOTE:** No C library functions can be used here, as they rely on the full
// Fuchsia Compiler ABI.  This includes even memcpy and memset calls that may
// be emitted by the compiler.  Entirely hermetic code that sticks to the
// basic machine ABI must be used to define this function.
//
// The argument is the initial handle transferred via zx_process_start.  This
// function takes ownership of that handle and the C library does not save it
// otherwise.  The return value provides the essential Zircon handles the C
// library needs to perform its basic work and complete startup.  An opaque
// void* value can be used to communicate any needed state to the second and
// third functions (declared below) without resorting to global variables.

typedef struct {
  // The process handle will be installed for zx_process_self() to return.
  // The C library is presumed to take ownership of this handle.  It never
  // closes the handle.  What zx_process_self() returns can never be changed.
  zx_handle_t process_self;

  // The thread handle will be used to manage the initial (calling) thread.
  // This is what zx_thread_self() or thrd_get_zx_handle() will return later.
  // The C library takes ownership of this handle and will close it if the
  // initial thread exits via thrd_exit() or pthread_exit().
  zx_handle_t thread_self;

  // The VMAR that libc can use for general allocation.
  // This is also installed for zx_vmar_root_self() to return.
  zx_handle_t allocation_vmar;

  // The innermost VMAR covering the load image of the executable itself.
  // This is used to apply RELRO protection after dynamic linking, and then
  // the handle is closed immediately.  It's usually the last handle to that
  // VMAR, such that revoking the protection becomes impossible thereafter.
  // If this is ZX_HANDLE_INVALID, no RELRO protection will be attempted.
  zx_handle_t executable_image_vmar;

  // May be ZX_HANDLE_INVALID or may be a handle for a ZX_OBJ_TYPE_DEBUGLOG or
  // ZX_OBJ_TYPE_SOCKET object to be used for implicit logging.  This will be
  // used for __sanitizer_log_write (see <zircon/sanitizer.h>), and also for
  // panic messages in early libc startup (if things go unrecoverably wrong
  // before constructors run).
  zx_handle_t log;

  // This is not examined by the C library, but is passed through to the
  // _zx_startup_get_arguments function.
  void* hook;
} zx_startup_handles_t;

zx_startup_handles_t _zx_startup_get_handles(zx_handle_t process_start_handle);

// This is called with the full Fuchsia Compiler ABI in place.  It runs before
// any C++ constructors.  Basic C library functions can be used normally, but
// anything using normal memory allocation (malloc et al, C++ non-placement
// new, etc.) should be avoided because the allocator is not initialized yet
// and its setup and may be influenced by the return value (e.g. environment
// variables).  The argument is the `hook` value _zx_startup_get_handles just
// returned.  The return value provides arguments for `main` and to make
// `getenv` work, etc.  Empty values will be replaced with valid pointers.
// Otherwise the invariant `argv[argc] == NULL` most hold.

typedef struct {
  int argc;
  char** argv;  // argc NUL-terminated strings, followed by nullptr.
  char** envp;  // NUL-terminated environment strings, followed by nullptr.
} zx_startup_arguments_t;

zx_startup_arguments_t _zx_startup_get_arguments(void* hook);

// This is the last thing called before C++ static constructors and the like
// will be called.  It can use all normal C library functionality, including
// allocation.  The argument is the same pointer value originally returned by
// _zx_startup_get_handles and then passed to _zx_startup_get_handles.  It
// should install any global state that should be in place before application
// or library code from constructors or `main` can run.  For example, it
// should call `_zx_utc_reference_swap` (<zircon/utc.h>).
void _zx_startup_preinit(void* hook);

__END_CDECLS

#endif  // ZIRCON_STARTUP_H_
