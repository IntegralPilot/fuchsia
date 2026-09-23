// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/function.h>

#include <optional>

#include <ddktl/device.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>

class TestLifecycleDriverChild;
using DeviceType = ddk::Device<TestLifecycleDriverChild, ddk::Initializable, ddk::Unbindable>;

class TestLifecycleDriverChild : public DeviceType,
                                 public fbl::RefCounted<TestLifecycleDriverChild> {
 public:
  static zx_status_t Create(zx_device_t* parent, bool complete_init, zx_status_t init_status,
                            fbl::RefPtr<TestLifecycleDriverChild>* out_device);

  explicit TestLifecycleDriverChild(zx_device_t* parent, bool complete_init,
                                    zx_status_t init_status)
      : DeviceType(parent), complete_init_(complete_init), init_status_(init_status) {}
  ~TestLifecycleDriverChild() {}

  void DdkInit(ddk::InitTxn txn);
  void DdkUnbind(ddk::UnbindTxn txn);
  void DdkRelease();
  void AsyncRemove(fit::function<void()> callback);
  void CompleteUnbind();

  zx_status_t CompleteInit();

 private:
  // Whether we should immediately reply to the init hook.
  bool complete_init_ = false;
  bool replied_to_init_ = false;
  bool async_remove_ = false;
  // The status passed to device_init_reply.
  zx_status_t init_status_ = ZX_OK;
  std::optional<fit::function<void()>> unbind_callback_;
  std::optional<ddk::InitTxn> init_txn_;
  std::optional<ddk::UnbindTxn> unbind_txn_;
};
