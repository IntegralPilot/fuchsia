// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.pci/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <fuchsia/hardware/pciroot/cpp/banjo.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fit/defer.h>
#include <lib/pci/hw.h>
#include <lib/zx/bti.h>
#include <lib/zx/clock.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <threads.h>
#include <zircon/errors.h>
#include <zircon/limits.h>
#include <zircon/syscalls/object.h>
#include <zircon/syscalls/port.h>

#include <cstddef>
#include <memory>

#include <ddktl/metadata_server.h>
#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/devices/pci/drivers/pci/bus.h"
#include "src/devices/pci/drivers/pci/test/fakes/fake_pciroot.h"
#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/lib/testing/predicates/status.h"

namespace pci {

class IncomingNamespace {
 public:
  using MetadataType = fuchsia_hardware_pci::BoardConfiguration;
  void Init(fidl::ServerEnd<fuchsia_io::Directory> outgoing_server) {
    ASSERT_OK(metadata_server_.Serve(outgoing_, async_get_default_dispatcher()));
    ASSERT_OK(outgoing_.Serve(std::move(outgoing_server)));
  }

  void SetBoardConfig(const fuchsia_hardware_pci::BoardConfiguration& board_config) {
    metadata_server_.SetMetadata(board_config);
  }

 private:
  ddk::MetadataServer<MetadataType> metadata_server_;
  component::OutgoingDirectory outgoing_{async_get_default_dispatcher()};
};

// Defined in device_tests.cc (linked into the same pci-unit binary): counts how
// many composite node specs the bus driver has published, via the
// device_add_composite_spec mock.
extern int g_pci_composite_spec_add_count;

// TODO(https://fxbug.dev/42075363): Migrate test to use dispatcher integration.
class PciBusTests : public ::gtest::TestLoopFixture {
 protected:
  void SetUp() final {
    pciroot_.emplace(0, 2, /*is_extended=*/true);
    parent_ = MockDevice::FakeRootParent();
    ASSERT_OK(incoming_loop_.StartThread("incoming-namespace"));
    zx::result endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
    incoming_namespace_.SyncCall(&IncomingNamespace::Init, std::move(endpoints->server));

    parent_->AddProtocol(ZX_PROTOCOL_PCIROOT, pciroot_->proto()->ops, pciroot_->proto()->ctx);
    parent_->AddFidlService(fuchsia_hardware_pci::BoardConfiguration::kSerializableName,
                            std::move(endpoints->client), "default");
  }

  void TearDown() final {
    parent_.reset();
    pciroot_.reset();
    incoming_namespace_.reset();
  }

  zx_device_t* parent() { return parent_.get(); }
  // Sets up 5 devices, including two under a bridge.
  uint32_t SetupTopology();
  // Sets up 5 devices: a multifunction device with an endpoint, a bridge, and
  // another endpoint, plus two devices behind the bridge.
  uint32_t SetupMultifunctionTopology();
  zx::interrupt AddLegacyIrqToBus(uint8_t vector);
  void AddRoutingEntryToBus(std::optional<uint8_t> p_dev, std::optional<uint8_t> p_func,
                            uint8_t dev_id, uint8_t a, uint8_t b, uint8_t c, uint8_t d);
  void SetBoardConfiguration(fuchsia_hardware_pci::BoardConfiguration board_config);
  FakePciroot& pciroot() { return pciroot_.value(); }

