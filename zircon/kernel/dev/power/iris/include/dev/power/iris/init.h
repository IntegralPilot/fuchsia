// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_POWER_IRIS_INCLUDE_DEV_POWER_IRIS_INIT_H_
#define ZIRCON_KERNEL_DEV_POWER_IRIS_INCLUDE_DEV_POWER_IRIS_INIT_H_

#include <lib/zbi-format/driver-config.h>
#include <zircon/compiler.h>

__BEGIN_CDECLS

void iris_power_init_early();
void iris_power_init(const zbi_cpu_energy_model_domain_t* domains, size_t domain_count);
uintptr_t cpp_iris_get_opp_vaddr();

__END_CDECLS

#endif  // ZIRCON_KERNEL_DEV_POWER_IRIS_INCLUDE_DEV_POWER_IRIS_INIT_H_
