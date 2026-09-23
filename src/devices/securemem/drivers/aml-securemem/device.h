// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SECUREMEM_DRIVERS_AML_SECUREMEM_DEVICE_H_
#define SRC_DEVICES_SECUREMEM_DRIVERS_AML_SECUREMEM_DEVICE_H_

#include <fidl/fuchsia.hardware.securemem/cpp/wire.h>
#include <fidl/fuchsia.hardware.sysmem/cpp/wire.h>
#include <fidl/fuchsia.hardware.tee/cpp/wire.h>
#include <lib/async_patterns/cpp/dispatcher_bound.h>
#include <lib/async_patterns/cpp/receiver.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/fpromise/result.h>
#include <lib/zx/bti.h>
#include <zircon/types.h>

#include <ddktl/device.h>
#include <ddktl/protocol/empty-protocol.h>

#include "sysmem-secure-mem-server.h"

namespace amlogic_secure_mem {

static constexpr const char* kDeviceName = "aml-securemem";

class AmlogicSecureMemDevice;

using AmlogicSecureMemDeviceBase =
    ddk::Device<AmlogicSecureMemDevice, ddk::Messageable<fuchsia_hardware_securemem::Device>::Mixin,
                ddk::Suspendable, ddk::Unbindable>;

class AmlogicSecureMemDevice : public AmlogicSecureMemDeviceBase,
                               public ddk::EmptyProtocol<ZX_PROTOCOL_SECURE_MEM> {
 public:
  static zx_status_t Create(void* ctx, zx_device_t* parent);

  zx_status_t Bind();

  void DdkSuspend(ddk::SuspendTxn txn);
  void DdkUnbind(ddk::UnbindTxn txn);
  void DdkRelease() { delete this; }

  // LLCPP interface implementations
  void GetSecureMemoryPhysicalAddress(
      GetSecureMemoryPhysicalAddressRequestView request,
      GetSecureMemoryPhysicalAddressCompleter::Sync& completer) override;

  fpromise::result<zx_paddr_t, zx_status_t> GetSecureMemoryPhysicalAddress(zx::vmo secure_mem);

 private:
  explicit AmlogicSecureMemDevice(zx_device_t* device);

  zx_status_t CreateAndServeSysmemTee();

  void SysmemSecureMemServerOnUnbound(bool is_success);

  void StartCleanServerClose(fit::closure on_unbind_callback);

  // The dispatcher that this |AmlogicSecureMemDevice| lives on.
  fdf_dispatcher_t* const fdf_dispatcher_;

  fidl::WireSyncClient<fuchsia_hardware_sysmem::Sysmem> sysmem_;
  fidl::WireSyncClient<fuchsia_hardware_tee::DeviceConnector> tee_proto_client_;

  // Note: |bti_| must be backed by a dummy IOMMU so that the physical address will be stable every
  // time a secure memory VMO is passed to be pinned.
  zx::bti bti_;

  // By destroying/shutting down the loop after the |DispatcherBound|, we know that we won't
  // trigger a synchronous call to any method calls queued via the normally-async
  // |sysmem_secure_mem_server_| below, nor any synchronous call to the
  // |SysmemSecureMemServer| constructor, nor any synchronous call to the
  // |SysmemSecureMemServer| destructor.
  async::Loop sysmem_secure_mem_server_loop_{&kAsyncLoopConfigNeverAttachToThread};

  // |sysmem_secure_mem_server_| lives on |sysmem_secure_mem_server_loop_|.
  // This shields the fdf dispatcher away from blocking TEE calls.
  async_patterns::DispatcherBound<SysmemSecureMemServer> sysmem_secure_mem_server_{
      sysmem_secure_mem_server_loop_.dispatcher()};

  bool is_clean_server_close_started_ = false;
  fit::closure on_unbind_callback_;

  // Used to receive messages from |SysmemSecureMemServer|, e.g. the server has stopped.
  async_patterns::Receiver<AmlogicSecureMemDevice> receiver_;
};

}  // namespace amlogic_secure_mem

#endif  // SRC_DEVICES_SECUREMEM_DRIVERS_AML_SECUREMEM_DEVICE_H_
