// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.sysmem/cpp/wire_test_base.h>
#include <fidl/fuchsia.hardware.tee/cpp/wire.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdf/env.h>
#include <lib/fpromise/result.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/resource.h>
#include <zircon/limits.h>

#include <fbl/array.h>
#include <zxtest/zxtest.h>

#include "device.h"
#include "src/devices/testing/mock-ddk/mock-device.h"

class FakeSysmem : public fidl::testing::WireTestBase<fuchsia_hardware_sysmem::Sysmem> {
 public:
  void RegisterHeap(RegisterHeapRequestView request,
                    RegisterHeapCompleter::Sync& completer) override {
    // Currently, do nothing
  }

  void RegisterSecureMem(RegisterSecureMemRequestView request,
                         RegisterSecureMemCompleter::Sync& completer) override {
    // Stash the tee_connection_ so the channel can stay open long enough to avoid a potentially
    // confusing error message during the test.
    tee_connection_ = std::move(request->secure_mem_connection);
  }

  void UnregisterSecureMem(UnregisterSecureMemCompleter::Sync& completer) override {
    // Currently, do nothing
    completer.ReplySuccess();
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void Connect(fidl::ServerEnd<fuchsia_hardware_sysmem::Sysmem> request) {
    sysmem_bindings_.AddBinding(async_get_default_dispatcher(), std::move(request), this,
                                fidl::kIgnoreBindingClosure);
  }

 private:
  fidl::ClientEnd<fuchsia_sysmem2::SecureMem> tee_connection_;
  fidl::ServerBindingGroup<fuchsia_hardware_sysmem::Sysmem> sysmem_bindings_;
};

// We cover the code involved in supporting non-VDEC secure memory and VDEC secure memory in
// sysmem-test, so this fake doesn't really need to do much yet.
class FakeTee : public fidl::WireServer<fuchsia_hardware_tee::DeviceConnector> {
 public:
  void ConnectToApplication(ConnectToApplicationRequestView request,
                            ConnectToApplicationCompleter::Sync& completer) override {
    // Currently, do nothing
    //
    // We don't rely on the tee_app_request channel sticking around for these tests.  See
    // sysmem-test for a test that exercises the tee_app_request channel.
  }

  void ConnectToDeviceInfo(ConnectToDeviceInfoRequestView request,
                           ConnectToDeviceInfoCompleter::Sync& completer) override {}

  fuchsia_hardware_tee::Service::InstanceHandler CreateInstanceHandler() {
    return fuchsia_hardware_tee::Service::InstanceHandler(
        {.device_connector = bindings_.CreateHandler(this, async_get_default_dispatcher(),
                                                     fidl::kIgnoreBindingClosure)});
  }

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_tee::DeviceConnector> bindings_;
};

class AmlogicSecureMemTest : public zxtest::Test {
 protected:
  AmlogicSecureMemTest() {
    ASSERT_OK(incoming_loop_.StartThread("incoming"));

    // Create pdev fragment
    fdf_fake::FakePDev::Config config{.use_fake_bti = true};
    pdev_.SyncCall(&fdf_fake::FakePDev::SetConfig, std::move(config));
    auto pdev_handler =
        pdev_.SyncCall(&fdf_fake::FakePDev::GetInstanceHandler, async_patterns::PassDispatcher);
    auto pdev_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    root_->AddFidlService(fuchsia_hardware_platform_device::Service::Name,
                          std::move(pdev_endpoints.client), "pdev");

    // Create sysmem fragment
    root_->AddNsProtocol<fuchsia_hardware_sysmem::Sysmem>(
        [&](auto request) { sysmem_.SyncCall(&FakeSysmem::Connect, std::move(request)); });

    // Create tee fragment
    auto tee_handler = tee_.SyncCall(&FakeTee::CreateInstanceHandler);
    auto tee_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    root_->AddFidlService(fuchsia_hardware_tee::Service::Name, std::move(tee_endpoints.client),
                          "tee");

    outgoing_.SyncCall(
        [pdev_server = std::move(pdev_endpoints.server),
         tee_server = std::move(tee_endpoints.server), pdev_handler = std::move(pdev_handler),
         tee_handler = std::move(tee_handler)](component::OutgoingDirectory* outgoing) mutable {
          ZX_ASSERT(outgoing->Serve(std::move(pdev_server)).is_ok());
          ZX_ASSERT(outgoing->Serve(std::move(tee_server)).is_ok());

          ZX_ASSERT(
              outgoing
                  ->AddService<fuchsia_hardware_platform_device::Service>(std::move(pdev_handler))
                  .is_ok());
          ZX_ASSERT(
              outgoing->AddService<fuchsia_hardware_tee::Service>(std::move(tee_handler)).is_ok());
        });

    libsync::Completion completion;
    async::PostTask(dispatcher_->async_dispatcher(), [&]() {
      ASSERT_OK(amlogic_secure_mem::AmlogicSecureMemDevice::Create(nullptr, parent()));
      completion.Signal();
    });
    completion.Wait();
    ASSERT_EQ(root_->child_count(), 1);
    auto child = root_->GetLatestChild();
    dev_ = child->GetDeviceContext<amlogic_secure_mem::AmlogicSecureMemDevice>();
  }

