// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/lib/goldfish/pipe_io/pipe_auto_reader.h"

#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <lib/async/cpp/task.h>

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/zxlogf.h"

namespace goldfish {

// TODO(liyl): Currently the contents of the message should be read all at once.
// This should support reading long messages with pipe wait.
zx_status_t PipeAutoReader::AsyncReadWithHeader() {
  auto read_result = ReadWithHeader(false);
  if (read_result.is_ok()) {
    if (handler_) {
      handler_(std::move(read_result));
    }

    // Schedule another read.
    if (running_.load()) {
      async::PostTask(dispatcher_, [this]() { AsyncReadWithHeader(); });
    }
    return ZX_OK;
  }

  switch (read_result.error_value()) {
    case ZX_ERR_PEER_CLOSED: {
      zxlogf(INFO, "The pipe server is closed. Read is cancelled.");
      return read_result.error_value();
    }
    case ZX_ERR_SHOULD_WAIT: {
      pipe_event().signal(/* clear_mask= */
                          fuchsia_hardware_goldfish::wire::kSignalHangup |
                              fuchsia_hardware_goldfish::wire::kSignalReadable,
                          0u);
      wait_event_.set_object(pipe_event().get());
      wait_event_.set_trigger(fuchsia_hardware_goldfish::wire::kSignalHangup |
                              fuchsia_hardware_goldfish::wire::kSignalReadable);
      wait_event_.Begin(dispatcher_, [this](async_dispatcher_t* dispatcher, async::WaitOnce* wait,
                                            zx_status_t status, const zx_packet_signal_t* signal) {
        if (status == ZX_OK) {
          AsyncReadWithHeader();
        } else if (status == ZX_ERR_CANCELED) {
          zxlogf(INFO, "AsyncReadWithHeader callback: wait event is canceled");
        } else {
          zxlogf(ERROR, "AsyncReadWithHeader callback: wait event error: %d", status);
        }
      });
      return read_result.error_value();
    }
    default: {
      ZX_ASSERT_MSG(false, "Unexpected Pipe IO error: %s",
                    zx_status_get_string(read_result.error_value()));
    }
  }
}

zx_status_t PipeAutoReader::BeginRead() {
  running_.store(true);
  return async::PostTask(dispatcher_, [this]() { AsyncReadWithHeader(); });
}

void PipeAutoReader::StopRead() {
  running_.store(false);
  wait_event_.Cancel();
}
}  // namespace goldfish
