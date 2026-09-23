// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon_platform_port.h"

#include <lib/magma/util/dlog.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma/util/utils.h>
#include <lib/zx/time.h>

#include "zircon/syscalls/port.h"

namespace magma {

Status ZirconPlatformPort::Wait(uint64_t* key_out, uint64_t timeout_ms,
                                uint64_t* trigger_time_out) {
  if (trigger_time_out) {
    *trigger_time_out = 0;
  }
  if (!port_)
    return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "wait on invalid port");

  zx_port_packet_t packet;
  zx_status_t status =
      port_.wait(zx::deadline_after(zx::duration(magma::ms_to_signed_ns(timeout_ms))), &packet);
  if (status == ZX_ERR_TIMED_OUT) {
    return MAGMA_STATUS_TIMED_OUT;
  }

  DLOG("port received key 0x%" PRIx64 " status %d", packet.key, status);

  if (status != ZX_OK)
    return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "port wait returned error: %d", status);

  if (packet.type == ZX_PKT_TYPE_USER) {
    // Port has been closed.
    port_.reset();
    return MAGMA_STATUS_INTERNAL_ERROR;
  } else if (packet.type == ZX_PKT_TYPE_SIGNAL_ONE &&
             packet.signal.observed == ZX_CHANNEL_PEER_CLOSED) {
    return DRET(MAGMA_STATUS_CONNECTION_LOST);
  } else if (packet.type == ZX_PKT_TYPE_INTERRUPT) {
    if (trigger_time_out)
      *trigger_time_out = packet.interrupt.timestamp;
  }

  *key_out = packet.key;
  return MAGMA_STATUS_OK;
}

std::unique_ptr<PlatformPort> PlatformPort::Create() {
  zx::port port;
  zx_status_t status = zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port);
  if (status != ZX_OK)
    return DRETP(nullptr, "port::create failed: %d", status);

  return std::make_unique<ZirconPlatformPort>(std::move(port));
}

}  // namespace magma
