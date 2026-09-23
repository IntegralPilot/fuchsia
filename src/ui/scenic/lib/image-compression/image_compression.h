// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_IMAGE_COMPRESSION_IMAGE_COMPRESSION_H_
#define SRC_UI_SCENIC_LIB_IMAGE_COMPRESSION_IMAGE_COMPRESSION_H_

#include <fidl/fuchsia.ui.compression.internal/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/syslog/cpp/macros.h>

namespace image_compression {

// This class implements the ImageCompressor protocol.
class ImageCompression : public fidl::Server<fuchsia_ui_compression_internal::ImageCompressor> {
 public:
  // |fidl::Server<fuchsia_ui_compression_internal::ImageCompressor>|
  void EncodePng(EncodePngRequest& request, EncodePngCompleter::Sync& completer) override;

  fidl::ProtocolHandler<fuchsia_ui_compression_internal::ImageCompressor> GetHandler(
      async_dispatcher_t* dispatcher) {
    return bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure);
  }

  void Connect(fidl::ServerEnd<fuchsia_ui_compression_internal::ImageCompressor> request,
               async_dispatcher_t* dispatcher) {
    bindings_.AddBinding(dispatcher, std::move(request), this, fidl::kIgnoreBindingClosure);
  }

  void OnFidlClosed(fidl::UnbindInfo info) {}

 private:
  fidl::ServerBindingGroup<fuchsia_ui_compression_internal::ImageCompressor> bindings_;
};

}  // namespace image_compression

#endif  // SRC_UI_SCENIC_LIB_IMAGE_COMPRESSION_IMAGE_COMPRESSION_H_
