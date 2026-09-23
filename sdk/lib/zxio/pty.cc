// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fcntl.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.pty/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fit/defer.h>
#include <lib/zx/channel.h>
#include <lib/zxio/null.h>
#include <lib/zxio/ops.h>
#include <lib/zxio/types.h>
#include <sys/stat.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>

#include <cstddef>
#include <cstdint>

#include "sdk/lib/zxio/private.h"

namespace fdevice = fuchsia_device;
namespace fio = fuchsia_io;

class Pty : public HasIo {
 public:
  Pty(fidl::ClientEnd<fuchsia_hardware_pty::Device> client_end, zx::eventpair event)
      : HasIo(kOps), client_(std::move(client_end)), event_(std::move(event)) {}

  void Destroy() { this->~Pty(); }

  zx_status_t Close() {
    if (client_.is_valid()) {
      const fidl::WireResult result = client_->Close();
      if (!result.ok()) {
        return result.status();
      }
      const auto& response = result.value();
      if (response.is_error()) {
        return response.error_value();
      }
    }
    return ZX_OK;
  }

  zx_status_t Release(zx_handle_t* out_handle) {
    *out_handle = client_.TakeClientEnd().TakeChannel().release();
    return ZX_OK;
  }

  zx_status_t Borrow(zx_handle_t* out_handle) {
    *out_handle = client_.client_end().channel().get();
    return ZX_OK;
  }

  zx_status_t Clone(zx_handle_t* out_handle) {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_unknown::Cloneable>::Create();
    const fidl::Status result = client_->Clone(std::move(server_end));
    if (!result.ok()) {
      return result.status();
    }
    *out_handle = client_end.TakeChannel().release();
    return ZX_OK;
  }

  void WaitBegin(zxio_signals_t zxio_signals, zx_handle_t* out_handle,
                 zx_signals_t* out_zx_signals) {
    *out_handle = event_.get();

    zx_signals_t zx_signals = ZX_SIGNAL_NONE;
    zx_signals |= [zxio_signals]() {
      fdevice::wire::DeviceSignal signals;
      if (zxio_signals & ZXIO_SIGNAL_READABLE) {
        signals |= fdevice::wire::DeviceSignal::kReadable;
      }
      if (zxio_signals & ZXIO_SIGNAL_OUT_OF_BAND) {
        signals |= fdevice::wire::DeviceSignal::kOob;
      }
      if (zxio_signals & ZXIO_SIGNAL_WRITABLE) {
        signals |= fdevice::wire::DeviceSignal::kWritable;
      }
      if (zxio_signals & ZXIO_SIGNAL_ERROR) {
        signals |= fdevice::wire::DeviceSignal::kError;
      }
      if (zxio_signals & ZXIO_SIGNAL_PEER_CLOSED) {
        signals |= fdevice::wire::DeviceSignal::kHangup;
      }
      return static_cast<zx_signals_t>(signals);
    }();
    if (zxio_signals & ZXIO_SIGNAL_READ_DISABLED) {
      zx_signals |= ZX_CHANNEL_PEER_CLOSED;
    }
    *out_zx_signals = zx_signals;
  }

  void WaitEnd(zx_signals_t zx_signals, zxio_signals_t* out_zxio_signals) {
    zxio_signals_t zxio_signals = ZXIO_SIGNAL_NONE;
    [&zxio_signals, signals = fdevice::wire::DeviceSignal::TruncatingUnknown(zx_signals)]() {
      if (signals & fdevice::wire::DeviceSignal::kReadable) {
        zxio_signals |= ZXIO_SIGNAL_READABLE;
      }
      if (signals & fdevice::wire::DeviceSignal::kOob) {
        zxio_signals |= ZXIO_SIGNAL_OUT_OF_BAND;
      }
      if (signals & fdevice::wire::DeviceSignal::kWritable) {
        zxio_signals |= ZXIO_SIGNAL_WRITABLE;
      }
      if (signals & fdevice::wire::DeviceSignal::kError) {
        zxio_signals |= ZXIO_SIGNAL_ERROR;
      }
      if (signals & fdevice::wire::DeviceSignal::kHangup) {
        zxio_signals |= ZXIO_SIGNAL_PEER_CLOSED;
      }
    }();
    if (zx_signals & ZX_CHANNEL_PEER_CLOSED) {
      zxio_signals |= ZXIO_SIGNAL_READ_DISABLED;
    }
    *out_zxio_signals = zxio_signals;
  }

  zx_status_t Readv(const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                    size_t* out_actual) {
    // fuchsia.hardware.pty/Device composes fuchsia.io/Readable.
    return RemoteReadv(fidl::UnownedClientEnd<fio::Readable>(client_.client_end().handle()), vector,
                       vector_count, flags, out_actual);
  }

  zx_status_t Writev(const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                     size_t* out_actual) {
    // fuchsia.hardware.pty/Device composes fuchsia.io/Writable.
    return RemoteWritev(fidl::UnownedClientEnd<fio::Writable>(client_.client_end().handle()),
                        vector, vector_count, flags, out_actual);
  }

  zx_status_t IsAtty(bool* tty) {
    *tty = true;
    return ZX_OK;
  }

  zx_status_t GetWindowSize(uint32_t* width, uint32_t* height) {
    if (!client_.is_valid()) {
      return ZX_ERR_BAD_STATE;
    }
    const fidl::WireResult result = client_->GetWindowSize();
    if (!result.ok()) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    const auto& response = result.value();
    if (response.status != ZX_OK) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    *width = response.size.width;
    *height = response.size.height;
    return ZX_OK;
  }

  zx_status_t SetWindowSize(uint32_t width, uint32_t height) {
    if (!client_.is_valid()) {
      return ZX_ERR_BAD_STATE;
    }

    const fuchsia_hardware_pty::wire::WindowSize size = {
        .width = width,
        .height = height,
    };

    const fidl::WireResult result = client_->SetWindowSize(size);
    if (!result.ok()) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    const auto& response = result.value();
    if (response.status != ZX_OK) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    return ZX_OK;
  }

 private:
  static const zxio_ops_t kOps;
  fidl::WireSyncClient<fuchsia_hardware_pty::Device> client_;
  const zx::eventpair event_;
};

constexpr zxio_ops_t Pty::kOps = ([]() {
  using Adaptor = Adaptor<Pty>;
  zxio_ops_t ops = zxio_default_ops;
  ops.destroy = Adaptor::From<&Pty::Destroy>;
  ops.close = Adaptor::From<&Pty::Close>;
  ops.release = Adaptor::From<&Pty::Release>;
  ops.borrow = Adaptor::From<&Pty::Borrow>;
  ops.clone = Adaptor::From<&Pty::Clone>;

  ops.wait_begin = Adaptor::From<&Pty::WaitBegin>;
  ops.wait_end = Adaptor::From<&Pty::WaitEnd>;
  ops.readv = Adaptor::From<&Pty::Readv>;
  ops.writev = Adaptor::From<&Pty::Writev>;

  ops.isatty = Adaptor::From<&Pty::IsAtty>;
  ops.get_window_size = Adaptor::From<&Pty::GetWindowSize>;
  ops.set_window_size = Adaptor::From<&Pty::SetWindowSize>;
  return ops;
})();

zx_status_t zxio_pty_init(zxio_storage_t* storage, zx::eventpair event,
                          fidl::ClientEnd<fuchsia_hardware_pty::Device> client) {
  new (storage) Pty(std::move(client), std::move(event));
  return ZX_OK;
}
