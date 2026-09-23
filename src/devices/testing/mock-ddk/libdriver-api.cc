// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/debug.h>
#include <lib/syslog/cpp/log_message_impl.h>
#include <lib/syslog/cpp/log_settings.h>
#include <stdarg.h>
#include <stdlib.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <mutex>
#include <utility>

#include "src/devices/testing/mock-ddk/mock-device.h"

namespace {

// Drivers can call into the ddk from separate threads.
// This mutex ensures all calls into the mock-ddk are synchronized.
std::mutex libdriver_lock;

}  // namespace

namespace mock_ddk {

__EXPORT void SetMinLogSeverity(fx_log_severity_t severity) {
  fuchsia_logging::LogSettingsBuilder()
      .WithMinLogSeverity(static_cast<fuchsia_logging::RawLogSeverity>(severity))
      .BuildAndInitialize();
}

}  // namespace mock_ddk

// Checks to possibly keep:
// InitReply:
//   If the init fails, the device should be automatically unbound and removed.
// AsyncRemove
//   We should not call unbind until the init hook has been replied to.

__EXPORT
zx_status_t device_add_from_driver(zx_driver_t* drv, zx_device_t* parent, device_add_args_t* args,
                                   zx_device_t** out) {
  std::lock_guard guard(libdriver_lock);
  return MockDevice::Create(args, parent, out);
}

// These calls are not supported by root parent devices:
__EXPORT
void device_async_remove(zx_device_t* device) {
  std::lock_guard guard(libdriver_lock);
  if (!device) {
    zxlogf(ERROR, "Error: %s passed a null device\n", __func__);
    return;
  }
  if (device->IsRootParent()) {
    zxlogf(ERROR, "Error: Mock parent device does not support %s\n", __func__);
    return;
  }
  device->RecordAsyncRemove(ZX_OK);
}

__EXPORT
void device_init_reply(zx_device_t* device, zx_status_t status,
                       const device_init_reply_args_t* args) {
  std::lock_guard guard(libdriver_lock);
  if (!device) {
    zxlogf(ERROR, "Error: %s passed a null device\n", __func__);
    return;
  }
  if (device->IsRootParent()) {
    zxlogf(ERROR, "Error: Mock parent device does not support %s\n", __func__);
    return;
  }
  device->RecordInitReply(status);
}

__EXPORT
void device_unbind_reply(zx_device_t* device) {
  std::lock_guard guard(libdriver_lock);
  if (!device) {
    zxlogf(ERROR, "Error: %s passed a null device\n", __func__);
    return;
  }
  if (device->IsRootParent()) {
    zxlogf(ERROR, "Error: Mock parent device does not support %s\n", __func__);
    return;
  }
  device->RecordUnbindReply(ZX_OK);
}

__EXPORT void device_suspend_reply(zx_device_t* device, zx_status_t status, uint8_t out_state) {
  std::lock_guard guard(libdriver_lock);
  if (!device) {
    zxlogf(ERROR, "Error: %s passed a null device\n", __func__);
    return;
  }
  if (device->IsRootParent()) {
    zxlogf(ERROR, "Error: Mock parent device does not support %s\n", __func__);
    return;
  }
  device->RecordSuspendReply(status);
}

// These functions TODO(will be) supported by devices created as root parents:
__EXPORT
zx_status_t device_get_protocol(const zx_device_t* device, uint32_t proto_id, void* protocol) {
  std::lock_guard guard(libdriver_lock);
  if (!device) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  return device->GetProtocol(proto_id, protocol);
}

