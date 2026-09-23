// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PLATFORM_PC_INCLUDE_PLATFORM_PC_KEYBOARD_H_
#define ZIRCON_KERNEL_PLATFORM_PC_INCLUDE_PLATFORM_PC_KEYBOARD_H_

#include <zircon/compiler.h>

__BEGIN_CDECLS

// Reboot the system via the keyboard, returns on failure.
void pc_keyboard_reboot();

__END_CDECLS

#endif  // ZIRCON_KERNEL_PLATFORM_PC_INCLUDE_PLATFORM_PC_KEYBOARD_H_
