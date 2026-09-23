// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_AUDIO_AUDIO_CODEC_AUDIO_CODEC_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_AUDIO_AUDIO_CODEC_AUDIO_CODEC_VISITOR_H_

#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

namespace audio_codec_visitor_dt {

class AudioCodecVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kCodecs[] = "codecs";

  AudioCodecVisitor();
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  std::unique_ptr<fdf_devicetree::PropertyParser> parser_;
};

}  // namespace audio_codec_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_AUDIO_AUDIO_CODEC_AUDIO_CODEC_VISITOR_H_