__EXPORT zx_status_t device_get_config_vmo(zx_device_t* device, zx_handle_t* config_vmo) {
  std::lock_guard guard(libdriver_lock);
  if (device == nullptr || config_vmo == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  *config_vmo = device->GetConfigVmo().release();
  return ZX_OK;
}

__EXPORT
zx_status_t device_get_metadata(zx_device_t* device, uint32_t type, void* buf, size_t buflen,
                                size_t* actual) {
  std::lock_guard guard(libdriver_lock);
  return device->GetMetadata(type, buf, buflen, actual);
}

__EXPORT zx_status_t device_get_fragment_protocol(zx_device_t* device, const char* name,
                                                  uint32_t proto_id, void* protocol) {
  std::lock_guard guard(libdriver_lock);
  if (!device) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  return device->GetProtocol(proto_id, protocol, name);
}

__EXPORT
zx_status_t device_get_fragment_metadata(zx_device_t* device, const char* name, uint32_t type,
                                         void* buf, size_t buflen, size_t* actual) {
  // The libdriver_lock will be locked in `device_get_metadata`.
  return device_get_metadata(device, type, buf, buflen, actual);
}

__EXPORT zx_status_t device_register_service_member(zx_device_t* dev, void* handler,
                                                    const char* service_name,
                                                    const char* instance_name,
                                                    const char* member_name) {
  // This call is currently unsupported in the MockDdk, so we just drop the handler on the floor.
  // As this is a unit-testing library, consider accessing the class directly and managing the
  // bindings in the test.
  return ZX_OK;
}

__EXPORT zx_status_t device_connect_fragment_fidl_protocol(zx_device_t* device,
                                                           const char* fragment_name,
                                                           const char* service_name,
                                                           const char* protocol_name,
                                                           zx_handle_t request) {
  std::lock_guard guard(libdriver_lock);
  if (!device) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  return device->ConnectToFidlProtocol(service_name, protocol_name, zx::channel(request),
                                       fragment_name);
}

__EXPORT zx_status_t device_connect_fragment_runtime_protocol(zx_device_t* device,
                                                              const char* fragment_name,
                                                              const char* service_name,
                                                              const char* protocol_name,
                                                              fdf_handle_t request) {
  std::lock_guard guard(libdriver_lock);
  if (!device) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  return device->ConnectToRuntimeProtocol(service_name, protocol_name, fdf::Channel(request),
                                          fragment_name);
}

__EXPORT zx_status_t device_connect_ns_protocol(zx_device_t* device, const char* protocol_name,
                                                zx_handle_t request) {
  std::lock_guard guard(libdriver_lock);
  if (!device) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  return device->ConnectToNsProtocol(protocol_name, zx::channel(request));
}

// Unsupported calls:
__EXPORT
zx_status_t device_set_profile_by_role(zx_device_t* device, zx_handle_t thread, const char* role,
                                       size_t role_size) {
  // This is currently a no-op.
  return ZX_OK;
}

__EXPORT __WEAK zx_status_t load_firmware_from_driver(zx_driver_t* drv, zx_device_t* device,
                                                      const char* path, zx_handle_t* fw,
                                                      size_t* size) {
  std::lock_guard guard(libdriver_lock);
  if (!device) {
    zxlogf(ERROR, "Error: %s passed a null device\n", __func__);
    return ZX_ERR_INVALID_ARGS;
  }
  return device->LoadFirmware(path, fw, size);
}

__EXPORT
zx_handle_t get_mmio_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT
zx_handle_t get_ioport_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT
zx_handle_t get_iommu_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT
zx_handle_t get_irq_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT
zx_handle_t get_info_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT
zx_handle_t get_smc_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT
zx_handle_t get_power_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

__EXPORT
zx_handle_t get_msi_resource(zx_device_t* parent) { return ZX_HANDLE_INVALID; }

extern "C" bool driver_log_severity_enabled_internal(const zx_driver_t* drv,
                                                     fx_log_severity_t flag) {
  return static_cast<fx_log_severity_t>(fuchsia_logging::GetMinLogSeverity()) <= flag;
}

extern "C" void driver_logvf_internal(const zx_driver_t* drv, fx_log_severity_t flag,
                                      const char* tag, const char* file, int line, const char* msg,
                                      va_list args) {
  char buffer[1024];
  if (vsnprintf(buffer, sizeof(buffer), msg, args) > 0) {
    fuchsia_logging::LogMessage(static_cast<fuchsia_logging::RawLogSeverity>(flag), file, line,
                                nullptr, tag)
            .stream()
        << buffer;
  }
}

extern "C" void driver_logf_internal(const zx_driver_t* drv, fx_log_severity_t flag,
                                     const char* tag, const char* file, int line, const char* msg,
                                     ...) {
  // The libdriver_logger_lock will be locked in `driver_logvf_internal`.
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
