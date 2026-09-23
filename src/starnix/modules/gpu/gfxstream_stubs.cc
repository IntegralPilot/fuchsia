// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "third_party/gfxstream/src/host/include/gfxstream/virtio-gpu-gfxstream-renderer.h"

extern "C" {

VG_EXPORT int stream_renderer_init(struct stream_renderer_param* stream_renderer_params,
                                   uint64_t num_params) {
  return 0;
}

VG_EXPORT void stream_renderer_teardown(void) {}

VG_EXPORT int stream_renderer_resource_create(struct stream_renderer_resource_create_args* args,
                                              struct iovec* iov, uint32_t num_iovs) {
  return 0;
}

VG_EXPORT void stream_renderer_resource_unref(uint32_t res_handle) {}

VG_EXPORT void stream_renderer_context_destroy(uint32_t handle) {}

VG_EXPORT int stream_renderer_submit_cmd(struct stream_renderer_command* cmd) { return 0; }

VG_EXPORT int stream_renderer_transfer_read_iov(uint32_t handle, uint32_t ctx_id, uint32_t level,
                                                uint32_t stride, uint32_t layer_stride,
                                                struct stream_renderer_box* box, uint64_t offset,
                                                struct iovec* iov, int iovec_cnt) {
  return 0;
}

VG_EXPORT int stream_renderer_transfer_write_iov(uint32_t handle, uint32_t ctx_id, int level,
                                                 uint32_t stride, uint32_t layer_stride,
                                                 struct stream_renderer_box* box, uint64_t offset,
                                                 struct iovec* iovec, unsigned int iovec_cnt) {
  return 0;
}

VG_EXPORT void stream_renderer_get_cap_set(uint32_t set, uint32_t* max_ver, uint32_t* max_size) {}

VG_EXPORT void stream_renderer_fill_caps(uint32_t set, uint32_t version, void* caps) {}

VG_EXPORT int stream_renderer_resource_attach_iov(int res_handle, struct iovec* iov, int num_iovs) {
  return 0;
}

VG_EXPORT void stream_renderer_resource_detach_iov(int res_handle, struct iovec** iov,
                                                   int* num_iovs) {}

VG_EXPORT void stream_renderer_ctx_attach_resource(int ctx_id, int res_handle) {}

VG_EXPORT void stream_renderer_ctx_detach_resource(int ctx_id, int res_handle) {}

VG_EXPORT int stream_renderer_create_blob(uint32_t ctx_id, uint32_t res_handle,
                                          const struct stream_renderer_create_blob* create_blob,
                                          const struct iovec* iovecs, uint32_t num_iovs,
                                          const struct stream_renderer_handle* handle) {
  return 0;
}

VG_EXPORT int stream_renderer_export_blob(uint32_t res_handle,
                                          struct stream_renderer_handle* handle) {
  return 0;
}

VG_EXPORT int stream_renderer_resource_map(uint32_t res_handle, void** hvaOut, uint64_t* sizeOut) {
  return 0;
}

VG_EXPORT int stream_renderer_resource_unmap(uint32_t res_handle) { return 0; }

VG_EXPORT int stream_renderer_context_create(uint32_t ctx_id, uint32_t nlen, const char* name,
                                             uint32_t context_init) {
  return 0;
}

VG_EXPORT int stream_renderer_create_fence(const struct stream_renderer_fence* fence) { return 0; }

VG_EXPORT int stream_renderer_resource_map_info(uint32_t res_handle, uint32_t* map_info) {
  return 0;
}

VG_EXPORT int stream_renderer_vulkan_info(uint32_t res_handle,
                                          struct stream_renderer_vulkan_info* vulkan_info) {
  return 0;
}

// Unstable
VG_EXPORT void stream_renderer_flush(uint32_t res_handle) {}

}  // extern "C"
