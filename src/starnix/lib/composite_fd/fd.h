// Copyright 2026 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_LIB_COMPOSITE_FD_FD_H_
#define SRC_STARNIX_LIB_COMPOSITE_FD_FD_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

typedef struct fdio fdio_t;

// Create a composite file descriptor from and array of handles.
//
// On success, the handles are owned by the file descriptor.
//
// In Fuchsia, there is an expectation that there is a 1:1 mapping between a file descriptor and a
// handle. In general, we do not want to violate that rule. This library is intended to be used in
// very limited circumstances (compatibility with Linux and Binder), where we need to violate that
// rule.
zx_status_t composite_fd_create(zx_handle_t* handles, size_t size, fdio_t** out_fdio);

// Release the handles associated with a composite file descriptor.
void composite_fd_release(fdio_t* fdio, size_t size, zx_handle_t* out_handles);

// Return the number of handles associated with the composite file descriptor.
size_t composite_fd_size(fdio_t* fdio);

// Return whether the file descriptor is a composite file descriptor.
bool composite_fd_valid(fdio_t* fdio);

__END_CDECLS

#endif  // SRC_STARNIX_LIB_COMPOSITE_FD_FD_H_
