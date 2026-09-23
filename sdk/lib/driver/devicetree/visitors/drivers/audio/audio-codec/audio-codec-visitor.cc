// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "audio-codec-visitor.h"

#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/cpp/bind.h>

namespace audio_codec_visitor_dt {

AudioCodecVisitor::AudioCodecVisitor() {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(
      std::make_unique<fdf_devicetree::ReferenceProperty>(kCodecs, 0u, /* required */ false));
  parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

zx::result<> AudioCodecVisitor::Visit(fdf_devicetree::Node& node,
                                      const devicetree::PropertyDecoder& decoder) {
  zx::result parser_output = parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Audio codec visitor parse failed for node '{}': {}", node.name(), parser_output);
    return parser_output.take_error();
  }

  auto codecs = parser_output->Get<fdf_devicetree::References>(kCodecs);
  if (!codecs || codecs->empty()) {
    return zx::ok();
  }

  for (uint32_t index = 0; index < codecs->size(); index++) {
    auto& reference = (*codecs)[index];
    if (!reference.reference_node()) {
      fdf::error("Node '{}' has invalid codec reference at index {}.", node.name(), index);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    uint32_t codec_instance = index + 1;

    // 1. Supply the codec node with a unique instance ID as private metadata.
    fuchsia_hardware_platform_bus::Metadata private_metadata{{
        .id = std::to_string(DEVICE_METADATA_PRIVATE),
        .data = std::vector<uint8_t>(
            reinterpret_cast<const uint8_t*>(&codec_instance),
            reinterpret_cast<const uint8_t*>(&codec_instance) + sizeof(codec_instance)),
    }};
    reference.reference_node().GetNode()->AddMetadata(std::move(private_metadata));

    // 2. Link the codec to the audio controller node by adding a parent spec.
    std::vector bind_rules = {
        fdf::MakeAcceptBindRule(bind_fuchsia::ID, codec_instance),
        fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.audio.CodecService"),
    };
    std::vector bind_properties = {
        fdf::MakeProperty2(bind_fuchsia::ID, codec_instance),
        fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.audio.CodecService"),
    };

    auto codec_spec = fuchsia_driver_framework::ParentSpec2{{bind_rules, bind_properties}};
    node.AddNodeSpec(codec_spec);

    fdf::info("Audio codec visitor: Linked codec '{}' to controller '{}' as codec-instance {}",
              reference.reference_node().name(), node.name(), codec_instance);
  }

  return zx::ok();
}

}  // namespace audio_codec_visitor_dt

REGISTER_DEVICETREE_VISITOR(audio_codec_visitor_dt::AudioCodecVisitor);
