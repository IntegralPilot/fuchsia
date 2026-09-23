// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/compression/external_decompressor.h"

#include <fidl/fuchsia.blobfs.internal/cpp/wire.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/fifo.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <cstddef>
#include <memory>
#include <optional>
#include <utility>

#include "src/storage/blobfs/compression_settings.h"

namespace blobfs {

DecompressorCreatorConnector& DecompressorCreatorConnector::DefaultServiceConnector() {
  class ServiceConnector final : public DecompressorCreatorConnector {
   public:
    // DecompressorCreatorConnector interface.
    zx_status_t ConnectToDecompressorCreator(
        fidl::ServerEnd<fuchsia_blobfs_internal::DecompressorCreator> remote_channel) final {
      return fdio_service_connect("/svc/fuchsia.blobfs.internal.DecompressorCreator",
                                  remote_channel.TakeChannel().release());
    }
  };
  static ServiceConnector singleton{};
  return singleton;
}

zx::result<std::unique_ptr<ExternalDecompressorClient>> ExternalDecompressorClient::Create(
    DecompressorCreatorConnector* connector, const zx::vmo& decompressed_vmo,
    const zx::vmo& compressed_vmo) {
  std::unique_ptr<ExternalDecompressorClient> client;
  client.reset(new ExternalDecompressorClient());
  client->connector_ = connector;

  zx_status_t status =
      decompressed_vmo.duplicate(ZX_DEFAULT_VMO_RIGHTS, &(client->decompressed_vmo_));
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to duplicate decompressed VMO: " << zx_status_get_string(status);
    return zx::error(status);
  }
  status = compressed_vmo.duplicate(ZX_DEFAULT_VMO_RIGHTS & (~ZX_RIGHT_WRITE),
                                    &(client->compressed_vmo_));
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to duplicate compressed VMO: " << zx_status_get_string(status);
    return zx::error(status);
  }
  status = client->ConnectToDecompressor();
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(client));
}

zx_status_t ExternalDecompressorClient::ConnectToDecompressor() {
  if (zx_status_t status = PrepareDecompressorCreator(); status != ZX_OK) {
    return status;
  }

  zx::vmo remote_decompressed_vmo;
  if (zx_status_t status =
          decompressed_vmo_.duplicate(ZX_RIGHT_SAME_RIGHTS, &remote_decompressed_vmo);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to create remote duplicate of decompressed VMO";
    return status;
  }

  zx::vmo remote_compressed_vmo;
  if (zx_status_t status = compressed_vmo_.duplicate(ZX_RIGHT_SAME_RIGHTS, &remote_compressed_vmo);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to create remote duplicate of compressed VMO";
    return status;
  }

  zx::fifo remote_fifo;
  // Sized for 4 elements, allows enough pipelining to keep the remote process
  // from descheduling to have 2 in flight requests/response pairs.
  if (zx_status_t status = zx::fifo::create(
          4, sizeof(fuchsia_blobfs_internal::wire::DecompressRequest), 0, &fifo_, &remote_fifo);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed create fifo for external decompressor";
    return status;
  }

  auto fidl_result = fidl::WireCall(decompressor_creator_)
                         ->Create(std::move(remote_fifo), std::move(remote_compressed_vmo),
                                  std::move(remote_decompressed_vmo));
  if (fidl_result.is_peer_closed()) {
    decompressor_creator_.reset();
  }
  if (!fidl_result.ok()) {
    FX_LOGS(ERROR) << "FIDL error communicating with external decompressor: "
                   << fidl_result.FormatDescription();
    return fidl_result.status();
  }
  if (fidl_result.value().status != ZX_OK) {
    FX_PLOGS(ERROR, fidl_result.value().status)
        << "Error calling Create on DecompressorCreator service";
  }
  return fidl_result.value().status;
}

zx_status_t ExternalDecompressorClient::PrepareDecompressorCreator() {
  if (decompressor_creator_.is_valid()) {
    zx_signals_t signal;
    zx_status_t status = decompressor_creator_.channel().wait_one(
        ZX_CHANNEL_WRITABLE | ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(), &signal);
    if (status == ZX_OK && (signal & ZX_CHANNEL_PEER_CLOSED) == 0 &&
        (signal & ZX_CHANNEL_WRITABLE) != 0) {
      return ZX_OK;
    } else {
      decompressor_creator_.reset();
    }
  }

  auto remote_channel = fidl::CreateEndpoints(&decompressor_creator_);
  if (remote_channel.is_error()) {
    FX_PLOGS(ERROR, remote_channel.status_value())
        << "Failed to create channel pair for external decompressor.";
    return remote_channel.status_value();
  }

  if (zx_status_t status =
          connector_->ConnectToDecompressorCreator(std::move(remote_channel).value());
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to connect to DecompressorCreator service";
    decompressor_creator_.reset();
    return status;
  }

  return ZX_OK;
}