 private:
  std::optional<FakePciroot> pciroot_;
  std::shared_ptr<MockDevice> parent_;
  async::Loop incoming_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<IncomingNamespace> incoming_namespace_{
      incoming_loop_.dispatcher(), std::in_place};
};

uint32_t PciBusTests::SetupTopology() {
  uint8_t idx = 1;
  auto& ecam = pciroot_->ecam();
  ecam.get_device({0, 0, 0})->set_vendor_id(0x8086).set_device_id(idx++).set_header_type(
      kHeaderTypeMultiFn);
  ecam.get_device({0, 0, 1})->set_vendor_id(0x8086).set_device_id(idx++);
  ecam.get_bridge({0, 1, 0})
      ->set_vendor_id(0x8086)
      .set_device_id(idx++)
      .set_header_type(kHeaderTypePciBridge)
      .set_io_base(0x10)
      .set_io_limit(0x0FFF)
      .set_memory_base(0x1000)
      .set_memory_limit(0xFFFFFFFF)
      .set_secondary_bus_number(1);
  ecam.get_device({1, 0, 0})->set_vendor_id(0x8086).set_device_id(idx++).set_header_type(
      kHeaderTypeMultiFn);
  ecam.get_device({1, 0, 1})->set_vendor_id(0x8086).set_device_id(idx);
  return idx;
}

uint32_t PciBusTests::SetupMultifunctionTopology() {
  uint8_t idx = 1;
  auto& ecam = pciroot_->ecam();
  ecam.get_device({0, 0, 0})->set_vendor_id(0x8086).set_device_id(idx++).set_header_type(
      kHeaderTypeMultiFn);
  ecam.get_bridge({0, 0, 1})
      ->set_vendor_id(0x8086)
      .set_device_id(idx++)
      .set_header_type(kHeaderTypePciBridge | kHeaderTypeMultiFn)
      .set_io_base(0x10)
      .set_io_limit(0x0FFF)
      .set_memory_base(0x1000)
      .set_memory_limit(0xFFFFFFFF)
      .set_secondary_bus_number(1);
  ecam.get_device({0, 0, 2})->set_vendor_id(0x8086).set_device_id(idx++);
  ecam.get_device({1, 0, 0})->set_vendor_id(0x8086).set_device_id(idx++);
  ecam.get_device({1, 1, 0})->set_vendor_id(0x8086).set_device_id(idx);
  return idx;
}

zx::interrupt PciBusTests::AddLegacyIrqToBus(uint8_t vector) {
  zx::interrupt interrupt;
  ZX_ASSERT(zx::interrupt::create(*zx::unowned_resource(ZX_HANDLE_INVALID), vector,
                                  ZX_INTERRUPT_VIRTUAL, &interrupt) == ZX_OK);
  zx::interrupt bus_interrupt;
  ZX_ASSERT(interrupt.duplicate(ZX_RIGHT_SAME_RIGHTS, &bus_interrupt) == ZX_OK);
  pciroot().legacy_irqs().push_back(
      pci_legacy_irq_t{.interrupt = bus_interrupt.release(), .vector = vector});

  return interrupt;
}

void PciBusTests::AddRoutingEntryToBus(std::optional<uint8_t> p_dev, std::optional<uint8_t> p_func,
                                       uint8_t dev_id, uint8_t a, uint8_t b, uint8_t c, uint8_t d) {
  pciroot().routing_entries().push_back(pci_irq_routing_entry_t{
      .port_device_id = (p_dev) ? *p_dev : static_cast<uint8_t>(PCI_IRQ_ROUTING_NO_PARENT),
      .port_function_id = (p_func) ? *p_func : static_cast<uint8_t>(PCI_IRQ_ROUTING_NO_PARENT),
      .device_id = dev_id,
      .pins = {a, b, c, d}});
}

void PciBusTests::SetBoardConfiguration(fuchsia_hardware_pci::BoardConfiguration board_config) {
  incoming_namespace_.SyncCall(&IncomingNamespace::SetBoardConfig, board_config);
}

// An encapsulated pci::Bus to allow inspection of some internal state.
class TestBus : public pci::Bus {
 public:
  TestBus(zx_device_t* parent, const pciroot_protocol_t* pciroot, const pci_platform_info_t info,
          std::optional<fdf::MmioBuffer> ecam)
      : pci::Bus(parent, pciroot, info, std::move(ecam)) {}
  virtual ~TestBus() = default;

  using pci::Bus::HandleDeviceLegacyIrqUntriggered;
  using pci::Bus::HandleLegacyIrq;
  pci::DeviceTree& Devices() { return devices(); }

  const fuchsia_hardware_pci::BoardConfiguration& GetBoardConfiguration() { return board_config(); }

  size_t GetDeviceCount() {
    fbl::AutoLock _(devices_lock());
    return devices().size();
  }

  pci::Device* GetDevice(pci_bdf_t bdf) {
    fbl::AutoLock _(devices_lock());
    auto it = devices().find(bdf);
    return it != devices().end() ? it->second.get() : nullptr;
  }

  size_t GetSharedIrqCount() {
    fbl::AutoLock _(devices_lock());
    return shared_irqs().size();
  }

  size_t GetLegacyIrqCount() {
    fbl::AutoLock _(devices_lock());
    return legacy_irqs().size();
  }
};

// Bind tests the entire initialization path using an ECAM included via platform information.
TEST_F(PciBusTests, Bind) {
  SetupTopology();
  ASSERT_OK(pci_bus_bind(nullptr, parent()));
}

// The lifecycle test is done through Proxy configs to ensure we don't need to worry
// about ownership of the vmo the MmioBuffers would share.
TEST_F(PciBusTests, Lifecycle) {
  uint32_t dev_cnt = SetupTopology();
  auto owned_bus =
      std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(), std::nullopt);
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();
  ASSERT_EQ(bus->GetDeviceCount(), dev_cnt);
}

TEST_F(PciBusTests, BdiGetBti) {
  pciroot().ecam().get_device(pci_bdf_t{})->set_vendor_id(8086).set_device_id(8086);
  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();
  ASSERT_EQ(bus->GetDeviceCount(), 1u);

  zx::bti bti = {};
  ASSERT_EQ(bus->GetBti(nullptr, 0, &bti), ZX_ERR_INVALID_ARGS);
  ASSERT_OK(bus->GetBti(bus->GetDevice(pci_bdf_t{}), 0, &bti));

  zx_info_bti_t info = {};
  zx_info_bti_t info2 = {};
  ASSERT_OK(bti.get_info(ZX_INFO_BTI, &info, sizeof(info), nullptr, nullptr));
  ASSERT_OK(pciroot().bti().get_info(ZX_INFO_BTI, &info2, sizeof(info2), nullptr, nullptr));
  ASSERT_EQ(info.aspace_size, info2.aspace_size);
  ASSERT_EQ(info.minimum_contiguity, info2.minimum_contiguity);
  ASSERT_EQ(info.pmo_count, info2.pmo_count);
  ASSERT_EQ(info.quarantine_count, info2.quarantine_count);
}

TEST_F(PciBusTests, BdiAllocateMsi) {
  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();

  for (uint32_t cnt = 1; cnt <= 32; cnt *= 2) {
    zx::msi msi = {};
    msi_allocation_info_t info = {};
    bus->AllocateMsi(cnt, &msi, &info);
    EXPECT_EQ(info.irq_count, cnt);
    EXPECT_EQ(info.target_addr, 0xCAFEu);
    EXPECT_EQ(info.target_data, 0xC0FEu);
  }
}

