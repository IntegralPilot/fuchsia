// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.pci/cpp/wire.h>
#include <lib/device-protocol/pci.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/process.h>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/framebuffer-bochs-display/bochs-vbe-registers.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/graphics/display/lib/framebuffer-display/framebuffer-display-driver.h"
#include "src/graphics/display/lib/framebuffer-display/framebuffer-display.h"

namespace framebuffer_display {

namespace {

constexpr int kDisplayWidth = 1024;
constexpr int kDisplayHeight = 768;
constexpr auto kDisplayFormat = display::PixelFormat::kB8G8R8A8;
constexpr int kBitsPerPixel = 32;

zx::result<> SetUpBochsDisplayEngine(fdf::MmioView mmio_space, int width, int height,
                                     int bits_per_pixel) {
  static constexpr uint16_t kDriverSupportedDisplayEngineApiVersion =
      bochs_vbe::DisplayEngineApiVersion::kVersion5;

  // Enabling linear framebuffer requires an API version of kVersion2 or higher.
  // Setting up virtual display requires an API version of kVersion1 or higher.
  static constexpr uint16_t kDriverRequiredDisplayEngineApiVersion =
      std::max(bochs_vbe::DisplayEngineApiVersion::kVersion2,
               bochs_vbe::kMinimumSupportedDisplayEngineApiVersion);

  // The Bochs VBE Display API states that the emulator expects the driver
  // ("bios") to write its supported API version before reading the version
  // supported by the hardware ("display code").
  bochs_vbe::DisplayEngineApiVersion::Get()
      .FromValue(kDriverSupportedDisplayEngineApiVersion)
      .WriteTo(&mmio_space);
  const uint16_t hardware_supported_display_engine_api_version =
      bochs_vbe::DisplayEngineApiVersion::Get().ReadFrom(&mmio_space).version();

  if (hardware_supported_display_engine_api_version < kDriverRequiredDisplayEngineApiVersion) {
    fdf::error(
        "Hardware supported display engine API version (0x{:04x}) is too low. Driver requires API version 0x{:04x}",
        hardware_supported_display_engine_api_version, kDriverRequiredDisplayEngineApiVersion);
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  bochs_vbe::DisplayFeatureControl display_feature_control =
      bochs_vbe::DisplayFeatureControl::Get().FromValue(0);
  display_feature_control.set_display_engine_enabled(false).WriteTo(&mmio_space);

  bochs_vbe::DisplayBitsPerPixel::Get()
      .FromValue(0)
      .set_bits_per_pixel(bits_per_pixel)
      .WriteTo(&mmio_space);
  bochs_vbe::DisplayHorizontalResolution::Get().FromValue(0).set_pixels(width).WriteTo(&mmio_space);
  bochs_vbe::DisplayVerticalResolution::Get().FromValue(0).set_pixels(height).WriteTo(&mmio_space);
  bochs_vbe::VideoMemoryBankIndex::Get().FromValue(0).set_bank_index(0).WriteTo(&mmio_space);

  bochs_vbe::VirtualDisplayWidth::Get().FromValue(0).set_pixels(width).WriteTo(&mmio_space);
  bochs_vbe::VirtualDisplayHeight::Get().FromValue(0).set_pixels(height).WriteTo(&mmio_space);
  bochs_vbe::VirtualDisplayHorizontalOffset::Get().FromValue(0).set_pixels(0).WriteTo(&mmio_space);
  bochs_vbe::VirtualDisplayVerticalOffset::Get().FromValue(0).set_pixels(0).WriteTo(&mmio_space);

  display_feature_control.set_linear_frame_buffer_enabled(true)
      .set_display_engine_enabled(true)
      .WriteTo(&mmio_space);

  return zx::ok();
}

void LogBochsDisplayEngineRegisters(fdf::MmioView mmio_space) {
  fdf::info("Bochs display engine current state:");
  fdf::info("{:30s}: 0x{:04x}", "Hardware supported API version",
            bochs_vbe::DisplayEngineApiVersion::Get().ReadFrom(&mmio_space).version());
  fdf::info("{:30s}: {} x {}", "Physical resolution (px)",
            bochs_vbe::DisplayHorizontalResolution::Get().ReadFrom(&mmio_space).pixels(),
            bochs_vbe::DisplayVerticalResolution::Get().ReadFrom(&mmio_space).pixels());
  fdf::info("{:30s}: {}", "Color depth",
            bochs_vbe::DisplayBitsPerPixel::Get().ReadFrom(&mmio_space).bits_per_pixel());

  bochs_vbe::DisplayFeatureControl display_feature_control =
      bochs_vbe::DisplayFeatureControl::Get().ReadFrom(&mmio_space);
  fdf::info("{:30s}: {}", "Video memory preserved on enable",
            display_feature_control.video_memory_preserved_on_enable() ? "true" : "false");
  fdf::info("{:30s}: {}", "Linear frame buffer",
            display_feature_control.linear_frame_buffer_enabled() ? "enabled" : "disabled");
  fdf::info("{:30s}: {}", "Pallete DAC mode",
            display_feature_control.palette_dac_in_8bit_mode() ? "8-bit" : "6-bit");
  fdf::info("{:30s}: {}", "Read display capabilities",
            display_feature_control.read_display_capabilities() ? "true" : "false");
  fdf::info("{:30s}: {}", "Display engine",
            display_feature_control.display_engine_enabled() ? "enabled" : "disabled");

  fdf::info("{:30s}: {}", "Video memory bank index",
            bochs_vbe::VideoMemoryBankIndex::Get().ReadFrom(&mmio_space).bank_index());
  fdf::info("{:30s}: {}", "Virtual display width",
            bochs_vbe::VirtualDisplayWidth::Get().ReadFrom(&mmio_space).pixels());
  fdf::info("{:30s}: {}", "Virtual display height",
            bochs_vbe::VirtualDisplayHeight::Get().ReadFrom(&mmio_space).pixels());
  fdf::info("{:30s}: ({}, {})", "Virtual display offset",
            bochs_vbe::VirtualDisplayHorizontalOffset::Get().ReadFrom(&mmio_space).pixels(),
            bochs_vbe::VirtualDisplayVerticalOffset::Get().ReadFrom(&mmio_space).pixels());
  fdf::info("{:30s}: {}", "Video memory size (bytes)",
            bochs_vbe::VideoMemorySize::Get().ReadFrom(&mmio_space).GetVideoMemorySizeBytes());
}

// Driver for the QEMU Bochs-compatible display engine.
class FramebufferBochsDisplayDriver final : public FramebufferDisplayDriver {
 public:
  FramebufferBochsDisplayDriver();

  FramebufferBochsDisplayDriver(const FramebufferBochsDisplayDriver&) = delete;
  FramebufferBochsDisplayDriver(FramebufferBochsDisplayDriver&&) = delete;
  FramebufferBochsDisplayDriver& operator=(const FramebufferBochsDisplayDriver&) = delete;
  FramebufferBochsDisplayDriver& operator=(FramebufferBochsDisplayDriver&&) = delete;

  ~FramebufferBochsDisplayDriver() override;

  // FramebufferDisplayDriver:
  zx::result<> ConfigureHardware() override;
  zx::result<fdf::MmioBuffer> GetFrameBufferMmioBuffer() override;
  zx::result<DisplayProperties> GetDisplayProperties() override;

 private:
  zx::result<ddk::Pci> GetPciClient();
};

FramebufferBochsDisplayDriver::FramebufferBochsDisplayDriver()
    : FramebufferDisplayDriver("framebuffer-bochs-display") {}

FramebufferBochsDisplayDriver::~FramebufferBochsDisplayDriver() = default;

zx::result<ddk::Pci> FramebufferBochsDisplayDriver::GetPciClient() {
  zx::result<fidl::ClientEnd<fuchsia_hardware_pci::Device>> pci_result =
      incoming().Connect<fuchsia_hardware_pci::Service::Device>("pci");
  if (pci_result.is_error()) {
    fdf::error("Failed to connect to PCI protocol: {}", pci_result);
    return pci_result.take_error();
  }
  ddk::Pci pci(std::move(pci_result).value());
  ZX_DEBUG_ASSERT(pci.is_valid());
  return zx::ok(std::move(pci));
}

zx::result<> FramebufferBochsDisplayDriver::ConfigureHardware() {
  zx::result<ddk::Pci> pci_result = GetPciClient();
  if (pci_result.is_error()) {
    return pci_result.take_error();
  }
  ddk::Pci pci = std::move(pci_result).value();

  std::optional<fdf::MmioBuffer> mmio;

  // The MMIO PCI BAR (base address register).
  //
  // The QEMU team, QEMU Standard VGA specs, section "PCI spec", version 9.0.0,
  // Apr 2024.
  // https://qemu.readthedocs.io/en/v9.0.0/specs/standard-vga.html#pci-spec
  static constexpr uint32_t kQemuBochsRegisterMmioPciBarIndex = 2;
  zx_status_t status =
      pci.MapMmio(kQemuBochsRegisterMmioPciBarIndex, ZX_CACHE_POLICY_UNCACHED_DEVICE, &mmio);
  if (status != ZX_OK) {
    fdf::error("Failed to map PCI config: {}", zx::make_result(status));
    return zx::error(status);
  }

  // QEMU Bochs-compatible display engine register MMIO addresses
  //
  // The Bochs VBE display API only specifies port IO (using
  // `VBE_DISPI_IOPORT_INDEX` and `VBE_DISPI_IOPORT_DATA`) to read / write device
  // registers.
  //
  // The QEMU Bochs VBE display implementation also supports memory-mapped IO
  // for Bochs registers. The MMIO area 0x0500 - 0x0515 is used for Bochs
  // registers.
  //
  // The QEMU team, QEMU Standard VGA specs, section "PCI spec", version 9.0.0,
  // Apr 2024.
  // https://qemu.readthedocs.io/en/v9.0.0/specs/standard-vga.html#pci-spec
  static constexpr uint64_t kQemuBochsRegisterOffset = 0x500;
  static constexpr size_t kQemuBochsRegisterSizeBytes = 0x16;

  fdf::MmioView bochs_registers = mmio->View(kQemuBochsRegisterOffset, kQemuBochsRegisterSizeBytes);

  zx::result<> setup_result =
      SetUpBochsDisplayEngine(bochs_registers, kDisplayWidth, kDisplayHeight, kBitsPerPixel);
  LogBochsDisplayEngineRegisters(bochs_registers);

  return setup_result;
}

zx::result<fdf::MmioBuffer> FramebufferBochsDisplayDriver::GetFrameBufferMmioBuffer() {
  zx::result<ddk::Pci> pci_result = GetPciClient();
  if (pci_result.is_error()) {
    return pci_result.take_error();
  }
  ddk::Pci pci = std::move(pci_result).value();

  std::optional<fdf::MmioBuffer> framebuffer_mmio;

  // The framebuffer memory PCI BAR (base address register).
  //
  // The QEMU team, QEMU Standard VGA specs, section "PCI spec", version 9.0.0,
  // Apr 2024.
  // https://qemu.readthedocs.io/en/v9.0.0/specs/standard-vga.html#pci-spec
  static constexpr uint32_t kFramebufferPciBarIndex = 0;
  zx_status_t status =
      pci.MapMmio(kFramebufferPciBarIndex, ZX_CACHE_POLICY_WRITE_COMBINING, &framebuffer_mmio);
  if (status != ZX_OK) {
    fdf::error("Failed to map PCI bar {}: {}", kFramebufferPciBarIndex, zx::make_result(status));
    return zx::error(status);
  }

  ZX_DEBUG_ASSERT(framebuffer_mmio.has_value());
  return zx::ok(std::move(framebuffer_mmio).value());
}

zx::result<DisplayProperties> FramebufferBochsDisplayDriver::GetDisplayProperties() {
  static constexpr DisplayProperties kDisplayProperties = {
      .width_px = kDisplayWidth,
      .height_px = kDisplayHeight,
      .row_stride_px = kDisplayWidth,
      .pixel_format = kDisplayFormat,
  };
  return zx::ok(kDisplayProperties);
}

}  // namespace

}  // namespace framebuffer_display

FUCHSIA_DRIVER_EXPORT2(framebuffer_display::FramebufferBochsDisplayDriver);
