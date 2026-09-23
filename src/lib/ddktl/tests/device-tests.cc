// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.examples/cpp/wire.h>

#include <memory>

#include <ddktl/device.h>
#include <zxtest/zxtest.h>

// TODO(https://fxbug.dev/42128480): The negative compilation tests are broken
// #define TEST_WILL_NOT_COMPILE 1

namespace {

class TestNone : public ddk::Device<TestNone> {
 public:
  TestNone() : ddk::Device<TestNone>(nullptr) {}

  void DdkRelease() {}
};

#define BEGIN_SUCCESS_CASE(name)                                  \
  class Test##name : public ddk::Device<Test##name, ddk::name> {  \
   public:                                                        \
    Test##name() : ddk::Device<Test##name, ddk::name>(nullptr) {} \
    void DdkRelease() {}

#define BEGIN_SUCCESS_DEPRECATED_CASE(name)                                  \
  class Test##name : public ddk::Device<Test##name, ddk_deprecated::name> {  \
   public:                                                                   \
    Test##name() : ddk::Device<Test##name, ddk_deprecated::name>(nullptr) {} \
    void DdkRelease() {}

#define END_SUCCESS_CASE \
  }                      \
  ;

BEGIN_SUCCESS_CASE(GetProtocolable)
zx_status_t DdkGetProtocol(uint32_t proto_id, void* protocol) { return ZX_OK; }
END_SUCCESS_CASE

BEGIN_SUCCESS_CASE(Initializable)
// As the txn does not contain a valid device pointer, the destructor won't throw an error
// if we don't reply.
void DdkInit(ddk::InitTxn txn) {}
END_SUCCESS_CASE

BEGIN_SUCCESS_CASE(Unbindable)
// As the txn does not contain a valid device pointer, the destructor won't throw an error
// if we don't reply.
void DdkUnbind(ddk::UnbindTxn txn) {}
END_SUCCESS_CASE

class TestMessageable;
using MessageableDevice =
    ddk::Device<TestMessageable, ddk::Messageable<fuchsia_examples::Echo>::Mixin>;
class TestMessageable : public MessageableDevice {
 public:
  TestMessageable() : MessageableDevice(nullptr) {}
  void DdkRelease() {}

  void SendString(SendStringRequestView request, SendStringCompleter::Sync& completer) override {}
  void EchoString(EchoStringRequestView request, EchoStringCompleter::Sync& completer) override {}
};

BEGIN_SUCCESS_CASE(Suspendable)
// As the txn does not contain a valid device pointer, the destructor won't throw an error
// if we don't reply.
void DdkSuspend(ddk::SuspendTxn txn) {}
END_SUCCESS_CASE

template <typename T>
static void do_test() {
  auto dev = std::make_unique<T>();
}

struct TestDispatch;
using TestDispatchType = ddk::Device<TestDispatch, ddk::GetProtocolable, ddk::Initializable,
                                     ddk::Unbindable, ddk::Suspendable>;

struct TestDispatch : public TestDispatchType {
  TestDispatch() : TestDispatchType(nullptr) {}

  // Give access to the device ops for testing
  const zx_protocol_device_t* GetDeviceOps() { return &ddk_device_proto_; }

  zx_status_t DdkGetProtocol(uint32_t proto_id, void* protcool) {
    get_protocol_called = true;
    return ZX_OK;
  }

  void DdkInit(ddk::InitTxn txn) { init_called = true; }

  void DdkUnbind(ddk::UnbindTxn txn) { unbind_called = true; }

  void DdkRelease() { release_called = true; }

  void DdkSuspend(ddk::SuspendTxn txn) { suspend_called = true; }

  bool get_protocol_called = false;
  bool init_called = false;
  bool unbind_called = false;
  bool release_called = false;
  bool suspend_called = false;
};