TEST_F(PciBusTests, BdiGetMsiHandle) {
  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();

  zx::msi msi = {};
  msi_allocation_info_t info = {};
  ASSERT_OK(bus->AllocateMsi(2, &msi, &info));

  zx::vmo cfg_vmo;
  ASSERT_OK(zx::vmo::create(4096, 0, &cfg_vmo));
  ASSERT_OK(cfg_vmo.set_cache_policy(ZX_CACHE_POLICY_UNCACHED_DEVICE));

  zx::interrupt interrupt;
  ASSERT_OK(bus->GetMsiHandle(msi, /*options=*/0, 0, cfg_vmo, 0, &interrupt));
  EXPECT_TRUE(interrupt.is_valid());
  interrupt.reset();
}

TEST_F(PciBusTests, BdiLinkUnlinkDevice) {
  pciroot().ecam().get_device(pci_bdf_t{})->set_vendor_id(8086).set_device_id(8086);
  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();
  ASSERT_EQ(bus->GetDeviceCount(), 1u);

  auto device = bus->GetDevice(pci_bdf_t{});
  auto reffed_device = fbl::RefPtr(bus->GetDevice(pci_bdf_t{}));
  EXPECT_EQ(bus->LinkDevice(reffed_device), ZX_ERR_ALREADY_EXISTS);
  EXPECT_OK(bus->UnlinkDevice(device));
  EXPECT_EQ(bus->GetDeviceCount(), 0u);
  EXPECT_EQ(bus->UnlinkDevice(device), ZX_ERR_NOT_FOUND);

  // Insert the device back into the bus topology so the disable / unplug
  // lifecycle runs. Otherwise, the normal teardown path of Device will assert
  // that it was never disabled.
  ASSERT_OK(bus->LinkDevice(fbl::RefPtr(device)));
  ASSERT_EQ(bus->GetDeviceCount(), 1u);
}

TEST_F(PciBusTests, IrqRoutingEntries) {
  // Add |int_cnt| interrupts, but make them share vectors based on |int_mod|. This ensures that
  // we handle duplicate IRQ entries properly.
  const size_t int_cnt = 5;
  const uint32_t int_mod = 3;
  zx::interrupt interrupt = {};
  for (uint32_t i = 0; i < int_cnt; i++) {
    ASSERT_OK(zx::interrupt::create(*zx::unowned_resource(ZX_HANDLE_INVALID), i,
                                    ZX_INTERRUPT_VIRTUAL, &interrupt));
    pciroot().legacy_irqs().push_back(
        pci_legacy_irq_t{.interrupt = interrupt.get(), .vector = i % int_mod});
    // The bus will take ownership of this.
    (void)interrupt.release();
  }
  pciroot().ecam().get_device(pci_bdf_t{})->set_vendor_id(1).set_device_id(2).set_interrupt_pin(1);

  pciroot().routing_entries().push_back(
      pci_irq_routing_entry_t{.port_device_id = PCI_IRQ_ROUTING_NO_PARENT,
                              .port_function_id = PCI_IRQ_ROUTING_NO_PARENT,
                              .device_id = 0,
                              .pins = {1, 2, 3, 4}});

  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();
  ASSERT_EQ(int_mod, bus->GetSharedIrqCount());
}

TEST_F(PciBusTests, LegacyIrqSignalTest) {
  // Establish the IRQ in the Pciroot implementation so that the bus will configure our device to
  // use it if the device id is 0x1 and it uses pin B.
  uint32_t vector = 0xA;
  zx::interrupt interrupt = AddLegacyIrqToBus(vector);
  AddRoutingEntryToBus(/*p_dev=*/std::nullopt, /*p_func=*/std::nullopt, /*dev_id=*/0,
                       /*a=*/vector,
                       /*b=*/vector, /*c=*/0, /*d=*/0);
  // Have the routing table target device 0, pin B. This is configured in
  // SetupTopology for the device itself.
  SetupTopology();
  // These devices need interrupt pins mapped before Bus scans the topology.
  pciroot().ecam().get_device({0, 0, 0})->set_interrupt_pin(0x1);
  pciroot().ecam().get_device({0, 0, 1})->set_interrupt_pin(0x2);
  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();
  ASSERT_EQ(1u, bus->GetSharedIrqCount());

  zx::interrupt dev_interrupt[2];
  // Configure both devices and map their driver facing interrupts. They have
  // different pins, but the pins are mapped to the same vector.
  for (uint8_t i = 0; i < 2; i++) {
    auto* bus_device = bus->GetDevice({0, 0, i});
    ASSERT_OK(bus_device->SetIrqMode(fuchsia_hardware_pci::InterruptMode::kLegacy, 1u));
    // Map the interrupt the same way a driver would.
    auto result = bus->GetDevice({0, 0, i})->MapInterrupt(0);
    ASSERT_TRUE(result.is_ok());
    dev_interrupt[i] = std::move(result.value());
  }

  // Bind device 00:00.0's interrupt to a port so we can "peek" at the interrupt
  // status via a port wait.
  zx::port port;
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));
  ASSERT_OK(dev_interrupt[0].bind(port, 1, ZX_INTERRUPT_BIND));

  // Here we simulate triggering the hardware vector and track it all the way to
  // the interrupt event a downstream driver bound to this device would get.
  // Timestamps of the original vector must match.
  zx::time_boot receive_time;
  zx::time_boot trigger_time = zx::clock::get_boot();
  pciroot().ecam().get_device({0, 0, 1})->set_status(kStatusInterrupt);
  ASSERT_OK(interrupt.trigger(0, trigger_time));

  // Process the interrupt event on the DFv1 async dispatcher.
  mock_ddk::GetDriverRuntime()->RunUntilIdle();

  // Only the device at 00:00.1 should trigger because 00:00.0 does not have the interrupt status
  // bit set in its config space. The interrupt time the driver receives must match the time the
  // interrupt dispatcher logged.
  ASSERT_OK(dev_interrupt[1].wait(&receive_time));
  ASSERT_EQ(trigger_time, receive_time);

  // If we handled the interrupt status check then there should be no packet on this port.
  zx_port_packet_t packet{};
  ASSERT_EQ(ZX_ERR_TIMED_OUT, port.wait(zx::deadline_after(zx::sec(0)), &packet));
}

