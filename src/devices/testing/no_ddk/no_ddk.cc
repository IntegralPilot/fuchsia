// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <zircon/errors.h>
#include <zircon/syscalls/log.h>
#include <zircon/types.h>

namespace no_ddk {

fx_log_severity_t kMinLogSeverity = DDK_LOG_INFO;
size_t kFakeFWSize = 0x1000;

}  // namespace no_ddk

__EXPORT
zx_status_t device_add_from_driver(zx_driver_t* drv, zx_device_t* parent, device_add_args_t* args,
                                   zx_device_t** out) {
  return ZX_OK;
}

__EXPORT
void device_async_remove(zx_device_t* device) {}

__EXPORT
void device_init_reply(zx_device_t* device, zx_status_t status,
                       const device_init_reply_args_t* args) {}

__EXPORT
void device_unbind_reply(zx_device_t* device) {}

__EXPORT void device_suspend_reply(zx_device_t* dev, zx_status_t status, uint8_t out_state) {}

__EXPORT void device_resume_reply(zx_device_t* dev, zx_status_t status, uint8_t out_power_state,
                                  uint32_t out_perf_state) {}

__EXPORT
zx_status_t device_get_protocol(const zx_device_t* device, uint32_t proto_id, void* protocol) {
  return ZX_ERR_NOT_SUPPORTED;
}

__EXPORT
zx_status_t device_get_metadata(zx_device_t* device, uint32_t type, void* buf, size_t buflen,
                                size_t* actual) {
  return ZX_ERR_NOT_SUPPORTED;
}

__EXPORT zx_status_t device_get_fragment_protocol(zx_device_t* device, const char* name,
                                                  uint32_t proto_id, void* protocol) {
  return ZX_ERR_NOT_SUPPORTED;
}

__EXPORT
zx_status_t device_get_fragment_metadata(zx_device_t* device, const char* name, uint32_t type,
                                         void* buf, size_t buflen, size_t* actual) {
  return ZX_ERR_NOT_SUPPORTED;
}

__EXPORT zx_status_t device_register_service_member(zx_device_t* dev, void* handler,
                                                    const char* service_name,
                                                    const char* instance_name,
                                                    const char* member_name) {
  return ZX_ERR_NOT_SUPPORTED;
}

__EXPORT
zx_status_t device_set_profile_by_role(zx_device_t* device, zx_handle_t thread, const char* role,
                                       size_t role_size) {
  // This is currently a no-op.
  return ZX_OK;
}

__EXPORT __WEAK zx_status_t load_firmware_from_driver(zx_driver_t* drv, zx_device_t* dev,
                                                      const char* path, zx_handle_t* fw,
                                                      size_t* size) {
  // This is currently a no-op.
  *fw = ZX_HANDLE_INVALID;
  *size = no_ddk::kFakeFWSize;
  return ZX_OK;
}

__EXPORT
zx_handle_t get_ioport_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT
zx_handle_t get_iommu_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT
zx_handle_t get_mmio_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT
zx_handle_t get_msi_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT
zx_handle_t get_irq_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT
zx_handle_t get_info_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT
zx_handle_t get_smc_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT
zx_handle_t get_power_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT zx_status_t device_connect_fragment_fidl_protocol(zx_device_t* device,
                                                           const char* fragment_name,
                                                           const char* service_name,
                                                           const char* protocol_name,
                                                           zx_handle_t request) {
  return ZX_ERR_NOT_SUPPORTED;
}

extern "C" bool driver_log_severity_enabled_internal(const zx_driver_t* drv,
                                                     fx_log_severity_t flag) {
  return flag >= no_ddk::kMinLogSeverity;
}

extern "C" void driver_logvf_internal(const zx_driver_t* drv, fx_log_severity_t flag,
                                      const char* tag, const char* file, int line, const char* msg,
                                      va_list args) {
  vfprintf(stdout, msg, args);
  putchar('\n');
}

extern "C" void driver_logf_internal(const zx_driver_t* drv, fx_log_severity_t flag,
                                     const char* tag, const char* file, int line, const char* msg,
                                     ...) {
  va_list args;
  va_start(args, msg);
  driver_logvf_internal(drv, flag, tag, file, line, msg, args);
  va_end(args);
}

__EXPORT
__WEAK zx_driver_rec __zircon_driver_rec__ = {
    .ops = {},
    .driver = {},
};
