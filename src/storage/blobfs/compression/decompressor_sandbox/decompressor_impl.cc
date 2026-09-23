// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef _ALL_SOURCE
#define _ALL_SOURCE  // Enables thrd_create_with_name in <threads.h>.
#endif

#include "src/storage/blobfs/compression/decompressor_sandbox/decompressor_impl.h"

#include <fidl/fuchsia.blobfs.internal/cpp/wire.h>
#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/scheduler/role.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/fifo.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <lib/zx/time.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <memory>
#include <utility>

#include <fbl/algorithm.h>
#include <safemath/checked_math.h>

#include "src/lib/chunked-compression/chunked-decompressor.h"
#include "src/storage/blobfs/compression/decompressor.h"
#include "src/storage/blobfs/compression/external_decompressor.h"
#include "src/storage/blobfs/compression_settings.h"
#include "src/storage/lib/trace/trace.h"

namespace blobfs {
namespace {

struct FifoInfo {
  zx::fifo fifo;
  fzl::OwnedVmoMapper compressed_mapper;
  fzl::OwnedVmoMapper decompressed_mapper;
};

void CommitPages(const fzl::OwnedVmoMapper& mapper,
                 const fuchsia_blobfs_internal::wire::Range range, uint32_t op) {
  const uint64_t page_size = zx_system_get_page_size();
  const uint64_t aligned_start = fbl::round_down(range.offset, page_size);
  const uint64_t aligned_end = fbl::round_up(range.offset + range.size, page_size);
  const uint64_t aligned_length = aligned_end - aligned_start;

  const uint8_t* aligned_address = static_cast<const uint8_t*>(mapper.start()) + aligned_start;
  if (zx_status_t status = zx::vmar::root_self()->op_range(
          op, reinterpret_cast<uint64_t>(aligned_address), aligned_length, nullptr, 0);
      status != ZX_OK) {
    FX_PLOGS(WARNING, status) << "Failed to commit pages";
    ZX_DEBUG_ASSERT_MSG(false, "Failed to commit pages");
  }
}

// This will only decompress a set of complete chunks, if the beginning or end
// of the range are not chunk aligned this operation will fail.
zx::result<uint64_t> DecompressChunkedPartial(
    const fzl::OwnedVmoMapper& decompressed_mapper, const fzl::OwnedVmoMapper& compressed_mapper,
    const fuchsia_blobfs_internal::wire::Range decompressed,
    const fuchsia_blobfs_internal::wire::Range compressed) {
  const uint8_t* src = static_cast<const uint8_t*>(compressed_mapper.start()) + compressed.offset;
  uint8_t* dst = static_cast<uint8_t*>(decompressed_mapper.start()) + decompressed.offset;

  // The mappings are reused across requests but blobfs decommits all of the pages out of the
  // compressed vmo and transfers the pages out of the decompressed vmo after each request. The
  // pages in the compressed vmo will be repopulated but they need to be re-added to this process's
  // page table. The decompressed vmo won't have any pages in it. Committing pages to the vmo
  // through the vmar will also add the pages to this process's page table. Without these calls, the
  // pages will be committed and added to the page table when they are accessed. Making the calls
  // ensures that the entire range is committed and added to the page table at once which could
  // avoid multiple page faults.
  CommitPages(compressed_mapper, compressed, ZX_VMAR_OP_MAP_RANGE);
  CommitPages(decompressed_mapper, decompressed, ZX_VMAR_OP_COMMIT);

  chunked_compression::ChunkedDecompressor decompressor;
  uint64_t bytes_decompressed = 0;
  if (zx_status_t status = decompressor.DecompressFrame(src, compressed.size, dst,
                                                        decompressed.size, &bytes_decompressed);
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(bytes_decompressed);
}

zx::result<uint64_t> DecompressFull(const fzl::OwnedVmoMapper& decompressed_mapper,
                                    const fzl::OwnedVmoMapper& compressed_mapper,
                                    size_t decompressed_length, size_t compressed_length,
                                    CompressionAlgorithm algorithm) {
  std::unique_ptr<Decompressor> decompressor = nullptr;
  if (zx_status_t status = Decompressor::Create(algorithm, &decompressor); status != ZX_OK) {
    return zx::error(status);
  }
  uint64_t bytes_decompressed = decompressed_length;
  if (zx_status_t status =
          decompressor->Decompress(decompressed_mapper.start(), &bytes_decompressed,
                                   compressed_mapper.start(), compressed_length);
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(bytes_decompressed);
}

bool RangeInBounds(const fzl::OwnedVmoMapper& mapper,
                   const fuchsia_blobfs_internal::wire::Range range) {
  uint64_t end;
  return (safemath::CheckAdd(range.offset, range.size).AssignIfValid(&end) && end <= mapper.size());
}

// The actual handling of a request on the fifo.
zx::result<uint64_t> HandleFifoRequest(
    const fzl::OwnedVmoMapper& compressed_mapper, const fzl::OwnedVmoMapper& decompressed_mapper,
    const fuchsia_blobfs_internal::wire::DecompressRequest& request) {
  TRACE_DURATION("decompressor", "HandleFifo", "length", request.decompressed.size);

  if (!RangeInBounds(decompressed_mapper, request.decompressed) ||
      !RangeInBounds(compressed_mapper, request.compressed)) {
    FX_LOGS(ERROR) << "Requested vmo ranges fall outside the mapped vmo.";
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  switch (request.algorithm) {
    case fuchsia_blobfs_internal::wire::CompressionAlgorithm::kChunkedPartial:
      return DecompressChunkedPartial(decompressed_mapper, compressed_mapper, request.decompressed,
                                      request.compressed);
    case fuchsia_blobfs_internal::wire::CompressionAlgorithm::kLz4:
    case fuchsia_blobfs_internal::wire::CompressionAlgorithm::kZstd:
    case fuchsia_blobfs_internal::wire::CompressionAlgorithm::kZstdSeekable:
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    case fuchsia_blobfs_internal::wire::CompressionAlgorithm::kChunked:
      if (request.decompressed.offset != 0 || request.compressed.offset != 0) {
        return zx::error(ZX_ERR_NOT_SUPPORTED);
      }
      if (auto algorithm_or =
              ExternalDecompressorClient::CompressionAlgorithmFidlToLocal(request.algorithm)) {
        return DecompressFull(decompressed_mapper, compressed_mapper, request.decompressed.size,
                              request.compressed.size, *algorithm_or);
      }
      // Invalid compression algorithm.
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    case fuchsia_blobfs_internal::wire::CompressionAlgorithm::kUncompressed:
      return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
}

zx_status_t SendFifoResponse(const zx::fifo& fifo,
                             const fuchsia_blobfs_internal::wire::DecompressResponse& response) {
  constexpr zx_signals_t kFifoWriteSignals = ZX_FIFO_WRITABLE | ZX_FIFO_PEER_CLOSED;
  constexpr size_t kResponseSize = sizeof(fuchsia_blobfs_internal::wire::DecompressResponse);

  // There's a 1-to-1 mapping between the blobfs pager threads and decompressor threads and they
  // both handle requests synchronously. If a request was received on the fifo then the previous
  // response should have already been read from the fifo and the fifo should be empty. Only wait
  // for the fifo to be writable if writing to the fifo initially fails.
  if (zx_status_t status = fifo.write(kResponseSize, &response, 1, nullptr);
      status != ZX_ERR_SHOULD_WAIT) {
    return status;
  }

  // Wait for the fifo to be writable or blobfs to close the other end.
  zx_signals_t signal;
  if (zx_status_t status = fifo.wait_one(kFifoWriteSignals, zx::time::infinite(), &signal);
      status != ZX_OK) {
    return status;
  }
  if ((signal & ZX_FIFO_PEER_CLOSED) != 0) {
    return ZX_ERR_PEER_CLOSED;
  }

  // Attempt to write again.
  return fifo.write(kResponseSize, &response, 1, nullptr);
}

zx_status_t WaitForFifoRequest(const zx::fifo& fifo,
                               fuchsia_blobfs_internal::wire::DecompressRequest& request) {
  constexpr zx_signals_t kFifoReadSignals = ZX_FIFO_READABLE | ZX_FIFO_PEER_CLOSED;
  constexpr size_t kRequestSize = sizeof(fuchsia_blobfs_internal::wire::DecompressRequest);

  // There's a 1-to-1 mapping between the blobfs pager threads and decompressor threads and they
  // both handle requests synchronously. Blobfs is unlikely to have sent another request immediately
  // after receiving the previous response. Wait for the fifo to be readable rather than immediately
  // trying to read.
  zx_signals_t signal;
  if (zx_status_t status = fifo.wait_one(kFifoReadSignals, zx::time::infinite(), &signal);
      status != ZX_OK) {
    return status;
  }
  // It doesn't matter if there's anything left in the queue, nobody is there
  // to read the response.
  if ((signal & ZX_FIFO_PEER_CLOSED) != 0) {
    return ZX_ERR_PEER_CLOSED;
  }

  return fifo.read(kRequestSize, &request, 1, nullptr);
}

// Watches a fifo for requests to take data from the compressed_vmo and
// extract the result into the memory region of decompressed_mapper.
void WatchFifo(zx::fifo fifo, fzl::OwnedVmoMapper compressed_mapper,
               fzl::OwnedVmoMapper decompressed_mapper) {
  while (true) {
    fuchsia_blobfs_internal::wire::DecompressRequest request;
    if (zx_status_t status = WaitForFifoRequest(fifo, request); status != ZX_OK) {
      break;
    }

    zx::result result = HandleFifoRequest(compressed_mapper, decompressed_mapper, request);
    fuchsia_blobfs_internal::wire::DecompressResponse response;
    if (result.is_error()) {
      response.status = result.status_value();
      response.size = 0;
    } else {
      response.status = ZX_OK;
      response.size = result.value();
    }

    if (zx_status_t status = SendFifoResponse(fifo, response); status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Failed to send decompression response";
      break;
    }
  }
}

// A Wrapper around WatchFifo just to unwrap the data in the callback provided
// by thrd_create_With_name().
int WatchFifoWrapper(void* data) {
  std::unique_ptr<FifoInfo> info(static_cast<FifoInfo*>(data));
  WatchFifo(std::move(info->fifo), std::move(info->compressed_mapper),
            std::move(info->decompressed_mapper));
  return 0;
}

void SetDeadlineProfile(thrd_t& thread) {
  const zx::unowned_thread borrowed_thread{thrd_get_zx_handle(thread)};
  const char role_name[]{"fuchsia.storage.blobfs.compression.decompressor"};
  zx_status_t status = fuchsia_scheduler::SetRoleForThread(borrowed_thread->borrow(), role_name);
  if (status != ZX_OK) {
    FX_PLOGS(WARNING, status) << "[decompressor]: Failed to set role";
    return;
  }
}

zx_status_t CreateFifoThread(zx::fifo server_end, zx::vmo compressed_vmo,
                             zx::vmo decompressed_vmo) {
  size_t vmo_size;
  if (zx_status_t status = decompressed_vmo.get_size(&vmo_size); status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to get `decompressed_vmo` size";
    return status;
  }

  fzl::OwnedVmoMapper decompressed_mapper;
  if (zx_status_t status = decompressed_mapper.Map(std::move(decompressed_vmo), 0, vmo_size,
                                                   ZX_VM_PERM_READ | ZX_VM_PERM_WRITE);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to map `decompressed_vmo`";
    return status;
  }

  if (zx_status_t status = compressed_vmo.get_size(&vmo_size); status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to get `compressed_vmo` size";
    return status;
  }

  fzl::OwnedVmoMapper compressed_mapper;
  if (zx_status_t status =
          compressed_mapper.Map(std::move(compressed_vmo), 0, vmo_size, ZX_VM_PERM_READ);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to map `compressed_vmo`";
    return status;
  }

  thrd_t handler_thread;
  std::unique_ptr<FifoInfo> info = std::make_unique<FifoInfo>();
  *info = {std::move(server_end), std::move(compressed_mapper), std::move(decompressed_mapper)};
  if (thrd_create_with_name(&handler_thread, WatchFifoWrapper, info.get(),
                            "decompressor-fifo-thread") != thrd_success) {
    FX_LOGS(ERROR) << "Failed to create decompressor FIFO thread!";
    return ZX_ERR_INTERNAL;
  }
  info.release();
  SetDeadlineProfile(handler_thread);

  thrd_detach(handler_thread);
  return ZX_OK;
}

}  // namespace

void DecompressorImpl::Create(
    ::fuchsia_blobfs_internal::wire::DecompressorCreatorCreateRequest* request,
    CreateCompleter::Sync& completer) {
  FX_LOGS(DEBUG) << "Creating decompressor.";
  completer.Reply(CreateFifoThread(std::move(request->server_end),
                                   std::move(request->compressed_vmo),
                                   std::move(request->decompressed_vmo)));
}

}  // namespace blobfs
