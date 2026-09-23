// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_I2C_GMBUS_GPIO_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_I2C_GMBUS_GPIO_H_

#include <zircon/assert.h>

#include <optional>

#include "src/graphics/display/drivers/intel-display/hardware-common.h"

namespace intel_display {

// The GMBUS controller has multiple GPIO pin pairs used as I2C clock and
// data lines connected to DDIs for I2C-based data transfer protocol like
// VESA DDC/CI and E-DDC. The mapping between pin pair number and DDI ID
// vary from platform to platform.
//
// This class wraps the (pin pair, platform) tuple and can be used for
// conversion between DDIs and GMBUs pin pairs on different platforms.
class GMBusPinPair {
 public:
  // Returns the number of the pin pair that is used by GMBusClockPortSelect
  // register.
  int number() const { return number_; }

  // Returns the ID of the DDI this pin pair connects in the Display Engine.
  DdiId ddi_id() const {
    switch (platform_) {
      case registers::Platform::kSkylake:
      case registers::Platform::kKabyLake:
      case registers::Platform::kTestDevice:
        return ToDdiIdSkylake();
      case registers::Platform::kTigerLake:
        return ToDdiIdTigerLake();
    }
  }

  // Get the GMBUS pin pair for a given DDI ID on `platform`.
  //
  // The `ddi_id` must be a valid DDI available in `platform`.
  static std::optional<GMBusPinPair> GetForDdi(DdiId ddi_id, registers::Platform platform) {
    switch (platform) {
      case registers::Platform::kSkylake:
      case registers::Platform::kKabyLake:
      case registers::Platform::kTestDevice:
        return GetForDdiSkylake(ddi_id, platform);
      case registers::Platform::kTigerLake:
        return GetForDdiTigerLake(ddi_id, platform);
    }
  }

  // Returns whether a DDI has a valid pin pair on `platform`.
  static bool HasValidPinPair(DdiId ddi_id, registers::Platform platform) {
    switch (platform) {
      case registers::Platform::kSkylake:
      case registers::Platform::kKabyLake:
      case registers::Platform::kTestDevice:
        return HasValidPinPairSkylake(ddi_id);
      case registers::Platform::kTigerLake:
        return HasValidPinPairTigerLake(ddi_id);
    }
  }

 private:
  GMBusPinPair(int number, registers::Platform platform) : number_(number), platform_(platform) {}

  // Helper method to convert given pin pair to its corresponding DDI ID
  // for Tiger Lake.
  //
  // The DDI <-> pin pair mapping is available at:
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 1, Page 1020
  DdiId ToDdiIdTigerLake() const {
    ZX_DEBUG_ASSERT(platform_ == registers::Platform::kTigerLake);
    switch (number_) {
      case 1:
      case 2:
      case 3:
        return static_cast<DdiId>(DdiId::DDI_A + (number_ - 1));
      case 9:
      case 10:
      case 11:
      case 12:
      case 13:
      case 14:
        return static_cast<DdiId>(DdiId::DDI_TC_1 + (number_ - 9));
      default:
        ZX_ASSERT_MSG(false, "Invalid GMBUS pin pair selected: %d", number_);
    }
  }

  // Helper method to convert given pin pair to its corresponding DDI ID
  // for Skylake / Kaby Lake.
  //
  // The DDI <-> pin pair mapping is available at:
  // Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 1, Page 728
  // Skylake: IHD-OS-SKL-Vol 2c-05.16 Part 1, Page 723
  DdiId ToDdiIdSkylake() const {
    ZX_DEBUG_ASSERT(platform_ == registers::Platform::kSkylake ||
                    platform_ == registers::Platform::kKabyLake ||
                    platform_ == registers::Platform::kTestDevice);
    switch (number_) {
      case 0b100:
        return DdiId::DDI_C;
      case 0b101:
        return DdiId::DDI_B;
      case 0b110:
        return DdiId::DDI_D;
      default:
        ZX_ASSERT_MSG(false, "Invalid GMBUS pin pair selected: %d", number_);
    }
  }

  // Helper method to get a GMBusPinPair for a given DDI ID for Tiger Lake.
  // If the DDI is valid but not connected to GMBus, returns nullopt.
  //
  // The DDI <-> pin pair mapping is available at:
  // Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev 2.0 Part 1, Page 1020
  static std::optional<GMBusPinPair> GetForDdiTigerLake(DdiId ddi_id,
                                                        registers::Platform platform) {
    ZX_DEBUG_ASSERT(platform == registers::Platform::kTigerLake);
    switch (ddi_id) {
      case DdiId::DDI_A:
      case DdiId::DDI_B:
      case DdiId::DDI_C:
        return GMBusPinPair{1 + (ddi_id - DdiId::DDI_A), platform};
      case DdiId::DDI_TC_1:
      case DdiId::DDI_TC_2:
      case DdiId::DDI_TC_3:
      case DdiId::DDI_TC_4:
      case DdiId::DDI_TC_5:
      case DdiId::DDI_TC_6:
        return GMBusPinPair{9 + (ddi_id - DdiId::DDI_TC_1), platform};
      default:
        ZX_ASSERT_MSG(false, "Invalid DDI: %d", ddi_id);
        break;
    }
  }

