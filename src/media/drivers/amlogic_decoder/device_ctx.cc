// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "device_ctx.h"

#include <fidl/fuchsia.hardware.mediacodec/cpp/wire.h>
#include <fuchsia/io/cpp/fidl.h>
#include <lib/ddk/driver.h>
#include <lib/sync/completion.h>
#include <lib/zx/profile.h>
#include <lib/zx/time.h>
#include <zircon/threads.h>

#include <ddktl/device.h>
#include <ddktl/fidl.h>

#include "amlogic-video.h"
#include "macros.h"
#include "sdk/lib/sys/cpp/service_directory.h"

namespace amlogic_decoder {

namespace {

const char* GetRoleName(ThreadRole role) {
  switch (role) {
    case ThreadRole::kSharedFidl:
      return "fuchsia.media.drivers.amlogic-decoder.fidl";
    case ThreadRole::kParserIrq:
      return "fuchsia.media.drivers.amlogic-decoder.parser-irq";
    case ThreadRole::kVdec0Irq:
    case ThreadRole::kVdec1Irq:
      return "fuchsia.media.drivers.amlogic-decoder.vdec-irq";
    case ThreadRole::kH264MultiCore:
      return "fuchsia.media.drivers.amlogic-decoder.h264-core";
    case ThreadRole::kH264MultiStreamControl:
      return "fuchsia.media.drivers.amlogic-decoder.h264-stream-control";
    case ThreadRole::kH264MultiResource:
      return "fuchsia.media.drivers.amlogic-decoder.h264-resource";
    case ThreadRole::kVp9InputProcessing:
      return "fuchsia.media.drivers.amlogic-decoder.vp9-input-processing";
    case ThreadRole::kVp9StreamControl:
      return "fuchsia.media.drivers.amlogic-decoder.vp9-stream-control";
  }
}

}  // namespace

DeviceCtx::DeviceCtx(DriverCtx* driver, zx_device_t* parent)
    : DdkDeviceType(parent),
      driver_(driver),
      codec_admission_control_(driver->shared_fidl_loop()->dispatcher()) {
  video_ = std::make_unique<AmlogicVideo>(this);
  video_->SetMetrics(&metrics());
  device_fidl_ = std::make_unique<DeviceFidl>(this);
}

DeviceCtx::~DeviceCtx() = default;

void DeviceCtx::TeardownDeviceFidl() {
  if (!device_fidl_) {
    return;
  }
  // There are two ways to destroy a fidl::Binding safely:
  //   * Switch to FIDL thread before Unbind() or ~Binding.
  //   * async::Loop Quit() + JoinThreads() before Unbind() or ~Binding
  //
  // For now this code (if implementation needed) will choose the first option
  // by destructing DeviceFidl on the FIDL thread. The current way forces this
  // thread to wait in this destructor until the shared_fidl_thread() is done
  // processing ~DeviceFidl, which means we require that ~DeviceCtx is not
  // itself running on the shared_fidl_thread() (or we could check which thread
  // here, if we really need to).
  //
  // This code is only run when we switch between test and production drivers.
  sync_completion_t completion;
  driver_->PostToSharedFidl([this, &completion]() {
    device_fidl_ = nullptr;
    sync_completion_signal(&completion);
  });
  sync_completion_wait(&completion, ZX_TIME_INFINITE);
}

zx_status_t DeviceCtx::Bind() {
  SetThreadProfile(zx::unowned_thread(thrd_get_zx_handle(driver_->shared_fidl_thread())),
                   ThreadRole::kSharedFidl);
  zx_status_t status = DdkAdd(
      ddk::DeviceAddArgs("amlogic_video").set_inspect_vmo(driver_->diagnostics().DuplicateVmo()));
  zxlogf(INFO, "amlogic-video finished initialization with status %d", status);

  diagnostics().SetBindTime();

  return status;
}

void DeviceCtx::DdkSuspend(ddk::SuspendTxn txn) {
  if (txn.enable_wake()) {
    zxlogf(ERROR, "Suspending with wake enabled not supported.");
    txn.Reply(ZX_ERR_NOT_SUPPORTED, DEV_POWER_STATE_D0);
    return;
  }
  // Tear down everything to ensure that no driver threads are active by the time blobfs is
  // unmounted, since attempting to page in code from blobfs after that point could fail. The driver
  // framework shouldn't call into the driver after suspend has completed.
  TeardownDeviceFidl();
  video_.reset();
  driver_->Suspend();
  txn.Reply(ZX_OK, txn.requested_state());
}

void DeviceCtx::DdkUnbind(ddk::UnbindTxn txn) {
  TeardownDeviceFidl();
  txn.Reply();
}

void DeviceCtx::SetThreadProfile(zx::unowned_thread thread, ThreadRole role) const {
  const char* role_name = GetRoleName(role);
  const size_t role_size = strlen(role_name);

  const zx_status_t status =
      device_set_profile_by_role(parent(), thread->get(), role_name, role_size);
  if (status != ZX_OK) {
    LOG(WARNING, "Unable to set thread to role %s: %s", role_name, zx_status_get_string(status));
  }
}

void DeviceCtx::GetCodecFactory(GetCodecFactoryRequestView request,
                                GetCodecFactoryCompleter::Sync& completer) {
  device_fidl()->ConnectChannelBoundCodecFactory(std::move(request->request));
}

void DeviceCtx::SetAuxServiceDirectory(SetAuxServiceDirectoryRequestView request,
                                       SetAuxServiceDirectoryCompleter::Sync& completer) {
  driver_->SetAuxServiceDirectory(
      fidl::InterfaceHandle<fuchsia::io::Directory>(request->service_directory.TakeChannel()));
}

void DeviceCtx::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_mediacodec::Device> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  LOG(WARNING, "handle_unknown_method: %" PRId64, metadata.method_ordinal);
  // ~completer
}

CodecMetrics& DeviceCtx::metrics() { return driver_->metrics(); }

CodecDiagnostics& DeviceCtx::diagnostics() { return driver_->diagnostics(); }

}  // namespace amlogic_decoder