TEST_F(PciBusTests, LegacyIrqMaskOnDeliverTest) {
  // Legacy (INTx) interrupts are level-triggered: a device holds its interrupt
  // line asserted until its driver services it. After the bus driver dispatches
  // the interrupt it must mask the device so the line deasserts; otherwise the
  // kernel re-delivers the still-asserted line in a storm. The mask is cleared
  // again once the driver acknowledges. This test verifies that mask/unmask.
  pci_bdf_t device = {0, 0, 0};
  pciroot()
      .ecam()
      .get_device(device)
      ->set_vendor_id(0x8086)
      .set_device_id(0x8086)
      .set_interrupt_pin(0x1)
      .set_status(kStatusInterrupt);
  // Route pin A to vector 16.
  constexpr uint8_t kVector = 0x10;
  zx::interrupt bus_interrupt = AddLegacyIrqToBus(kVector);
  AddRoutingEntryToBus(/*p_dev=*/std::nullopt, /*p_func=*/std::nullopt, /*dev_id=*/0,
                       /*a=*/kVector,
                       /*b=*/0, /*c=*/0, /*d=*/0);
  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();

  auto* bus_device = bus->GetDevice(device);
  ASSERT_OK(bus_device->SetIrqMode(fuchsia_hardware_pci::InterruptMode::kLegacy, 1));
  auto result = bus_device->MapInterrupt(0);
  ASSERT_TRUE(result.is_ok());
  zx::interrupt dev_interrupt = std::move(result.value());

  zx::port port;
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));
  ASSERT_OK(dev_interrupt.bind(port, 0, 0));

  // Reads legacy_disabled under the device lock.
  auto disabled = [bus_device]() {
    fbl::AutoLock _(bus_device->dev_lock());
    return bus_device->irqs().legacy_disabled;
  };
  ASSERT_FALSE(disabled());

  // Fire the hardware vector. The async IRQ handler signals the device's virtual
  // interrupt and then masks the device.
  ASSERT_OK(bus_interrupt.trigger(0, zx::clock::get_boot()));
  mock_ddk::GetDriverRuntime()->RunUntilIdle();

  zx_port_packet_t packet;
  ASSERT_OK(port.wait(zx::time::infinite(), &packet));
  ASSERT_TRUE(disabled());

  // AckLegacyIrq is a no-op that returns ZX_OK for backward compatibility;
  // verify it does not unmask the device prematurely.
  {
    fbl::AutoLock _(bus_device->dev_lock());
    ASSERT_OK(bus_device->AckLegacyIrq());
  }
  ASSERT_TRUE(disabled());

  // Acknowledging the virtual interrupt handle re-arms the device automatically
  // via the ZX_VIRTUAL_INTERRUPT_UNTRIGGERED signal handler.
  ASSERT_OK(dev_interrupt.ack());
  mock_ddk::GetDriverRuntime()->RunUntilIdle();
  ASSERT_FALSE(disabled());

  // Trigger a second interrupt to verify consecutive delivery and re-arming.
  ASSERT_OK(bus_interrupt.trigger(0, zx::clock::get_boot()));
  mock_ddk::GetDriverRuntime()->RunUntilIdle();
  ASSERT_OK(port.wait(zx::time::infinite(), &packet));
  ASSERT_TRUE(disabled());

  ASSERT_OK(dev_interrupt.ack());
  mock_ddk::GetDriverRuntime()->RunUntilIdle();
  ASSERT_FALSE(disabled());
}