TEST(DdktlDevice, Dispatch) {
  auto dev = std::make_unique<TestDispatch>();

  // Since we're not adding the device to devmgr, we don't have a valid zx_device_t.
  // TODO: use a devmgr API to add a test device, and use that instead
  auto ctx = dev.get();
  auto ops = dev->GetDeviceOps();
  EXPECT_EQ(ZX_OK, ops->get_protocol(ctx, 0, nullptr), "");
  ops->init(ctx);
  ops->unbind(ctx);
  ops->release(ctx);
  ops->suspend(ctx, 2, false, 0);

  EXPECT_TRUE(dev->get_protocol_called, "");
  EXPECT_TRUE(dev->init_called, "");
  EXPECT_TRUE(dev->unbind_called, "");
  EXPECT_TRUE(dev->release_called, "");
  EXPECT_TRUE(dev->suspend_called, "");
}

#if TEST_WILL_NOT_COMPILE || 0

class TestNotReleasable : public ddk::Device<TestNotReleasable> {
 public:
  TestNotReleasable() : ddk::Device<TestNotReleasable>(nullptr) {}
};

#define DEFINE_FAIL_CASE(name)                                          \
  class TestNot##name : public ddk::Device<TestNot##name, ddk::name> {  \
   public:                                                              \
    TestNot##name() : ddk::Device<TestNot##name, ddk::name>(nullptr) {} \
    void DdkRelease() {}                                                \
  };

DEFINE_FAIL_CASE(GetProtocolable)
DEFINE_FAIL_CASE(Initializable)
DEFINE_FAIL_CASE(Unbindable)
DEFINE_FAIL_CASE(Suspendable)

class TestBadOverride : public ddk::Device<TestBadOverride> {
 public:
  TestBadOverride() : ddk::Device<TestBadOverride>(nullptr) {}
  void DdkRelease() {}
};

class TestHiddenOverride : public ddk::Device<TestHiddenOverride> {
 public:
  TestHiddenOverride() : ddk::Device<TestHiddenOverride>(nullptr) {}

 private:
  void DdkRelease() {}
};

class TestStaticOverride : public ddk::Device<TestStaticOverride> {
 public:
  TestStaticOverride() : ddk::Device<TestStaticOverride>(nullptr) {}
  static void DdkRelease() {}
};

template <typename D>
struct A {
  explicit A(zx_protocol_device_t* proto) {}
};

class TestNotAMixin : public ddk::Device<TestNotAMixin, A> {
 public:
  TestNotAMixin() : ddk::Device<TestNotAMixin, A>(nullptr) {}
  void DdkRelease() {}
};

class TestNotAllMixins;
using TestNotAllMixinsType = ddk::Device<TestNotAllMixins, A>;
class TestNotAllMixins : public TestNotAllMixinsType {
 public:
  TestNotAllMixins() : TestNotAllMixinsType(nullptr) {}
  void DdkRelease() {}
};
#endif

TEST(DdktlDevice, NoMixins) { do_test<TestNone>(); }
TEST(DdktlDevice, MixinGetProtocolable) { do_test<TestGetProtocolable>(); }
TEST(DdktlDevice, MixinInitializable) { do_test<TestInitializable>(); }
TEST(DdktlDevice, MixinUnbindable) { do_test<TestUnbindable>(); }
TEST(DdktlDevice, MixinSuspendable) { do_test<TestSuspendable>(); }

}  // namespace

#if TEST_WILL_NOT_COMPILE || 0
TEST(DdktlDevice, FailNoGetProtocol) { do_test<TestNotGetProtocolable>(); }
TEST(DdktlDevice, FailNoInitialize) { do_test<TestNotInitializable>(); }
TEST(DdktlDevice, FailNoUnbind) { do_test<TestNotUnbindable>(); }
TEST(DdktlDevice, FailNoSuspende) { do_test<TestNotSuspendable>(); }
TEST(DdktlDevice, FailBadOverride) { do_test<TestBadOverride>(); }
TEST(DdktlDevice, FailHiddenOverride) { do_test<TestHiddenOverride>(); }
TEST(DdktlDevice, FailStaticOverride) { do_test<TestStaticOverride>(); }
TEST(DdktlDevice, FailNotAMixin) { do_test<TestNotAMixin>(); }
TEST(DdktlDevice, FailNotAllMixins) { do_test<TestNotAllMixins>(); }
#endif
