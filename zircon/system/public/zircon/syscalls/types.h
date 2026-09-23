// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSCALLS_TYPES_H_
#define ZIRCON_SYSCALLS_TYPES_H_

#include <zircon/compiler.h>

#include "zircon/syscalls/exception.h"

__BEGIN_CDECLS

// forward declarations needed by syscalls.h
typedef struct zx_exception_context zx_exception_context_t;
typedef struct zx_port_packet zx_port_packet_t;
typedef struct zx_system_powerctl_arg zx_system_powerctl_arg_t;
typedef struct zx_profile_info zx_profile_info_t;
typedef struct zx_smc_parameters zx_smc_parameters_t;
typedef struct zx_smc_result zx_smc_result_t;
typedef struct zx_sampler_config zx_sampler_config_t;

__END_CDECLS

#endif  // ZIRCON_SYSCALLS_TYPES_H_