TEST_F(PciBusTests, LegacyInterruptLifecycle_SyncWait) {
  pci_bdf_t device = {0, 0, 0};
  pciroot()
      .ecam()
      .get_device(device)
      ->set_vendor_id(0x8086)
      .set_device_id(0x8086)
      .set_interrupt_pin(0x1)
      .set_status(kStatusInterrupt);
  constexpr uint8_t kVector = 0x10;
  zx::interrupt bus_interrupt = AddLegacyIrqToBus(kVector);
  AddRoutingEntryToBus(/*p_dev=*/std::nullopt, /*p_func=*/std::nullopt, /*dev_id=*/0,
                       /*a=*/kVector, /*b=*/0, /*c=*/0, /*d=*/0);
  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();

  auto* bus_device = bus->GetDevice(device);
  ASSERT_OK(bus_device->SetIrqMode(fuchsia_hardware_pci::InterruptMode::kLegacy, 1));
  auto result = bus_device->MapInterrupt(0);
  ASSERT_TRUE(result.is_ok());
  zx::interrupt dev_interrupt = std::move(result.value());

  auto disabled = [bus_device]() {
    fbl::AutoLock _(bus_device->dev_lock());
    return bus_device->irqs().legacy_disabled;
  };
  ASSERT_FALSE(disabled());

  // Trigger physical IRQ vector to signal the virtual interrupt to the waiter thread.
  ASSERT_OK(bus_interrupt.trigger(0, zx::clock::get_boot()));
  mock_ddk::GetDriverRuntime()->RunUntilIdle();

  zx::time_boot receive_time;
  ASSERT_OK(dev_interrupt.wait(&receive_time));
  ASSERT_TRUE(disabled());

  // In synchronous mode, the consumer does not call ack(); instead, its subsequent call
  // to wait() transitions the virtual interrupt to untriggered, unmasking the device.
  thrd_t waiter_thrd;
  auto waiter_entry = [](void* arg) -> int {
    auto* irq = static_cast<zx::interrupt*>(arg);
    zx::time_boot ts;
    return irq->wait(&ts);
  };
  ASSERT_EQ(thrd_create(&waiter_thrd, waiter_entry, &dev_interrupt), thrd_success);
  int thrd_res = 0;
  auto cleanup = fit::defer([&]() {
    dev_interrupt.destroy();
    thrd_join(waiter_thrd, &thrd_res);
  });

  // Wait for the waiter thread to enter wait() and assert ZX_VIRTUAL_INTERRUPT_UNTRIGGERED.
  zx_signals_t observed;
  ASSERT_OK(
      dev_interrupt.wait_one(ZX_VIRTUAL_INTERRUPT_UNTRIGGERED, zx::time::infinite(), &observed));

  mock_ddk::GetDriverRuntime()->RunUntilIdle();
  ASSERT_FALSE(disabled());

  cleanup.call();
  EXPECT_EQ(thrd_res, ZX_ERR_CANCELED);
}

TEST_F(PciBusTests, LegacyInterruptAckLegacyIrqModes) {
  pci_bdf_t device = {0, 0, 0};
  pciroot()
      .ecam()
      .get_device(device)
      ->set_vendor_id(0x8086)
      .set_device_id(0x8086)
      .set_interrupt_pin(0x1);
  constexpr uint8_t kVector = 0x10;
  AddLegacyIrqToBus(kVector);
  AddRoutingEntryToBus(/*p_dev=*/std::nullopt, /*p_func=*/std::nullopt, /*dev_id=*/0,
                       /*a=*/kVector, /*b=*/0, /*c=*/0, /*d=*/0);
  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();
  auto* bus_device = bus->GetDevice(device);

  // In kLegacy mode, AckLegacyIrq is a no-op returning ZX_OK.
  ASSERT_OK(bus_device->SetIrqMode(fuchsia_hardware_pci::InterruptMode::kLegacy, 1));
  {
    fbl::AutoLock _(bus_device->dev_lock());
    ASSERT_OK(bus_device->AckLegacyIrq());
  }

  // Setting LegacyNoack returns ZX_ERR_INVALID_ARGS.
  ASSERT_STATUS(ZX_ERR_INVALID_ARGS,
                bus_device->SetIrqMode(fuchsia_hardware_pci::InterruptMode::kLegacyNoack, 1));

  ASSERT_OK(bus_device->SetIrqMode(fuchsia_hardware_pci::InterruptMode::kDisabled, 0));
  {
    fbl::AutoLock _(bus_device->dev_lock());
    ASSERT_STATUS(ZX_ERR_BAD_STATE, bus_device->AckLegacyIrq());
  }
}

TEST_F(PciBusTests, LegacyInterruptWaitCancellation) {
  pci_bdf_t device = {0, 0, 0};
  pciroot()
      .ecam()
      .get_device(device)
      ->set_vendor_id(0x8086)
      .set_device_id(0x8086)
      .set_interrupt_pin(0x1)
      .set_status(kStatusInterrupt);
  constexpr uint8_t kVector = 0x10;
  zx::interrupt bus_interrupt = AddLegacyIrqToBus(kVector);
  AddRoutingEntryToBus(/*p_dev=*/std::nullopt, /*p_func=*/std::nullopt, /*dev_id=*/0,
                       /*a=*/kVector, /*b=*/0, /*c=*/0, /*d=*/0);
  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();
  auto* bus_device = bus->GetDevice(device);
  ASSERT_OK(bus_device->SetIrqMode(fuchsia_hardware_pci::InterruptMode::kLegacy, 1));
  auto result = bus_device->MapInterrupt(0);
  ASSERT_TRUE(result.is_ok());
  zx::interrupt dev_interrupt = std::move(result.value());
  zx::port port;
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));
  ASSERT_OK(dev_interrupt.bind(port, 0, 0));

  auto disabled = [bus_device]() {
    fbl::AutoLock _(bus_device->dev_lock());
    return bus_device->irqs().legacy_disabled;
  };
  ASSERT_FALSE(disabled());

  // Fire interrupt to start untriggered wait.
  ASSERT_OK(bus_interrupt.trigger(0, zx::clock::get_boot()));
  mock_ddk::GetDriverRuntime()->RunUntilIdle();
  zx_port_packet_t packet;
  ASSERT_OK(port.wait(zx::time::infinite(), &packet));
  ASSERT_TRUE(disabled());

  // Disabling interrupts cancels the pending wait cleanly.
  ASSERT_OK(bus_device->SetIrqMode(fuchsia_hardware_pci::InterruptMode::kDisabled, 0));
  mock_ddk::GetDriverRuntime()->RunUntilIdle();

  // Acknowledging the handle after disable must not unmask the device.
  ASSERT_OK(dev_interrupt.ack());
  mock_ddk::GetDriverRuntime()->RunUntilIdle();
  {
    fbl::AutoLock _(bus_device->dev_lock());
    EXPECT_EQ(bus_device->irqs().mode, fuchsia_hardware_pci::InterruptMode::kDisabled);
    EXPECT_TRUE(bus_device->irqs().legacy_disabled);
  }
}

