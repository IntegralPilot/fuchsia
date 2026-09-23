// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DDKTL_TESTS_METADATA_SERVER_TEST_METADATA_RETRIEVER_TEST_DRIVER_METADATA_RETRIEVER_TEST_DEVICE_H_
#define SRC_LIB_DDKTL_TESTS_METADATA_SERVER_TEST_METADATA_RETRIEVER_TEST_DRIVER_METADATA_RETRIEVER_TEST_DEVICE_H_

#include <fidl/fuchsia.hardware.test/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include <ddktl/device.h>

namespace ddk::test {

class MetadataRetrieverTestDevice;
using MetadataRetrieverTestDeviceType =
    ddk::Device<MetadataRetrieverTestDevice,
                ddk::Messageable<fuchsia_hardware_test::MetadataRetriever>::Mixin>;

// This driver's purpose is to retrieve metadata from its parent using `ddk::GetMetadata()` and
// `ddk::GetMetadataIfExists()`.
class MetadataRetrieverTestDevice final : public MetadataRetrieverTestDeviceType {
 public:
  static zx_status_t Bind(void* ctx, zx_device_t* parent);

  explicit MetadataRetrieverTestDevice(zx_device_t* parent)
      : MetadataRetrieverTestDeviceType(parent) {}

  // ddk::Device mixin
  void DdkRelease();

  // fuchsia.hardware.test/MetadataRetriever implementation.
  void GetMetadata(GetMetadataCompleter::Sync& completer) override;
  void GetMetadataIfExists(GetMetadataIfExistsCompleter::Sync& completer) override;
};

}  // namespace ddk::test

#endif  // SRC_LIB_DDKTL_TESTS_METADATA_SERVER_TEST_METADATA_RETRIEVER_TEST_DRIVER_METADATA_RETRIEVER_TEST_DEVICE_H_
