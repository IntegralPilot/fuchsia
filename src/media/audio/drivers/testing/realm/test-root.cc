// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/binding_driver.h>

#include <bind/fuchsia/test/audio/cpp/bind.h>
#include <bind/fuchsia/test/cpp/bind.h>
#include <ddktl/device.h>

namespace audio {

class Child : public ddk::Device<Child> {
 public:
  explicit Child(zx_device_t* parent) : ddk::Device<Child>(parent) {}

  static zx_status_t Create(zx_device_t* parent, const char* name, const char* bind_name) {
    std::unique_ptr<Child> child = std::make_unique<Child>(parent);
    zx_device_str_prop_t str_props[] = {
        {bind_fuchsia_test::TEST_CHILD, str_prop_str_val(bind_name)},
    };
    zx_status_t status = child->DdkAdd(ddk::DeviceAddArgs(name).set_str_props(str_props));
    if (status != ZX_OK) {
      return status;
    }

    child.release();
    return ZX_OK;
  }
  void DdkRelease() { delete this; }
};

class Test : public ddk::Device<Test> {
 public:
  explicit Test(zx_device_t* parent) : ddk::Device<Test>(parent) {}

  static zx_status_t Bind(void* ctx, zx_device_t* parent) {
    std::unique_ptr<Test> test = std::make_unique<Test>(parent);
    zx_status_t status =
        test->DdkAdd(ddk::DeviceAddArgs("audio_test_root").set_flags(DEVICE_ADD_NON_BINDABLE));
    if (status != ZX_OK) {
      return status;
    }

    status =
        Child::Create(test->zxdev(), "codec-parent", bind_fuchsia_test_audio::TEST_CHILD_CODEC);
    if (status != ZX_OK) {
      return status;
    }

    status =
        Child::Create(test->zxdev(), "codec2-parent", bind_fuchsia_test_audio::TEST_CHILD_CODEC2);
    if (status != ZX_OK) {
      return status;
    }

    status = Child::Create(test->zxdev(), "dai-parent", bind_fuchsia_test_audio::TEST_CHILD_DAI);
    if (status != ZX_OK) {
      return status;
    }

    test.release();
    return ZX_OK;
  }
  void DdkRelease() { delete this; }
};

static constexpr zx_driver_ops_t driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = Test::Bind;
  return ops;
}();

}  // namespace audio

ZIRCON_DRIVER(audio_test_root, audio::driver_ops, "zircon", "0.1");
