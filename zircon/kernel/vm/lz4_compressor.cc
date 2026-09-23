// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/fit/defer.h>
#include <lib/page/size.h>

#include <vm/lz4_compressor.h>
#include <vm/physmap.h>
#include <vm/vm.h>

VmLz4Compressor::CompressResult VmLz4Compressor::Compress(const void *src, void *dst,
                                                          size_t dst_limit) {
  int compressed_result;
  int threshold = static_cast<int>(dst_limit);
  {
    Guard<Mutex> guard{&compress_lock_};
    compressed_result = LZ4_compress_fast_extState_fastReset(
        &stream_, static_cast<const char *>(src), static_cast<char *>(dst), kPageSize, threshold,
        acceleration_);
  }
  if (compressed_result == 0) {
    return FailTag{};
  }
  DEBUG_ASSERT(compressed_result > 0 && compressed_result <= threshold);
  size_t compressed_size = static_cast<size_t>(compressed_result);
  if (compressed_size == compressed_zero_.size()) {
    if (memcmp(dst, compressed_zero_.get(), compressed_size) == 0) {
      return ZeroTag{};
    }
  }
  return compressed_size;
}

void VmLz4Compressor::Decompress(const void *src, size_t src_len, void *dst) {
  int result = LZ4_decompress_safe(static_cast<const char *>(src), static_cast<char *>(dst),
                                   static_cast<int>(src_len), kPageSize);
  ASSERT(result == kPageSize);
}

void VmLz4Compressor::Dump() const {}

bool VmLz4Compressor::Init() {
  Guard<Mutex> guard{&compress_lock_};
  // Initialize the stream. As our stream should be exactly sized and aligned there is no reason for
  // this to return anything other than the original pointer.
  LZ4_stream_t *stream = LZ4_initStream(&stream_, sizeof(stream_));
  if (stream != &stream_) {
    return false;
  }

  // Zero page should compress quite well, so just use a small stack allocation.
  constexpr size_t kMaxZeroPageStorage = 128;
  char temp_zero_compress[kMaxZeroPageStorage];
  int compress_result = LZ4_compress_fast_extState_fastReset(
      &stream_, static_cast<const char *>(paddr_to_physmap(vm_get_zero_page_paddr())),
      temp_zero_compress, kPageSize, kMaxZeroPageStorage, acceleration_);
  if (compress_result == 0) {
    printf("ERROR: LZ4 failed to compress zero page with acceleration %d into %zu bytes\n",
           acceleration_, kMaxZeroPageStorage);
    return false;
  }
  DEBUG_ASSERT(compress_result > 0);
  size_t compressed_size = static_cast<size_t>(compress_result);
  DEBUG_ASSERT(compressed_size <= kMaxZeroPageStorage);
  // Now allocate the exact storage.
  fbl::AllocChecker ac;
  compressed_zero_ = fbl::MakeArray<char>(&ac, compressed_size);
  if (!ac.check()) {
    return false;
  }
  memcpy(compressed_zero_.get(), temp_zero_compress, compressed_size);

  return true;
}

fbl::RefPtr<VmLz4Compressor> VmLz4Compressor::Create() {
  const int acceleration = static_cast<int>(BootOptions::Get()->compression_lz4_acceleration);

  fbl::AllocChecker ac;
  fbl::RefPtr<VmLz4Compressor> lz4 =
      fbl::AdoptRef<VmLz4Compressor>(new (&ac) VmLz4Compressor(acceleration));
  if (!ac.check()) {
    return nullptr;
  }

  if (!lz4->Init()) {
    return nullptr;
  }

  return lz4;
}
