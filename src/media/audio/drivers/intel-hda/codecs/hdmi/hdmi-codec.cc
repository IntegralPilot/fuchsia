// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "hdmi-codec.h"

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/platform-defs.h>

#include <fbl/auto_lock.h>

#include "hdmi-stream.h"

namespace audio {
namespace intel_hda {
namespace codecs {

zx_status_t HdmiCodec::Create(void* ctx, zx_device_t* parent) {
  fbl::RefPtr<HdmiCodec> codec = fbl::AdoptRef(new HdmiCodec);
  ZX_DEBUG_ASSERT(codec != nullptr);
  return codec->Init(parent);
}

zx_status_t HdmiCodec::Init(zx_device_t* codec_dev) {
  zx_status_t res = Bind(codec_dev, "hdmi-codec").status_value();
  if (res != ZX_OK) {
    return res;
  }

  res = Start();
  if (res != ZX_OK) {
    Shutdown();
    return res;
  }

  return ZX_OK;
}

zx_status_t HdmiCodec::Start() {
  zx_status_t res;

  waiting_for_vendor_id_ = true;

  // Fetch the vendor ID from the root node.
  res = SendCodecCommand(0u, GET_PARAM(CodecParam::VENDOR_ID), false);
  if (res != ZX_OK) {
    zxlogf(ERROR, "Failed to send get vendor id command (res %d)", res);
  }
  return res;
}

zx_status_t HdmiCodec::GetImplementationId() {
  zx_status_t res;

  waiting_for_impl_id_ = true;

  // Fetch the implementation ID register from the main audio function group.
  res = SendCodecCommand(1u, GET_IMPLEMENTATION_ID, false);
  if (res != ZX_OK) {
    zxlogf(ERROR, "Failed to send get impl id command (res %d)", res);
  }
  return res;
}

zx_status_t HdmiCodec::ProcessSolicitedResponse(const CodecResponse& resp) {
  if (waiting_for_vendor_id_) {
    waiting_for_vendor_id_ = false;
    uint16_t device_id = resp.data & 0xFFFFu;
    zxlogf(INFO, "Device ID %04x", device_id);
    is_tgl_ = (device_id == 0x2812);  // Intel TigerLake

    // TigerLake support is not complete and results in exposing an
    // audio channel which is non-functional.  For this reason, it is
    // disabled here with a log message; the support can be enabled
    // for further development on TigerLake.
    constexpr bool kEnableTigerLake = false;
    if (!kEnableTigerLake && is_tgl_) {
      zxlogf(ERROR, "TigerLake support not enabled");
      return ZX_ERR_NOT_SUPPORTED;
    }

    return GetImplementationId();
  }

  if (!waiting_for_impl_id_) {
    zxlogf(INFO, "Unexpected solicited codec response %08x", resp.data);
    return ZX_ERR_BAD_STATE;
  }
  waiting_for_impl_id_ = false;
  zxlogf(INFO, "Implementation ID %08x", resp.data);
  return Setup();
}

zx_status_t HdmiCodec::Setup() {
  if (is_tgl_) {
    static const CommandListEntry TGL_HDMI[] = {
        // Enable all nodes and converters and DP 1.2 functionality.
        {2u, SP_SET_VERB<0x81>(0x03)},
    };

    zx_status_t ires = RunCommandList(TGL_HDMI, std::size(TGL_HDMI));
    if (ires != ZX_OK) {
      zxlogf(ERROR, "Failed to send startup command (res %d)", ires);
      return ires;
    }

    // Just to be safe...
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
  }

  static const CommandListEntry START_CMDS[] = {
      // Start powering down all nodes.
      {1u, SET_POWER_STATE(HDA_PS_D3HOT)},
      {2u, SET_POWER_STATE(HDA_PS_D3HOT)},
      {3u, SET_POWER_STATE(HDA_PS_D3HOT)},
      // Power up the top level Audio Function group only.
      {1u, SET_POWER_STATE(HDA_PS_D0)},
  };

  zx_status_t res = RunCommandList(START_CMDS, std::size(START_CMDS));
  if (res != ZX_OK) {
    zxlogf(ERROR, "Failed to send startup command (res %d)", res);
    return res;
  }

  // Create and publish the stream we will use.
  // pc_nid 6 below is the HDMI 2 connector on NUC11TNHv5
  static const StreamProperties STREAMS[] = {
      {
          .stream_id = 1,
          .afg_nid = 1,
          .conv_nid = static_cast<uint16_t>(is_tgl_ ? 3 : 2),
          .pc_nid = static_cast<uint16_t>(is_tgl_ ? 6 : 3),
          .default_conv_gain = 0.f,
          .default_pc_gain = 0.0f,
      },
  };

  res = CreateAndStartStreams(STREAMS, std::size(STREAMS));
  if (res != ZX_OK) {
    zxlogf(ERROR, "Failed to create and publish HDMI streams (res %d)", res);
    return res;
  }

  return ZX_OK;
}

zx_status_t HdmiCodec::RunCommandList(const CommandListEntry* cmds, size_t cmd_count) {
  zx_status_t res;

  if (cmds == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }

  for (size_t i = 0; i < cmd_count; ++i) {
    const auto& cmd = cmds[i];
    zxlogf(DEBUG, "SEND: nid %2hu verb 0x%05x", cmd.nid, cmd.verb.val);
    res = SendCodecCommand(cmd.nid, cmd.verb, true);
    if (res != ZX_OK) {
      zxlogf(ERROR, "Failed to send codec command %zu/%zu (nid %hu verb 0x%05x) (res %d)", i + 1,
             cmd_count, cmd.nid, cmd.verb.val, res);
      return res;
    }
  }

  return ZX_OK;
}

zx_status_t HdmiCodec::CreateAndStartStreams(const StreamProperties* streams, size_t stream_cnt) {
  zx_status_t res;

  if (streams == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }

  for (size_t i = 0; i < stream_cnt; ++i) {
    const auto& stream_def = streams[i];
    auto stream = fbl::AdoptRef(new HdmiStream(stream_def));

    res = ActivateStream(stream);
    if (res != ZX_OK) {
      zxlogf(ERROR, "Failed to activate stream id #%u (res %d)!", stream_def.stream_id, res);
      return res;
    }
  }

  return ZX_OK;
}

static constexpr zx_driver_ops_t driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = HdmiCodec::Create;
  return ops;
}();

}  // namespace codecs
}  // namespace intel_hda
}  // namespace audio

ZIRCON_DRIVER(hdmi_ihda_codec, audio::intel_hda::codecs::driver_ops, "zircon", "0.1");
