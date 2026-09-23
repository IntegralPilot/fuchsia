// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_DRIVERS_AMLOGIC_DECODER_DEVICE_CTX_H_
#define SRC_MEDIA_DRIVERS_AMLOGIC_DECODER_DEVICE_CTX_H_

#include <fidl/fuchsia.hardware.mediacodec/cpp/wire.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/media/codec_impl/codec_diagnostics.h>
#include <lib/media/codec_impl/codec_metrics.h>
#include <lib/zx/thread.h>

#include <ddktl/device.h>
#include <ddktl/protocol/empty-protocol.h>

#include "amlogic-video.h"
#include "device_fidl.h"
#include "driver_ctx.h"
#include "thread_role.h"

namespace amlogic_decoder {

class DeviceCtx;

}  // namespace amlogic_decoder

namespace fidl {

// This is temporary to enable a soft transition of fuchsia_hardware_mediacodec::Device from closed
// to open.
template <typename Protocol>
struct UnknownMethodMetadata<
    Protocol, std::enable_if_t<std::is_same_v<Protocol, fuchsia_hardware_mediacodec::Device> &&
                                   Protocol::kOpenness == ::fidl::internal::Openness::kClosed,
                               void>> {
  uint64_t method_ordinal{};
  UnknownMethodType unknown_method_type{};
};

}  // namespace fidl

namespace amlogic_decoder {

using DdkDeviceType =
    ddk::Device<DeviceCtx, ddk::Messageable<fuchsia_hardware_mediacodec::Device>::Mixin,
                ddk::Suspendable, ddk::Unbindable>;

// A pointer to an instance of this class is the per-device "ctx".  The purpose
// of this class is to provide a place for device-lifetime stuff to be rooted,
// without itself being any particular aspect of the driver.
//
// TODO(dustingreen): If this device's release() can get called, we'll want to
// sequence the shutdown more carefully/explicitly.  Just destructing an
// instance of this class isn't tested to actually shut down cleanly (yet).
class DeviceCtx : public DdkDeviceType,
                  public ddk::EmptyProtocol<ZX_PROTOCOL_MEDIA_CODEC>,
                  public AmlogicVideo::Owner {
 public:
  DeviceCtx(DriverCtx* driver, zx_device_t* parent);
  ~DeviceCtx();

  zx_status_t Bind();

  DriverCtx* driver() { return driver_; }

  AmlogicVideo* video() { return video_.get(); }

  DeviceFidl* device_fidl() { return device_fidl_.get(); }

  CodecAdmissionControl* codec_admission_control() { return &codec_admission_control_; }

  CodecMetrics& metrics();

  CodecDiagnostics& diagnostics();

  void DdkRelease() { delete this; }
  void DdkSuspend(ddk::SuspendTxn txn);
  void DdkUnbind(ddk::UnbindTxn txn);

  // AmlogicVideo::Owner implementation
  void SetThreadProfile(zx::unowned_thread thread, ThreadRole role) const override;

  // fuchsia_hardware_mediacodec::Device impl.
  void GetCodecFactory(GetCodecFactoryRequestView request,
                       GetCodecFactoryCompleter::Sync& completer) override;
  void SetAuxServiceDirectory(SetAuxServiceDirectoryRequestView request,
                              SetAuxServiceDirectoryCompleter::Sync& completer) override;
  // Switch from "virtual" to "override" after fuchsia.hardware.mediacodec.Device switches to open.
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Winconsistent-missing-override"
#pragma GCC diagnostic ignored "-Wno-suggest-override"
  virtual void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_mediacodec::Device> metadata,
      fidl::UnknownMethodCompleter::Sync& completer);
#pragma GCC diagnostic pop

 private:
  void TeardownDeviceFidl();
  DriverCtx* driver_ = nullptr;

  //
  // Generic driver/device interfacing:
  //

  // Specific device driving:
  std::unique_ptr<AmlogicVideo> video_;

  // FIDL interface handling:
  std::unique_ptr<DeviceFidl> device_fidl_;

  // Codec admission control:
  CodecAdmissionControl codec_admission_control_;
};

}  // namespace amlogic_decoder

#endif  // SRC_MEDIA_DRIVERS_AMLOGIC_DECODER_DEVICE_CTX_H_
