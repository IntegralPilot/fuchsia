// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
#ifndef ZIRCON_KERNEL_DEV_POWER_MOONFLOWER_INCLUDE_DEV_POWER_MOONFLOWER_INIT_H_
#define ZIRCON_KERNEL_DEV_POWER_MOONFLOWER_INCLUDE_DEV_POWER_MOONFLOWER_INIT_H_

#include <zircon/compiler.h>

__BEGIN_CDECLS

void moonflower_power_init_early();
void moonflower_power_init();

__END_CDECLS

#endif  // ZIRCON_KERNEL_DEV_POWER_MOONFLOWER_INCLUDE_DEV_POWER_MOONFLOWER_INIT_H_