zx_status_t ExternalDecompressorClient::SendRequest(
    const fuchsia_blobfs_internal::wire::DecompressRequest& request) {
  zx_status_t write_status = fifo_.write(sizeof(request), &request, 1, nullptr);
  if (write_status == ZX_OK) {
    return ZX_OK;
  }
  bool try_to_reconnect = false;
  if (write_status == ZX_ERR_SHOULD_WAIT) {
    // The fifo is full, wait for it to become writable.
    zx_signals_t signals = 0;
    if (zx_status_t status =
            fifo_.wait_one(ZX_FIFO_WRITABLE | ZX_FIFO_PEER_CLOSED, zx::time::infinite(), &signals);
        status != ZX_OK) {
      return status;
    }
    if ((signals & ZX_FIFO_PEER_CLOSED) != 0) {
      // The other end of the fifo was closed while waiting for the fifo to become writable. Try to
      // make a new connection.
      try_to_reconnect = true;
    }
  } else if (write_status == ZX_ERR_PEER_CLOSED || write_status == ZX_ERR_BAD_HANDLE) {
    try_to_reconnect = true;
  } else {
    FX_PLOGS(ERROR, write_status) << "Unexpected response when writing to fifo";
    return write_status;
  }
  if (try_to_reconnect) {
    fifo_.reset();
    if (zx_status_t status = ConnectToDecompressor(); status != ZX_OK) {
      return status;
    }
  }
  // The original fifo should now be writable or a new connection was established.
  return fifo_.write(sizeof(request), &request, 1, nullptr);
}

zx_status_t ExternalDecompressorClient::SendMessage(
    const fuchsia_blobfs_internal::wire::DecompressRequest& request) {
  if (zx_status_t status = SendRequest(request); status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to write fifo request to decompressor";
    return status;
  }

  zx_signals_t signal;
  fifo_.wait_one(ZX_FIFO_READABLE | ZX_FIFO_PEER_CLOSED, zx::time::infinite(), &signal);
  if ((signal & ZX_FIFO_READABLE) == 0) {
    fifo_.reset();
    FX_LOGS(ERROR) << "External decompressor closed the fifo.";
    return ZX_ERR_INTERNAL;
  }

  fuchsia_blobfs_internal::wire::DecompressResponse response;
  if (zx_status_t status = fifo_.read(sizeof(response), &response, 1, nullptr); status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to read from fifo";
    return status;
  }
  if (response.status != ZX_OK) {
    FX_PLOGS(ERROR, response.status) << "Error from external decompressor";
    return response.status;
  }
  if (response.size != request.decompressed.size) {
    FX_LOGS(ERROR) << "Decompressed size did not match. Expected: " << request.decompressed.size
                   << " Got: " << response.size;
    return ZX_ERR_IO_DATA_INTEGRITY;
  }
  return ZX_OK;
}

std::optional<CompressionAlgorithm> ExternalDecompressorClient::CompressionAlgorithmFidlToLocal(
    const fuchsia_blobfs_internal::wire::CompressionAlgorithm algorithm) {
  using Fidl = fuchsia_blobfs_internal::wire::CompressionAlgorithm;
  switch (algorithm) {
    case Fidl::kUncompressed:
      return CompressionAlgorithm::kUncompressed;
    case Fidl::kChunked:
    case Fidl::kChunkedPartial:
      return CompressionAlgorithm::kChunked;
    default:
      return std::nullopt;
  }
}

fuchsia_blobfs_internal::wire::CompressionAlgorithm
ExternalDecompressorClient::CompressionAlgorithmLocalToFidl(CompressionAlgorithm algorithm) {
  using Fidl = fuchsia_blobfs_internal::wire::CompressionAlgorithm;
  switch (algorithm) {
    case CompressionAlgorithm::kUncompressed:
      return Fidl::kUncompressed;
    case CompressionAlgorithm::kChunked:
      return Fidl::kChunked;
  }

  ZX_DEBUG_ASSERT(false);
  return Fidl::kUncompressed;
}

zx::result<fuchsia_blobfs_internal::wire::CompressionAlgorithm>
ExternalDecompressorClient::CompressionAlgorithmLocalToFidlForPartial(
    CompressionAlgorithm algorithm) {
  switch (algorithm) {
    case CompressionAlgorithm::kChunked:
      return zx::ok(fuchsia_blobfs_internal::wire::CompressionAlgorithm::kChunkedPartial);
    case CompressionAlgorithm::kUncompressed:
      return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  ZX_DEBUG_ASSERT(false);
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

ExternalSeekableDecompressor::ExternalSeekableDecompressor(ExternalDecompressorClient* client,
                                                           CompressionAlgorithm algorithm)
    : client_(client), algorithm_(algorithm) {}

zx_status_t ExternalSeekableDecompressor::DecompressRange(size_t compressed_offset,
                                                          size_t compressed_size,
                                                          size_t uncompressed_size) {
  auto algorithm_or =
      ExternalDecompressorClient::CompressionAlgorithmLocalToFidlForPartial(algorithm_);
  if (!algorithm_or.is_ok()) {
    return algorithm_or.status_value();
  }
  fuchsia_blobfs_internal::wire::CompressionAlgorithm fidl_algorithm = algorithm_or.value();

  return client_->SendMessage({
      {0, uncompressed_size},
      {compressed_offset, compressed_size},
      fidl_algorithm,
  });
}

}  // namespace blobfs