TEST_F(PciBusTests, LegacyInterruptSharedVector_ConcurrentAssert) {
  pci_bdf_t device_a = {0, 0, 0};
  pci_bdf_t device_b = {0, 1, 0};
  pciroot()
      .ecam()
      .get_device(device_a)
      ->set_vendor_id(0x8086)
      .set_device_id(0x8086)
      .set_interrupt_pin(0x1)
      .set_status(kStatusInterrupt);
  pciroot()
      .ecam()
      .get_device(device_b)
      ->set_vendor_id(0x8086)
      .set_device_id(0x8086)
      .set_interrupt_pin(0x1);
  constexpr uint8_t kVector = 0x10;
  zx::interrupt bus_interrupt = AddLegacyIrqToBus(kVector);
  AddRoutingEntryToBus(/*p_dev=*/std::nullopt, /*p_func=*/std::nullopt, /*dev_id=*/0,
                       /*a=*/kVector, /*b=*/0, /*c=*/0, /*d=*/0);
  AddRoutingEntryToBus(/*p_dev=*/std::nullopt, /*p_func=*/std::nullopt, /*dev_id=*/1,
                       /*a=*/kVector, /*b=*/0, /*c=*/0, /*d=*/0);
  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();

  auto* bus_dev_a = bus->GetDevice(device_a);
  auto* bus_dev_b = bus->GetDevice(device_b);
  ASSERT_OK(bus_dev_a->SetIrqMode(fuchsia_hardware_pci::InterruptMode::kLegacy, 1));
  ASSERT_OK(bus_dev_b->SetIrqMode(fuchsia_hardware_pci::InterruptMode::kLegacy, 1));
  auto res_a = bus_dev_a->MapInterrupt(0);
  auto res_b = bus_dev_b->MapInterrupt(0);
  ASSERT_TRUE(res_a.is_ok());
  ASSERT_TRUE(res_b.is_ok());
  zx::interrupt dev_a_irq = std::move(res_a.value());
  zx::interrupt dev_b_irq = std::move(res_b.value());

  zx::port port;
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));
  ASSERT_OK(dev_a_irq.bind(port, 1, 0));
  ASSERT_OK(dev_b_irq.bind(port, 2, 0));

  auto disabled_a = [bus_dev_a]() {
    fbl::AutoLock _(bus_dev_a->dev_lock());
    return bus_dev_a->irqs().legacy_disabled;
  };
  auto disabled_b = [bus_dev_b]() {
    fbl::AutoLock _(bus_dev_b->dev_lock());
    return bus_dev_b->irqs().legacy_disabled;
  };

  // Assert vector: Device A asserts while Device B is idle.
  ASSERT_OK(bus_interrupt.trigger(0, zx::clock::get_boot()));
  mock_ddk::GetDriverRuntime()->RunUntilIdle();

  zx_port_packet_t packet;
  ASSERT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_EQ(packet.key, 1u);
  ASSERT_TRUE(disabled_a());
  ASSERT_FALSE(disabled_b());

  // Vector triggers again while Device A ack is pending; Device B asserts.
  pciroot().ecam().get_device(device_b)->set_status(kStatusInterrupt);
  ASSERT_OK(bus_interrupt.trigger(0, zx::clock::get_boot()));
  mock_ddk::GetDriverRuntime()->RunUntilIdle();

  // Device B is signaled and masked; Device A was skipped without error.
  ASSERT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_EQ(packet.key, 2u);
  ASSERT_TRUE(disabled_b());
  ASSERT_TRUE(disabled_a());

  // Acknowledging Device A unmasks it while Device B remains masked.
  ASSERT_OK(dev_a_irq.ack());
  mock_ddk::GetDriverRuntime()->RunUntilIdle();
  ASSERT_FALSE(disabled_a());
  ASSERT_TRUE(disabled_b());

  // Acknowledging Device B unmasks the remaining device.
  ASSERT_OK(dev_b_irq.ack());
  mock_ddk::GetDriverRuntime()->RunUntilIdle();
  ASSERT_FALSE(disabled_b());
}

