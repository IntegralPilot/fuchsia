// Copyright 2015 The Chromium Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_MEDIA_GPU_VP9_PICTURE_H_
#define SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_MEDIA_GPU_VP9_PICTURE_H_

#include <memory>
#include <optional>  // Fuchsia change: include optional library

#include "media/gpu/codec_picture.h"
#include "media/parsers/vp9_parser.h"
// Fuchsia change: Remove libraries in favor of "chromium_utils.h"
#include "chromium_utils.h"
#include "geometry.h"
#include "media/video/video_encode_accelerator.h"

namespace media {

class V4L2VP9Picture;
class VaapiVP9Picture;

class MEDIA_GPU_EXPORT VP9Picture : public CodecPicture {
 public:
  VP9Picture();

  VP9Picture(const VP9Picture&) = delete;
  VP9Picture& operator=(const VP9Picture&) = delete;

  // TODO(tmathmeyer) remove these and just use static casts everywhere.
// Fuchsia change: Remove AsV4L2VP9Picture and AsVaapiVP9Picture
#if 0
  virtual V4L2VP9Picture* AsV4L2VP9Picture();
  virtual VaapiVP9Picture* AsVaapiVP9Picture();
#endif

  // Create a copy of this picture (used to implement show_existing_frame).
  scoped_refptr<VP9Picture> Duplicate();

  std::unique_ptr<Vp9FrameHeader> frame_hdr;

  // Fuchsia change: use std::optional instead of absl::optional
  std::optional<Vp9Metadata> metadata_for_encoding;

 protected:
  ~VP9Picture() override;

  // Create an instance of the same class, and copy any accelerator-specific
  // fields. Used by Duplicate() which handles copying CodecPicture and
  // VP9Picture fields.
  //
  // All subclasses should override this method.
  virtual scoped_refptr<VP9Picture> CreateDuplicate();
};

}  // namespace media

#endif  // SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_MEDIA_GPU_VP9_PICTURE_H_
