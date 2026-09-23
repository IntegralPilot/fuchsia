// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_COMPRESSION_DECOMPRESSOR_SANDBOX_DECOMPRESSOR_IMPL_H_
#define SRC_STORAGE_BLOBFS_COMPRESSION_DECOMPRESSOR_SANDBOX_DECOMPRESSOR_IMPL_H_

#include <fidl/fuchsia.blobfs.internal/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>

namespace blobfs {

// ensure inline request/response are the same sizes to fit in a FIFO
static_assert(fidl::TypeTraits<fuchsia_blobfs_internal::wire::DecompressRequest>::kPrimarySize ==
              fidl::TypeTraits<fuchsia_blobfs_internal::wire::DecompressResponse>::kPrimarySize);
static_assert(fidl::TypeTraits<fuchsia_blobfs_internal::wire::DecompressRequest>::kMaxOutOfLine ==
              0);
static_assert(fidl::TypeTraits<fuchsia_blobfs_internal::wire::DecompressResponse>::kMaxOutOfLine ==
              0);

// DecompressorCreator implementation
class DecompressorImpl : public fidl::WireServer<fuchsia_blobfs_internal::DecompressorCreator> {
 public:
  // Sets up a fifo to do work using the provided vmos, with the compressed vmo
  // as the src and the decompressed as the destination in all requests on the
  // fifo.
  void Create(::fuchsia_blobfs_internal::wire::DecompressorCreatorCreateRequest* request,
              CreateCompleter::Sync& completer) final;
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_COMPRESSION_DECOMPRESSOR_SANDBOX_DECOMPRESSOR_IMPL_H_
