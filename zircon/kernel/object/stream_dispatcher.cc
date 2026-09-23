// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/stream_dispatcher.h"

#include <lib/user_copy/user_iovec.h>

#include <fbl/ref_ptr.h>
#include <vm/stream_size_manager.h>
#include <vm/vm_object_paged.h>

StreamDispatcher::StreamDispatcher(uint32_t options, fbl::RefPtr<VmObjectPaged> vmo,
                                   fbl::RefPtr<StreamSizeManager> stream_size_manager,
                                   zx_off_t seek)
    : Dispatcher(options) {
  DISPATCHER_VERIFY_OFFSET(StreamDispatcher, kStreamDispatcherStateOffset);
  rust_stream_dispatcher_state_init(&opaque_storage_, this, options, seek,
                                    fbl::ExportToRawPtr(&vmo),
                                    fbl::ExportToRawPtr(&stream_size_manager));
}

IMPLEMENT_DISPATCHER_RUST_STATE(StreamDispatcher, rust_stream_dispatcher_state_get_lock,
                                rust_stream_dispatcher_state_destroy)

zx_status_t StreamDispatcher::user_signal_self(uint32_t clear_mask, uint32_t set_mask) {
  return UserSignalSelfSolo(this, clear_mask, set_mask, 0);
}