  void TearDown() override {
    // For now, we use DdkSuspend(mexec) partly to cover DdkSuspend(mexec) handling, and
    // partly because it's the only way of cleaning up safely that we've implemented so far, as
    // aml-securemem doesn't yet implement DdkUnbind() - and arguably it doesn't really need to
    // given what aml-securemem is.

    async::PostTask(dispatcher_->async_dispatcher(), [&]() {
      dev()->zxdev()->SuspendNewOp(DEV_POWER_STATE_D3COLD, false, DEVICE_SUSPEND_REASON_MEXEC);
    });

    ASSERT_OK(dev()->zxdev()->WaitUntilSuspendReplyCalled());

    // Destroy the driver object in the dispatcher context.
    libsync::Completion destroy_completion;
    async::PostTask(dispatcher_->async_dispatcher(), [&]() {
      EXPECT_EQ(1, root_->child_count());
      dev()->zxdev()->ReleaseOp();
      mock_ddk::ReleaseFlaggedDevices(root_.get());
      destroy_completion.Signal();
    });
    destroy_completion.Wait();
  }

  zx_device_t* parent() { return root_.get(); }

  amlogic_secure_mem::AmlogicSecureMemDevice* dev() { return dev_; }

 private:
  fdf_testing::DriverRuntime* runtime() { return fdf_testing::DriverRuntime::GetInstance(); }

  async::Loop incoming_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  std::shared_ptr<MockDevice> root_{MockDevice::FakeRootParent()};
  async_patterns::TestDispatcherBound<fdf_fake::FakePDev> pdev_{incoming_loop_.dispatcher(),
                                                                std::in_place};
  async_patterns::TestDispatcherBound<FakeSysmem> sysmem_{incoming_loop_.dispatcher(),
                                                          std::in_place};
  async_patterns::TestDispatcherBound<FakeTee> tee_{incoming_loop_.dispatcher(), std::in_place};
  async_patterns::TestDispatcherBound<component::OutgoingDirectory> outgoing_{
      incoming_loop_.dispatcher(), std::in_place, async_patterns::PassDispatcher};
  amlogic_secure_mem::AmlogicSecureMemDevice* dev_;
  fdf::UnownedSynchronizedDispatcher dispatcher_{runtime()->StartBackgroundDispatcher()};

  libsync::Completion shutdown_completion_;
};

TEST_F(AmlogicSecureMemTest, GetSecureMemoryPhysicalAddressBadVmo) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  ASSERT_TRUE(dev()->GetSecureMemoryPhysicalAddress(std::move(vmo)).is_error());
}