  // Helper method to get a GMBusPinPair for a given DDI ID for Skylake / Kaby
  // Lake.
  // If the DDI is valid but not connected to GMBus, returns nullopt.
  //
  // The DDI <-> pin pair mapping is available at:
  // Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 1, Page 728
  // Skylake: IHD-OS-SKL-Vol 2c-05.16 Part 1, Page 723
  static std::optional<GMBusPinPair> GetForDdiSkylake(DdiId ddi_id, registers::Platform platform) {
    ZX_DEBUG_ASSERT(platform == registers::Platform::kSkylake ||
                    platform == registers::Platform::kKabyLake ||
                    platform == registers::Platform::kTestDevice);
    switch (ddi_id) {
      case DdiId::DDI_B:
        return GMBusPinPair{0b101, platform};
      case DdiId::DDI_C:
        return GMBusPinPair{0b100, platform};
      case DdiId::DDI_D:
        return GMBusPinPair{0b110, platform};
      case DdiId::DDI_A:
      case DdiId::DDI_E:
        return std::nullopt;
      default:
        ZX_ASSERT_MSG(false, "Invalid DDI: %d", ddi_id);
        break;
    }
  }

  // Returns if a DDI has valid GMBUS pin pair in Tiger Lake.
  static bool HasValidPinPairTigerLake(DdiId ddi_id) {
    switch (ddi_id) {
      case DdiId::DDI_A:
      case DdiId::DDI_B:
      case DdiId::DDI_C:
      case DdiId::DDI_TC_1:
      case DdiId::DDI_TC_2:
      case DdiId::DDI_TC_3:
      case DdiId::DDI_TC_4:
      case DdiId::DDI_TC_5:
      case DdiId::DDI_TC_6:
        return true;
      default:
        return false;
    }
  }

  // Returns if a DDI has valid GMBUS pin pair in Skylake / Kaby Lake.
  static bool HasValidPinPairSkylake(DdiId ddi_id) {
    switch (ddi_id) {
      case DdiId::DDI_B:
      case DdiId::DDI_C:
      case DdiId::DDI_D:
        return true;
      default:
        return false;
    }
  }

  int number_;
  registers::Platform platform_;
};

// The Intel Display Engine has multiple GPIO pin pairs (ports) which may
// be used as I2C clock and data lines, or for DSI devices. The mapping between
// GPIO port number and DDI ID vary from platform to platform.
//
// This class wraps the (GPIO port, platform) tuple and can be used for
// conversion between DDIs and GMBUs pin pairs on different platforms.
class GpioPort {
 public:
  // Returns the number of the GPIO port that is used when selecting the GPIO
  // control register.
  int number() const { return number_; }

  // Returns the ID of the DDI this GPIO port connects in the Display Engine.
  DdiId ddi_id() const {
    switch (platform_) {
      case registers::Platform::kSkylake:
      case registers::Platform::kKabyLake:
      case registers::Platform::kTestDevice:
        return ToDdiIdSkylake();
      case registers::Platform::kTigerLake:
        return ToDdiIdTigerLake();
    }
  }

  // Get the GPIO port for a given DDI ID on `platform`.
  static std::optional<GpioPort> GetForDdi(DdiId ddi_id, registers::Platform platform) {
    switch (platform) {
      case registers::Platform::kSkylake:
      case registers::Platform::kKabyLake:
      case registers::Platform::kTestDevice:
        return GetForDdiSkylake(ddi_id, platform);
      case registers::Platform::kTigerLake:
        return GetForDdiTigerLake(ddi_id, platform);
    }
  }

  // Returns whether a DDI has a valid GPIO port on `platform`.
  static bool HasValidPort(DdiId ddi_id, registers::Platform platform) {
    switch (platform) {
      case registers::Platform::kSkylake:
      case registers::Platform::kKabyLake:
      case registers::Platform::kTestDevice:
        return HasValidPortSkylake(ddi_id);
      case registers::Platform::kTigerLake:
        return HasValidPortTigerLake(ddi_id);
    }
  }

 private:
  GpioPort(int number, registers::Platform platform) : number_(number), platform_(platform) {}

  // Helper method to convert given GPIO port to its corresponding DDI ID
  // for Tiger Lake.
  //
  // The DDI <-> GPIO port mapping is available at:
  // Tiger Lake: IHD-OS-TGL-Vol 12-1.22-Rev 2.0 "Pin Usage", Page 423
  DdiId ToDdiIdTigerLake() const {
    ZX_DEBUG_ASSERT(platform_ == registers::Platform::kTigerLake);
    switch (number_) {
      case 1:
      case 2:
      case 3:
        return static_cast<DdiId>(DdiId::DDI_A + (number_ - 1));
      case 9:
      case 10:
      case 11:
      case 12:
      case 13:
      case 14:
        return static_cast<DdiId>(DdiId::DDI_TC_1 + (number_ - 9));
      default:
        ZX_ASSERT_MSG(false, "Invalid GPIO pin pair selected: %d", number_);
    }
  }