TEST_F(PciBusTests, LegacyInterruptWorkaround_PendingSkip) {
  pci_bdf_t device_a = {0, 0, 0};
  pci_bdf_t device_b = {0, 1, 0};
  pciroot()
      .ecam()
      .get_device(device_a)
      ->set_vendor_id(0x8086)
      .set_device_id(0x8086)
      .set_interrupt_pin(0x1);
  pciroot()
      .ecam()
      .get_device(device_b)
      ->set_vendor_id(0x8086)
      .set_device_id(0x8086)
      .set_interrupt_pin(0x1);
  constexpr uint8_t kVector = 0x10;
  zx::interrupt bus_interrupt = AddLegacyIrqToBus(kVector);
  AddRoutingEntryToBus(/*p_dev=*/std::nullopt, /*p_func=*/std::nullopt, /*dev_id=*/0,
                       /*a=*/kVector, /*b=*/0, /*c=*/0, /*d=*/0);
  AddRoutingEntryToBus(/*p_dev=*/std::nullopt, /*p_func=*/std::nullopt, /*dev_id=*/1,
                       /*a=*/kVector, /*b=*/0, /*c=*/0, /*d=*/0);

  // Set use_intx_workaround board config.
  fuchsia_hardware_pci::BoardConfiguration board_config;
  board_config.use_intx_workaround(true);
  SetBoardConfiguration(std::move(board_config));

  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();

  auto* bus_dev_a = bus->GetDevice(device_a);
  auto* bus_dev_b = bus->GetDevice(device_b);
  ASSERT_OK(bus_dev_a->SetIrqMode(fuchsia_hardware_pci::InterruptMode::kLegacy, 1));
  ASSERT_OK(bus_dev_b->SetIrqMode(fuchsia_hardware_pci::InterruptMode::kLegacy, 1));
  auto res_a = bus_dev_a->MapInterrupt(0);
  auto res_b = bus_dev_b->MapInterrupt(0);
  ASSERT_TRUE(res_a.is_ok());
  ASSERT_TRUE(res_b.is_ok());
  zx::interrupt dev_a_irq = std::move(res_a.value());
  zx::interrupt dev_b_irq = std::move(res_b.value());

  zx::port port;
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));
  ASSERT_OK(dev_a_irq.bind(port, 1, 0));
  ASSERT_OK(dev_b_irq.bind(port, 2, 0));

  auto disabled_a = [bus_dev_a]() {
    fbl::AutoLock _(bus_dev_a->dev_lock());
    return bus_dev_a->irqs().legacy_disabled;
  };
  auto disabled_b = [bus_dev_b]() {
    fbl::AutoLock _(bus_dev_b->dev_lock());
    return bus_dev_b->irqs().legacy_disabled;
  };

  // Under workaround, both devices are signaled and masked on vector trigger.
  ASSERT_OK(bus_interrupt.trigger(0, zx::clock::get_boot()));
  mock_ddk::GetDriverRuntime()->RunUntilIdle();
  zx_port_packet_t packet;
  ASSERT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_TRUE(packet.key == 1u || packet.key == 2u);
  uint64_t first_key = packet.key;
  ASSERT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_TRUE(packet.key == 1u || packet.key == 2u);
  EXPECT_NE(packet.key, first_key);
  ASSERT_TRUE(disabled_a());
  ASSERT_TRUE(disabled_b());

  // Acknowledge Device A only -> Device A unmasks.
  ASSERT_OK(dev_a_irq.ack());
  mock_ddk::GetDriverRuntime()->RunUntilIdle();
  ASSERT_FALSE(disabled_a());
  ASSERT_TRUE(disabled_b());

  // Trigger vector again -> Device A is signaled and masked; Device B (pending) is skipped.
  ASSERT_OK(bus_interrupt.trigger(0, zx::clock::get_boot()));
  mock_ddk::GetDriverRuntime()->RunUntilIdle();
  ASSERT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_EQ(packet.key, 1u);
  ASSERT_TRUE(disabled_a());
  ASSERT_TRUE(disabled_b());

  // Acknowledge both devices to return them to unmasked state.
  ASSERT_OK(dev_a_irq.ack());
  ASSERT_OK(dev_b_irq.ack());
  mock_ddk::GetDriverRuntime()->RunUntilIdle();
  ASSERT_FALSE(disabled_a());
  ASSERT_FALSE(disabled_b());
}

