// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_LZ4_COMPRESSOR_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_LZ4_COMPRESSOR_H_

#include <fbl/array.h>
#include <kernel/mutex.h>
#include <lz4/lz4.h>
#include <vm/compression.h>

class VmLz4Compressor final : public VmCompressionStrategy {
 public:
  // Returns nullptr on allocation or other failure.
  static fbl::RefPtr<VmLz4Compressor> Create();
  ~VmLz4Compressor() override = default;
  DISALLOW_COPY_ASSIGN_AND_MOVE(VmLz4Compressor);

  CompressResult Compress(const void* src, void* dst, size_t dst_limit) override;
  void Decompress(const void* src, size_t src_len, void* dst) override;
  void Dump() const override;

 private:
  // Constructor is private to ensure that Init() gets called to initialize state.
  VmLz4Compressor(int lz4_acceleration) : acceleration_(lz4_acceleration) {}

  DECLARE_MUTEX(VmLz4Compressor) compress_lock_;

  // Internal helper that initializes the stream_ and compressed_zero_. If this returns false the
  // object should be destroyed and not used.
  bool Init();

  // The acceleration factor that is directly passed into the lz4 compress methods. Is set by the
  // constructor and has no default value. Making this non-const would require a different form of
  // zero detection.
  const int acceleration_;

  // The compressed representation, using the |acceleration_| value, of a page of zeroes. Used to
  // determine if any |Compress| request is actually just a zero page by memcmp'ing with the result.
  fbl::Array<char> compressed_zero_;

  // The LZ4_stream_t used to hold compression state.
  LZ4_stream_t stream_ TA_GUARDED(compress_lock_);
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_LZ4_COMPRESSOR_H_
