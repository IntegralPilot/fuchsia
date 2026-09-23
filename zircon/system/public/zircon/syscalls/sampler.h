// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSCALLS_SAMPLER_H_
#define ZIRCON_SYSCALLS_SAMPLER_H_

#include <stddef.h>
#include <zircon/compiler.h>
#include <zircon/time.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// The act of taking a sample takes on the order of single digit microseconds.
// A period close to or shorter than that doesn't make sense.
#define ZX_SAMPLER_MIN_PERIOD ZX_USEC(10)

#define ZX_SAMPLER_MAX_BUFFER_SIZE (size_t)(1024 * 1024 * 1024) /*1 GiB*/

// Configuration struct for periodically sampling a thread
typedef struct zx_sampler_config {
  zx_duration_mono_t period;
  size_t buffer_size;
} zx_sampler_config_t;

__END_CDECLS

#endif  // ZIRCON_SYSCALLS_SAMPLER_H_
