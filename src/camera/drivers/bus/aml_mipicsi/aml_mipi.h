// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CAMERA_DRIVERS_BUS_AML_MIPICSI_AML_MIPI_H_
#define SRC_CAMERA_DRIVERS_BUS_AML_MIPICSI_AML_MIPI_H_

#include <fuchsia/hardware/mipicsi/cpp/banjo.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/fzl/pinned-vmo.h>
#include <lib/zx/interrupt.h>

#include <atomic>

#include <ddk/metadata/camera.h>
#include <ddktl/device.h>
#ifndef _ALL_SOURCE
#define _ALL_SOURCE  // Enables thrd_create_with_name in <threads.h>.
#endif
#include <threads.h>

namespace camera {
// |AmlMipiDevice| is spawned by the driver in |aml_mipi.cc|
// to which the IMX 277 Sensor driver binds to.
// This class provides the ZX_PROTOCOL_MIPICSI ops for all of it's
// children.
class AmlMipiDevice;
using DeviceType = ddk::Device<AmlMipiDevice>;

class AmlMipiDevice : public DeviceType,
                      public ddk::MipiCsiProtocol<AmlMipiDevice, ddk::base_protocol> {
 public:
  // MMIO Indexes.
  static constexpr uint32_t kCsiPhy0 = 0;
  static constexpr uint32_t kAphy0 = 1;
  static constexpr uint32_t kCsiHost0 = 2;
  static constexpr uint32_t kMipiAdap = 3;
  static constexpr uint32_t kHiu = 4;

  // CLK Shifts & Masks
  static constexpr uint32_t kClkMuxMask = 0xfff;
  static constexpr uint32_t kClockEnableShift = 8;

  DISALLOW_COPY_AND_ASSIGN_ALLOW_MOVE(AmlMipiDevice);

  explicit AmlMipiDevice(zx_device_t* parent) : DeviceType(parent) {}

  static zx_status_t Bind(void* ctx, zx_device_t* parent);

  ~AmlMipiDevice();

  // Methods required by the ddk.
  void DdkRelease();

  // Methods for ZX_PROTOCOL_MIPI_CSI.
  zx_status_t MipiCsiInit(const mipi_info_t* mipi_info, const mipi_adap_info_t* adap_info);
  zx_status_t MipiCsiDeInit();

 private:
  zx_status_t InitBuffer(const mipi_adap_info_t* info, size_t size);
  zx_status_t Init();
  static uint32_t AdapGetDepth(const mipi_adap_info_t* info);
  int AdapterIrqHandler();

  // MIPI Iternal APIs
  void MipiPhyInit(const mipi_info_t* info);
  void MipiCsi2Init(const mipi_info_t* info);
  void MipiPhyReset();
  void MipiCsi2Reset();

  // Mipi Adapter APIs
  zx_status_t MipiAdapInit(const mipi_adap_info_t* info);
  void MipiAdapStart(const mipi_adap_info_t* info);
  void MipiAdapReset();
  void MipiAdapStart();

  // Initialize the MIPI host to 720p by default.
  zx_status_t AdapFrontendInit(const mipi_adap_info_t* info);
  zx_status_t AdapReaderInit(const mipi_adap_info_t* info);
  zx_status_t AdapPixelInit(const mipi_adap_info_t* info);
  zx_status_t AdapAlignInit(const mipi_adap_info_t* info);

  // Set it up for the correct format and resolution.
  void AdapAlignStart(const mipi_adap_info_t* info);
  void AdapPixelStart(const mipi_adap_info_t* info);
  void AdapReaderStart(const mipi_adap_info_t* info);
  void AdapFrontEndStart(const mipi_adap_info_t* info);

  // MIPI clock.
  void InitMipiClock();

  // Debug.
  void DumpCsiPhyRegs();
  void DumpAPhyRegs();
  void DumpHostRegs();
  void DumpFrontEndRegs();
  void DumpReaderRegs();
  void DumpAlignRegs();
  void DumpPixelRegs();
  void DumpMiscRegs();

  std::optional<fdf::MmioBuffer> csi_phy0_mmio_;
  std::optional<fdf::MmioBuffer> aphy0_mmio_;
  std::optional<fdf::MmioBuffer> csi_host0_mmio_;
  std::optional<fdf::MmioBuffer> mipi_adap_mmio_;
  std::optional<fdf::MmioBuffer> hiu_mmio_;

  zx::bti bti_;
  zx::interrupt adap_irq_;
  std::atomic<bool> running_;
  std::optional<thrd_t> irq_thread_;

  zx::vmo ring_buffer_vmo_;
  fzl::PinnedVmo pinned_ring_buffer_;
};

}  // namespace camera

#endif  // SRC_CAMERA_DRIVERS_BUS_AML_MIPICSI_AML_MIPI_H_