  // Helper method to convert given GPIO port to its corresponding DDI ID
  // for Skylake / Kaby Lake.
  //
  // The DDI <-> GPIO port mapping is available at:
  // Kaby Lake: IHD-OS-KBL-Vol 12-1.17 "Pin Usage", Page 198
  // Skylake: IHD-OS-SKL-Vol 12-05.16 "Pin Usage", Page 190
  DdiId ToDdiIdSkylake() const {
    ZX_DEBUG_ASSERT(platform_ == registers::Platform::kSkylake ||
                    platform_ == registers::Platform::kKabyLake ||
                    platform_ == registers::Platform::kTestDevice);
    switch (number_) {
      case 3:
        return DdiId::DDI_C;
      case 4:
        return DdiId::DDI_B;
      case 5:
        return DdiId::DDI_D;
      default:
        ZX_ASSERT_MSG(false, "Invalid GPIO pin pair selected: %d", number_);
    }
  }

  // Helper method to get a GpioPort for a given DDI ID for Tiger Lake.
  // If the DDI is valid but not connected to GPIO pins, returns nullopt.
  //
  // The DDI <-> GPIO port mapping is available at:
  // Tiger Lake: IHD-OS-TGL-Vol 12-1.22-Rev 2.0 "Pin Usage", Page 423
  static std::optional<GpioPort> GetForDdiTigerLake(DdiId ddi_id, registers::Platform platform) {
    ZX_DEBUG_ASSERT(platform == registers::Platform::kTigerLake);
    switch (ddi_id) {
      case DdiId::DDI_A:
      case DdiId::DDI_B:
      case DdiId::DDI_C:
        return GpioPort{1 + (ddi_id - DdiId::DDI_A), platform};
      case DdiId::DDI_TC_1:
      case DdiId::DDI_TC_2:
      case DdiId::DDI_TC_3:
      case DdiId::DDI_TC_4:
      case DdiId::DDI_TC_5:
      case DdiId::DDI_TC_6:
        return GpioPort{9 + (ddi_id - DdiId::DDI_TC_1), platform};
      default:
        ZX_ASSERT_MSG(false, "Invalid DDI: %d", ddi_id);
        break;
    }
  }

  // Helper method to get a GpioPort for a given DDI ID for Skylake / Kaby Lake.
  // If the DDI is valid but not connected to GPIO pins, returns nullopt.
  //
  // The DDI <-> GPIO port mapping is available at:
  // Kaby Lake: IHD-OS-KBL-Vol 12-1.17 "Pin Usage", Page 198
  // Skylake: IHD-OS-SKL-Vol 12-05.16 "Pin Usage", Page 190
  static std::optional<GpioPort> GetForDdiSkylake(DdiId ddi_id, registers::Platform platform) {
    ZX_DEBUG_ASSERT(platform == registers::Platform::kSkylake ||
                    platform == registers::Platform::kKabyLake ||
                    platform == registers::Platform::kTestDevice);
    switch (ddi_id) {
      case DdiId::DDI_B:
        return GpioPort{4, platform};
      case DdiId::DDI_C:
        return GpioPort{3, platform};
      case DdiId::DDI_D:
        return GpioPort{5, platform};
      case DdiId::DDI_A:
      case DdiId::DDI_E:
        return std::nullopt;
      default:
        ZX_ASSERT_MSG(false, "Invalid DDI: %d", ddi_id);
        break;
    }
  }

  // Returns if a DDI is connected to a valid GPIO port in Tiger Lake.
  static bool HasValidPortTigerLake(DdiId ddi_id) {
    switch (ddi_id) {
      case DdiId::DDI_A:
      case DdiId::DDI_B:
      case DdiId::DDI_C:
      case DdiId::DDI_TC_1:
      case DdiId::DDI_TC_2:
      case DdiId::DDI_TC_3:
      case DdiId::DDI_TC_4:
      case DdiId::DDI_TC_5:
      case DdiId::DDI_TC_6:
        return true;
      default:
        return false;
    }
  }

  // Returns if a DDI is connected to a valid GPIO port in Skylake / Kaby Lake.
  static bool HasValidPortSkylake(DdiId ddi_id) {
    switch (ddi_id) {
      case DdiId::DDI_B:
      case DdiId::DDI_C:
      case DdiId::DDI_D:
        return true;
      default:
        return false;
    }
  }

  int number_;
  registers::Platform platform_;
};

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_I2C_GMBUS_GPIO_H_
