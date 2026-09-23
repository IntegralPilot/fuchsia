// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PLATFORM_PC_INCLUDE_PLATFORM_PC_HPET_H_
#define ZIRCON_KERNEL_PLATFORM_PC_INCLUDE_PLATFORM_PC_HPET_H_

#include <lib/affine/ratio.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

bool hpet_is_present();

uint64_t hpet_get_value();
zx_status_t hpet_set_value(uint64_t v);

void hpet_enable();
void hpet_disable();

void hpet_wait_ms(uint16_t ms);

uint64_t hpet_ticks_per_ms();

__END_CDECLS

#endif  // ZIRCON_KERNEL_PLATFORM_PC_INCLUDE_PLATFORM_PC_HPET_H_
