// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef VIRTIO_CONSOLE_H_
#define VIRTIO_CONSOLE_H_

#include <stdint.h>
#include <zircon/compiler.h>

// clang-format off

#define VIRTIO_CONSOLE_F_SIZE               ((uint64_t)1 << 0)
#define VIRTIO_CONSOLE_F_MULTIPORT          ((uint64_t)1 << 1)
#define VIRTIO_CONSOLE_F_EMERG_WRITE        ((uint64_t)1 << 2)

// clang-format on

__BEGIN_CDECLS

typedef struct virtio_console_config {
  uint16_t cols;
  uint16_t rows;
  uint32_t max_nr_ports;
  uint32_t emerg_wr;
} __PACKED virtio_console_config_t;

__END_CDECLS

#endif  // VIRTIO_CONSOLE_H_
