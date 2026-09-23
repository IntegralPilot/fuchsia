// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZXIO_FAULT_CATCHER_H_
#define LIB_ZXIO_FAULT_CATCHER_H_

#include <lib/zxio/types.h>
#include <zircon/compiler.h>

__BEGIN_CDECLS

// Attempts to copy up to |count| bytes from |src| to |dest|.
//
// This method will always return true as it never handles faults. If a fault
// occurs during the copy, this method will trigger a Zircon exception.
//
// Provides the default (weak) definition of zxio_maybe_faultable_copy.
__EXPORT bool zxio_default_maybe_faultable_copy(unsigned char* dest, const unsigned char* src,
                                                size_t count, bool ret_dest);

// Attempts to copy up to |count| bytes from |src| to |dest|.
//
// Only at most one of |src| or |dest| may point to user-memory (provided by a
// consumder of this library); the other (or both) must be memory that zxio
// owns and is guaranteed not to fault.
//
// Returns true if all the bytes were successfully copied, or false if a fault
// occurred.
//
// This library weakly defines this symbol so that it may be overridden by
// downstream libraries/applications. The default weak symbol is defined with an
// alias to |zxio_default_maybe_faultable_copy|.
//
// __NO_INLINE because this method may be overridden and we want to make sure
// callers get the overridden method.
__EXPORT __NO_INLINE bool zxio_maybe_faultable_copy(unsigned char* dest, const unsigned char* src,
                                                    size_t count, bool ret_dest);

// Returns true if fault catching is disabled.
//
// This method MUST return false if downstream libraries/applications expect
// to pass memory addresses to zxio that may fault on access. Note that a copy
// method that is able to catch faults must also be provided. For more details,
// see |zxio_maybe_faultable_copy|.
//
// This library weakly defines this symbol so that it may be overridden by
// downstream libraries/applications. The default weak symbol is defined by a
// method that always returns true.
//
// __NO_INLINE because this method may be overridden and we want to make sure
// callers get the overridden method.
__EXPORT __NO_INLINE bool zxio_fault_catching_disabled();

__END_CDECLS

#endif  // LIB_ZXIO_FAULT_CATCHER_H_
