// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FDF_INTERNAL_H_
#define LIB_FDF_INTERNAL_H_

extern "C" {

// Blocks the current thread until each runtime dispatcher in the process
// is observed to enter an idle state. This does not guarantee that all the
// dispatchers will be idle when this function returns. This will only wait
// on dispatchers that existed when this function was called. This does not
// include any new dispatchers that might have been created while the waiting
// was happening.
// This does not wait for registered waits that have not yet been signaled,
// or delayed tasks which have been scheduled for a future deadline.
// This should not be called from a thread managed by the driver runtime,
// such as from tasks or ChannelRead callbacks.
void fdf_internal_wait_until_all_dispatchers_idle(void);

// Blocks the current thread until each runtime dispatcher in the process
// is observed to have been destroyed.
//
// # Thread requirements
//
// This should not be called from a thread managed by the driver runtime,
// such as from tasks or ChannelRead callbacks.
void fdf_internal_wait_until_all_dispatchers_destroyed(void);
}

#endif  // LIB_FDF_INTERNAL_H_
