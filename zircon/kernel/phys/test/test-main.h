// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_TEST_TEST_MAIN_H_
#define ZIRCON_KERNEL_PHYS_TEST_TEST_MAIN_H_

#include <lib/arch/ticks.h>

#include <ktl/variant.h>
#include <phys/main.h>
#include <phys/symbolize.h>

class AddressSpace;

// Just like InitMemory(), takes a pointer to the bootloader-provided data and
// additionally an early boot appropriate representation of the ZBI in the case
// of ZBI booting
int TestMain(void* bootloader_data, ktl::optional<EarlyBootZbi> zbi,
             arch::EarlyTicks) PHYS_SINGLETHREAD;

#endif  // ZIRCON_KERNEL_PHYS_TEST_TEST_MAIN_H_