TEST_F(PciBusTests, LegacyIrqHandlerErrorAndEdgeCases) {
  pci_bdf_t device_bdf = {0, 0, 0};
  pciroot()
      .ecam()
      .get_device(device_bdf)
      ->set_vendor_id(0x8086)
      .set_device_id(0x8086)
      .set_interrupt_pin(0x1)
      .set_status(kStatusInterrupt);
  constexpr uint8_t kVector = 0x10;
  zx::interrupt bus_interrupt = AddLegacyIrqToBus(kVector);
  AddRoutingEntryToBus(/*p_dev=*/std::nullopt, /*p_func=*/std::nullopt, /*dev_id=*/0,
                       /*a=*/kVector, /*b=*/0, /*c=*/0, /*d=*/0);

  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();
  auto* bus_device = bus->GetDevice(device_bdf);
  ASSERT_OK(bus_device->SetIrqMode(fuchsia_hardware_pci::InterruptMode::kLegacy, 1));

  // HandleLegacyIrq with ZX_ERR_CANCELED returns early.
  bus->HandleLegacyIrq(nullptr, nullptr, ZX_ERR_CANCELED, nullptr, kVector);

  // HandleLegacyIrq with non-OK status logs error and returns early.
  bus->HandleLegacyIrq(nullptr, nullptr, ZX_ERR_INTERNAL, nullptr, kVector);

  // HandleLegacyIrq with null interrupt packet logs error and returns early.
  bus->HandleLegacyIrq(nullptr, nullptr, ZX_OK, nullptr, kVector);

  // HandleLegacyIrq with untracked vector logs error and returns early.
  zx_packet_interrupt_t packet = {};
  bus->HandleLegacyIrq(nullptr, nullptr, ZX_OK, &packet, 0x99);

  // HandleDeviceLegacyIrqUntriggered with ZX_ERR_CANCELED returns early.
  bus->HandleDeviceLegacyIrqUntriggered(nullptr, nullptr, ZX_ERR_CANCELED, nullptr, bus_device);

  // HandleDeviceLegacyIrqUntriggered with non-OK status logs error and returns early.
  bus->HandleDeviceLegacyIrqUntriggered(nullptr, nullptr, ZX_ERR_INTERNAL, nullptr, bus_device);

  // Invalidate the device's virtual interrupt handle so SignalLegacyIrq fails.
  // Device remains masked to prevent interrupt storms, and ack failure on
  // the untriggered bus_interrupt is safely handled.
  {
    fbl::AutoLock _(bus_device->dev_lock());
    bus_device->irqs().legacy.reset();
  }
  packet.timestamp = zx::clock::get_boot().get();
  bus->HandleLegacyIrq(nullptr, nullptr, ZX_OK, &packet, kVector);
  {
    fbl::AutoLock _(bus_device->dev_lock());
    EXPECT_TRUE(bus_device->irqs().legacy_disabled);
  }

  // HandleDeviceLegacyIrqUntriggered returns early if mode is not kLegacy.
  ASSERT_OK(bus_device->SetIrqMode(fuchsia_hardware_pci::InterruptMode::kDisabled, 0));
  bus->HandleDeviceLegacyIrqUntriggered(nullptr, nullptr, ZX_OK, nullptr, bus_device);
  {
    fbl::AutoLock _(bus_device->dev_lock());
    EXPECT_TRUE(bus_device->irqs().legacy_disabled);
    EXPECT_EQ(bus_device->irqs().mode, fuchsia_hardware_pci::InterruptMode::kDisabled);
  }
}

TEST_F(PciBusTests, ObeysHeaderTypeMultiFn) {
  auto& ecam = pciroot().ecam();

  ecam.get_device({0, 0, 0})->set_vendor_id(0x8086).set_device_id(1).set_header_type(
      kHeaderTypeMultiFn);
  ecam.get_device({0, 0, 1})->set_vendor_id(0x8086).set_device_id(2);
  ecam.get_device({0, 1, 0})->set_vendor_id(0x8086).set_device_id(3).set_header_type(0);
  ecam.get_device({0, 1, 1})->set_vendor_id(0x8086).set_device_id(4);

  auto owned_bus =
      std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(), std::nullopt);
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();
  ASSERT_EQ(bus->GetDeviceCount(), 3u);
}

TEST_F(PciBusTests, MultiFnAllFunctionsScanned) {
  uint32_t dev_cnt = SetupMultifunctionTopology();
  auto owned_bus =
      std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(), std::nullopt);
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();
  ASSERT_EQ(bus->GetDeviceCount(), dev_cnt);
}

// A device whose BDF the platform reports as devicetree-described gets only a
// fragment from the bus driver; the bus driver does not publish a composite node
// spec for it (the devicetree owns the composite).
TEST_F(PciBusTests, DevicetreeDeviceSkipsCompositeSpec) {
  auto& ecam = pciroot().ecam();
  ecam.get_device({0, 0, 0})->set_vendor_id(0x8086).set_device_id(1);
  pciroot().devicetree_devices().push_back(
      {/*bus_id=*/0x0, /*device_id=*/0x0, /*function_id=*/0x0});

  g_pci_composite_spec_add_count = 0;
  auto owned_bus =
      std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(), std::nullopt);
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();
  ASSERT_EQ(bus->GetDeviceCount(), 1u);
  EXPECT_EQ(g_pci_composite_spec_add_count, 0);
}

// The same topology without the devicetree entry: the bus driver publishes a
// composite node spec as usual.
TEST_F(PciBusTests, NonDevicetreeDeviceAddsCompositeSpec) {
  auto& ecam = pciroot().ecam();
  ecam.get_device({0, 0, 0})->set_vendor_id(0x8086).set_device_id(1);

  g_pci_composite_spec_add_count = 0;
  auto owned_bus =
      std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(), std::nullopt);
  ASSERT_OK(owned_bus->Initialize());
  auto* bus = owned_bus.release();
  ASSERT_EQ(bus->GetDeviceCount(), 1u);
  EXPECT_EQ(g_pci_composite_spec_add_count, 1);
}

TEST_F(PciBusTests, MetadataEmpty) {
  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  EXPECT_TRUE(owned_bus->GetBoardConfiguration().IsEmpty());
  [[maybe_unused]] auto* bus = owned_bus.release();
}

TEST_F(PciBusTests, MetadataPopulated) {
  SetBoardConfiguration({{PciFidl::UseIntxWorkaroundType()}});
  auto owned_bus = std::make_unique<TestBus>(parent(), pciroot().proto(), pciroot().info(),
                                             pciroot().ecam().mmio());
  ASSERT_OK(owned_bus->Initialize());
  EXPECT_FALSE(owned_bus->GetBoardConfiguration().IsEmpty());
  EXPECT_TRUE(owned_bus->GetBoardConfiguration().use_intx_workaround().has_value());
  [[maybe_unused]] auto* bus = owned_bus.release();
}

}  // namespace pci
