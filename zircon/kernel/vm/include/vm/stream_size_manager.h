// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_STREAM_SIZE_MANAGER_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_STREAM_SIZE_MANAGER_H_

#include <lib/zx/result.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <fbl/recycler.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <kernel/ffi.h>

class StreamSizeManager;

__BEGIN_CDECLS

zx_status_t rust_stream_size_manager_create(uint64_t stream_size, StreamSizeManager** out_ptr);
void rust_stream_size_manager_recycle(StreamSizeManager* stream_size_manager);
uint64_t rust_stream_size_manager_get_stream_size(const StreamSizeManager* stream_size_manager);

__END_CDECLS

class StreamSizeManager : public fbl::RefCounted<StreamSizeManager>,
                          public fbl::Recyclable<StreamSizeManager> {
 public:
  // TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
  FFI_ALWAYS_INLINE static zx::result<fbl::RefPtr<StreamSizeManager>> Create(uint64_t stream_size) {
    StreamSizeManager* ptr = nullptr;
    zx_status_t status = rust_stream_size_manager_create(stream_size, &ptr);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(fbl::ImportFromRawPtr(ptr));
  }

  // TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
  FFI_ALWAYS_INLINE void fbl_recycle() { rust_stream_size_manager_recycle(this); }

  // TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
  FFI_ALWAYS_INLINE uint64_t GetStreamSize() const {
    return rust_stream_size_manager_get_stream_size(this);
  }

 private:
  StreamSizeManager() = default;
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_STREAM_SIZE_MANAGER_H_
