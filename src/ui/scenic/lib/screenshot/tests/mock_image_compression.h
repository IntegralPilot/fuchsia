// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_SCREENSHOT_TESTS_MOCK_IMAGE_COMPRESSION_H_
#define SRC_UI_SCENIC_LIB_SCREENSHOT_TESTS_MOCK_IMAGE_COMPRESSION_H_

#include <fidl/fuchsia.ui.compression.internal/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

#include <gmock/gmock.h>

namespace screenshot::test {

// Mock class of ImageCompressor for API testing.
class MockImageCompression : public fidl::Server<fuchsia_ui_compression_internal::ImageCompressor> {
 public:
  MockImageCompression() = default;

  void Bind(fidl::ServerEnd<fuchsia_ui_compression_internal::ImageCompressor> server_end,
            async_dispatcher_t* dispatcher) {
    bindings_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
  }

  void EncodePng(EncodePngRequest& request, EncodePngCompleter::Sync& completer) override {
    EncodePngMock(request, completer);
  }

  MOCK_METHOD(void, EncodePngMock,
              (fuchsia_ui_compression_internal::ImageCompressorEncodePngRequest & request,
               EncodePngCompleter::Sync& completer));

 private:
  fidl::ServerBindingGroup<fuchsia_ui_compression_internal::ImageCompressor> bindings_;
};

}  // namespace screenshot::test

#endif  // SRC_UI_SCENIC_LIB_SCREENSHOT_TESTS_MOCK_IMAGE_COMPRESSION_H_
