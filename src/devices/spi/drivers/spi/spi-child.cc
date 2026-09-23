// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "spi-child.h"

#include <lib/async/cpp/task.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/trace/event.h>
#include <zircon/errors.h>

#include "sdk/lib/driver/logging/cpp/logger.h"

namespace spi {

template <typename T>
inline zx_status_t FidlStatus(const T& result) {
  if (result.ok()) {
    return result->is_ok() ? ZX_OK : result->error_value();
  }
  return result.status();
}

void SpiChild::OpenSession(OpenSessionRequestView request, OpenSessionCompleter::Sync& completer) {
  Bind(std::move(request->session));
}

void SpiChild::TransmitVector(TransmitVectorRequestView request,
                              TransmitVectorCompleter::Sync& completer) {
  fdf::Arena arena('SPI_');
  spi_.buffer(arena)
      ->TransmitVector(cs_, request->data)
      .ThenExactlyOnce(
          fit::inline_callback<
              void(fdf::WireUnownedResult<fuchsia_hardware_spiimpl::SpiImpl::TransmitVector>&),
              sizeof(TransmitVectorCompleter::Async)>(
              [completer = completer.ToAsync()](auto& result) mutable {
                if (zx_status_t status = FidlStatus(result); status != ZX_OK) {
                  fdf::error("Couldn't complete SpiImpl::TransmitVector: {}",
                             zx_status_get_string(status));
                  completer.Reply(status);
                } else {
                  completer.Reply(ZX_OK);
                }
              }));
}

void SpiChild::ReceiveVector(ReceiveVectorRequestView request,
                             ReceiveVectorCompleter::Sync& completer) {
  struct {
    uint32_t size;
    ReceiveVectorCompleter::Async completer;
  } request_data{request->size, completer.ToAsync()};

  fdf::Arena arena('SPI_');
  spi_.buffer(arena)
      ->ReceiveVector(cs_, request->size)
      .ThenExactlyOnce(
          fit::inline_callback<
              void(fdf::WireUnownedResult<fuchsia_hardware_spiimpl::SpiImpl::ReceiveVector>&),
              sizeof(request_data)>([request_data = std::move(request_data)](auto& result) mutable {
            if (zx_status_t status = FidlStatus(result); status != ZX_OK) {
              fdf::error("Couldn't complete SpiImpl::ReceiveVector: {}",
                         zx_status_get_string(status));
              request_data.completer.Reply(status, {});
            } else if (result->value()->data.size() != request_data.size) {
              fdf::error("Expected {} bytes != received {} bytes", request_data.size,
                         result->value()->data.size());
              request_data.completer.Reply(ZX_ERR_INTERNAL, {});
            } else {
              request_data.completer.Reply(ZX_OK, result->value()->data);
            }
          }));
}

void SpiChild::ExchangeVector(ExchangeVectorRequestView request,
                              ExchangeVectorCompleter::Sync& completer) {
  struct {
    size_t txdata_count;
    ExchangeVectorCompleter::Async completer;
  } request_data{request->txdata.size(), completer.ToAsync()};

  fdf::Arena arena('SPI_');
  spi_.buffer(arena)
      ->ExchangeVector(cs_, request->txdata)
      .ThenExactlyOnce(
          fit::inline_callback<
              void(fdf::WireUnownedResult<fuchsia_hardware_spiimpl::SpiImpl::ExchangeVector>&),
              sizeof(request_data)>([request_data = std::move(request_data)](auto& result) mutable {
            if (zx_status_t status = FidlStatus(result); status != ZX_OK) {
              fdf::error("Couldn't complete SpiImpl::ExchangeVector: {}",
                         zx_status_get_string(status));
              request_data.completer.Reply(status, {});
            } else if (result->value()->rxdata.size() != request_data.txdata_count) {
              fdf::error("Expected {} bytes != received {} bytes", request_data.txdata_count,
                         result->value()->rxdata.size());
              request_data.completer.Reply(ZX_ERR_INTERNAL, {});
            } else {
              request_data.completer.Reply(ZX_OK, result->value()->rxdata);
            }
          }));
}

void SpiChild::RegisterVmo(RegisterVmoRequestView request, RegisterVmoCompleter::Sync& completer) {
  fdf::Arena arena('SPI_');
  spi_.buffer(arena)
      ->RegisterVmo(cs_, request->vmo_id, std::move(request->vmo), request->rights)
      .ThenExactlyOnce(
          fit::inline_callback<
              void(fdf::WireUnownedResult<fuchsia_hardware_spiimpl::SpiImpl::RegisterVmo>&),
              sizeof(RegisterVmoCompleter::Async)>(
              [completer = completer.ToAsync()](auto& result) mutable {
                if (zx_status_t status = FidlStatus(result); status != ZX_OK) {
                  fdf::error("Couldn't complete SpiImpl::RegisterVmo: {}",
                             zx_status_get_string(status));
                  completer.ReplyError(status);
                } else {
                  completer.ReplySuccess();
                }
              }));
}

void SpiChild::UnregisterVmo(UnregisterVmoRequestView request,
                             UnregisterVmoCompleter::Sync& completer) {
  fdf::Arena arena('SPI_');
  spi_.buffer(arena)
      ->UnregisterVmo(cs_, request->vmo_id)
      .ThenExactlyOnce(
          fit::inline_callback<
              void(fdf::WireUnownedResult<fuchsia_hardware_spiimpl::SpiImpl::UnregisterVmo>&),
              sizeof(UnregisterVmoCompleter::Async)>(
              [completer = completer.ToAsync()](auto& result) mutable {
                if (zx_status_t status = FidlStatus(result); status != ZX_OK) {
                  fdf::error("Couldn't complete SpiImpl::UnregisterVmo: {}",
                             zx_status_get_string(status));
                  completer.ReplyError(status);
                } else {
                  completer.ReplySuccess(std::move(result->value()->vmo));
                }
              }));
}

void SpiChild::Transmit(TransmitRequestView request, TransmitCompleter::Sync& completer) {
  TRACE_DURATION("spi", "Transmit", "cs", cs_, "size", request->buffer.size);
  fdf::Arena arena('SPI_');
  spi_.buffer(arena)
      ->TransmitVmo(cs_, request->buffer)
      .ThenExactlyOnce(
          fit::inline_callback<
              void(fdf::WireUnownedResult<fuchsia_hardware_spiimpl::SpiImpl::TransmitVmo>&),
              sizeof(TransmitCompleter::Async)>(
              [completer = completer.ToAsync()](auto& result) mutable {
                if (zx_status_t status = FidlStatus(result); status != ZX_OK) {
                  fdf::error("Couldn't complete SpiImpl::TransmitVmo: {}",
                             zx_status_get_string(status));
                  completer.ReplyError(status);
                } else {
                  completer.ReplySuccess();
                }
              }));
}

void SpiChild::Receive(ReceiveRequestView request, ReceiveCompleter::Sync& completer) {
  TRACE_DURATION("spi", "Receive", "cs", cs_, "size", request->buffer.size);
  fdf::Arena arena('SPI_');
  spi_.buffer(arena)
      ->ReceiveVmo(cs_, request->buffer)
      .ThenExactlyOnce(fit::inline_callback<
                       void(fdf::WireUnownedResult<fuchsia_hardware_spiimpl::SpiImpl::ReceiveVmo>&),
                       sizeof(ReceiveCompleter::Async)>(
          [completer = completer.ToAsync()](auto& result) mutable {
            if (zx_status_t status = FidlStatus(result); status != ZX_OK) {
              fdf::error("Couldn't complete SpiImpl::ReceiveVmo: {}", zx_status_get_string(status));
              completer.ReplyError(status);
            } else {
              completer.ReplySuccess();
            }
          }));
}

void SpiChild::Exchange(ExchangeRequestView request, ExchangeCompleter::Sync& completer) {
  if (request->tx_buffer.size != request->rx_buffer.size) {
    fdf::error("tx_buffer and rx_buffer size must match. {} (tx) != {} (rx)",
               request->tx_buffer.size, request->rx_buffer.size);
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  TRACE_DURATION("spi", "Exchange", "cs", cs_, "size", request->tx_buffer.size);
  fdf::Arena arena('SPI_');
  spi_.buffer(arena)
      ->ExchangeVmo(cs_, request->tx_buffer, request->rx_buffer)
      .ThenExactlyOnce(
          fit::inline_callback<
              void(fdf::WireUnownedResult<fuchsia_hardware_spiimpl::SpiImpl::ExchangeVmo>&),
              sizeof(ExchangeCompleter::Async)>(
              [completer = completer.ToAsync()](auto& result) mutable {
                if (zx_status_t status = FidlStatus(result); status != ZX_OK) {
                  fdf::error("Couldn't complete SpiImpl::ExchangeVmo: {}",
                             zx_status_get_string(status));
                  completer.ReplyError(status);
                } else {
                  completer.ReplySuccess();
                }
              }));
}

void SpiChild::CanAssertCs(CanAssertCsCompleter::Sync& completer) {
  completer.Reply(!has_siblings_);
}

void SpiChild::AssertCs(AssertCsCompleter::Sync& completer) {
  if (has_siblings_) {
    completer.Reply(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  fdf::Arena arena('SPI_');
  spi_.buffer(arena)->LockBus(cs_).ThenExactlyOnce(
      fit::inline_callback<void(
                               fdf::WireUnownedResult<fuchsia_hardware_spiimpl::SpiImpl::LockBus>&),
                           sizeof(AssertCsCompleter::Async)>(
          [completer = completer.ToAsync()](auto& result) mutable {
            if (zx_status_t status = FidlStatus(result); status != ZX_OK) {
              fdf::error("SpiImpl::LockBus failed: {}", zx_status_get_string(status));
              completer.Reply(status);
            } else {
              completer.Reply(ZX_OK);
            }
          }));
}

void SpiChild::DeassertCs(DeassertCsCompleter::Sync& completer) {
  if (has_siblings_) {
    completer.Reply(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  fdf::Arena arena('SPI_');
  spi_.buffer(arena)->UnlockBus(cs_).ThenExactlyOnce(
      fit::inline_callback<
          void(fdf::WireUnownedResult<fuchsia_hardware_spiimpl::SpiImpl::UnlockBus>&),
          sizeof(DeassertCsCompleter::Async)>(
          [completer = completer.ToAsync()](auto& result) mutable {
            if (zx_status_t status = FidlStatus(result); status != ZX_OK) {
              fdf::error("SpiImpl::UnlockBus failed: {}", zx_status_get_string(status));
              completer.Reply(status);
            } else {
              completer.Reply(ZX_OK);
            }
          }));
}

void SpiChild::Bind(fidl::ServerEnd<fuchsia_hardware_spi::Device> server_end) {
  // binding_ can only be accessed on fidl_dispatcher_.
  async::PostTask(
      fidl_dispatcher_->async_dispatcher(), [this, server_end = std::move(server_end)]() mutable {
        if (binding_.has_value()) {
          server_end.Close(ZX_ERR_ALREADY_BOUND);
          return;
        }
        binding_.emplace(fidl_dispatcher_->async_dispatcher(), std::move(server_end), this,
                         [](SpiChild* self, fidl::UnbindInfo info) {
                           self->binding_.reset();

                           fdf::Arena arena('SPI_');
                           (void)self->spi_.sync().buffer(arena)->ReleaseRegisteredVmos(self->cs_);
                         });
      });
}

fuchsia_hardware_spi::Service::InstanceHandler SpiChild::CreateInstanceHandler() {
  return fuchsia_hardware_spi::Service::InstanceHandler({
      .device =
          [this](fidl::ServerEnd<fuchsia_hardware_spi::Device> server_end) {
            Bind(std::move(server_end));
          },
  });
}

void SpiChild::DevfsConnect(fidl::ServerEnd<fuchsia_hardware_spi::Controller> server) {
  controller_bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                  std::move(server), this, fidl::kIgnoreBindingClosure);
}

}  // namespace spi
