// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_PCI_LIB_CPP_INCLUDE_LIB_PCI_HW_H_
#define SRC_DEVICES_PCI_LIB_CPP_INCLUDE_LIB_PCI_HW_H_

#include <stdint.h>

namespace pci {

// clang-format off

/*
 * PCI configuration space offsets
 */
inline constexpr uint8_t kConfigVendorId        = 0x00;
inline constexpr uint8_t kConfigDeviceId        = 0x02;
inline constexpr uint8_t kConfigCommand         = 0x04;
inline constexpr uint8_t kConfigStatus          = 0x06;
inline constexpr uint8_t kConfigRevisionId      = 0x08;
inline constexpr uint8_t kConfigClassCode       = 0x09;
inline constexpr uint8_t kConfigClassCodeIntr   = 0x09;
inline constexpr uint8_t kConfigClassCodeSub    = 0x0a;
inline constexpr uint8_t kConfigClassCodeBase   = 0x0b;
inline constexpr uint8_t kConfigCacheLineSize   = 0x0c;
inline constexpr uint8_t kConfigLatencyTimer    = 0x0d;
inline constexpr uint8_t kConfigHeaderType      = 0x0e;
inline constexpr uint8_t kConfigBist            = 0x0f;
inline constexpr uint8_t kConfigBaseAddresses   = 0x10;
inline constexpr uint8_t kConfigCardbusCisPtr   = 0x28;
inline constexpr uint8_t kConfigSubsysVendorId  = 0x2c;
inline constexpr uint8_t kConfigSubsysId        = 0x2e;
inline constexpr uint8_t kConfigExpRomAddress   = 0x30;
inline constexpr uint8_t kConfigCapabilities    = 0x34;
inline constexpr uint8_t kConfigInterruptLine   = 0x3c;
inline constexpr uint8_t kConfigInterruptPin    = 0x3d;
inline constexpr uint8_t kConfigMinGrant        = 0x3e;
inline constexpr uint8_t kConfigMaxLatency      = 0x3f;

/*
 * PCI header type register bits
 */
inline constexpr uint8_t kHeaderTypeMask        = 0x7f;
inline constexpr uint8_t kHeaderTypeMultiFn     = 0x80;

/*
 * PCI header types
 */
inline constexpr uint8_t kHeaderTypeStandard    = 0x00;
inline constexpr uint8_t kHeaderTypePciBridge   = 0x01;
inline constexpr uint8_t kHeaderTypeCardBus     = 0x02;

/*
 * PCI command register bits
 */
inline constexpr uint16_t kCommandIoEn           = 0x0001;
inline constexpr uint16_t kCommandMemEn          = 0x0002;
inline constexpr uint16_t kCommandBusMasterEn    = 0x0004;
inline constexpr uint16_t kCommandSpecialEn      = 0x0008;
inline constexpr uint16_t kCommandMemWrInvEn     = 0x0010;
inline constexpr uint16_t kCommandPalSnoopEn     = 0x0020;
inline constexpr uint16_t kCommandPerrRespEn     = 0x0040;
inline constexpr uint16_t kCommandAdStepEn       = 0x0080;
inline constexpr uint16_t kCommandSerrEn         = 0x0100;
inline constexpr uint16_t kCommandFastB2bEn      = 0x0200;
inline constexpr uint16_t kCommandIntDisable     = 0x0400;

/*
 * PCI status register bits
 */
inline constexpr uint16_t kStatusInterrupt       = 0x0008;
inline constexpr uint16_t kStatusNewCaps         = 0x0010;
inline constexpr uint16_t kStatus66Mhz           = 0x0020;
inline constexpr uint16_t kStatusFastB2b         = 0x0080;
inline constexpr uint16_t kStatusMstrPerr        = 0x0100;
inline constexpr uint16_t kStatusDevselMask      = 0x0600;
inline constexpr uint16_t kStatusTargAbortSig    = 0x0800;
inline constexpr uint16_t kStatusTargAbortRcv    = 0x1000;
inline constexpr uint16_t kStatusMstrAbortRcv    = 0x2000;
inline constexpr uint16_t kStatusSerrSig         = 0x4000;
inline constexpr uint16_t kStatusPerr            = 0x8000;

/*
 * PCI class codes
 */
inline constexpr uint8_t kClassLegacyDevice      = 0x00;
inline constexpr uint8_t kClassMassStorage       = 0x01;
inline constexpr uint8_t kClassNetwork           = 0x02;
inline constexpr uint8_t kClassDisplay           = 0x03;
inline constexpr uint8_t kClassMultimedia        = 0x04;
inline constexpr uint8_t kClassMemory            = 0x05;
inline constexpr uint8_t kClassBridge            = 0x06;
inline constexpr uint8_t kClassSimpleComm        = 0x07;
inline constexpr uint8_t kClassBasePeriph        = 0x08;
inline constexpr uint8_t kClassInput             = 0x09;
inline constexpr uint8_t kClassDock              = 0x0A;
inline constexpr uint8_t kClassProcessor         = 0x0B;
inline constexpr uint8_t kClassSerialBus         = 0x0C;
inline constexpr uint8_t kClassWireless          = 0x0D;
inline constexpr uint8_t kClassIntelligentIo     = 0x0E;
inline constexpr uint8_t kClassSatelliteComm     = 0x0F;
inline constexpr uint8_t kClassEncryption        = 0x10;
inline constexpr uint8_t kClassDataAcq           = 0x11;
inline constexpr uint8_t kClassUndefined         = 0x99;

/*
 * PCI subclasses by category
 */
// Mass storage
inline constexpr uint8_t kSubclassScsi           = 0x00;
inline constexpr uint8_t kSubclassIde            = 0x01;
inline constexpr uint8_t kSubclassFloppyDisk     = 0x02;
inline constexpr uint8_t kSubclassIpiBus         = 0x03;
inline constexpr uint8_t kSubclassRaidBus        = 0x04;
inline constexpr uint8_t kSubclassAta            = 0x05;
inline constexpr uint8_t kSubclassSata           = 0x06;
inline constexpr uint8_t kSubclassSerialScsi     = 0x07;
inline constexpr uint8_t kSubclassNvmem          = 0x08;
inline constexpr uint8_t kSubclassUfs            = 0x09;
inline constexpr uint8_t kSubclassMassStorage    = 0x80;
// Network
inline constexpr uint8_t kSubclassEthernet       = 0x00;
inline constexpr uint8_t kSubclassTokenRing      = 0x01;
inline constexpr uint8_t kSubclassFddi           = 0x02;
inline constexpr uint8_t kSubclassAtm            = 0x03;
inline constexpr uint8_t kSubclassIsdn           = 0x04;
inline constexpr uint8_t kSubclassWorldfip       = 0x05;
inline constexpr uint8_t kSubclassPicmg          = 0x06;
inline constexpr uint8_t kSubclassInfiniband     = 0x07;
inline constexpr uint8_t kSubclassFabric         = 0x08;
inline constexpr uint8_t kSubclassNetwork        = 0x80;
// Display
inline constexpr uint8_t kSubclassVga            = 0x00;
inline constexpr uint8_t kSubclassXga            = 0x01;
inline constexpr uint8_t kSubclass3D             = 0x02;
inline constexpr uint8_t kSubclassDisplay        = 0x80;
// Multimedia
inline constexpr uint8_t kSubclassVideoCtrl      = 0x00;
inline constexpr uint8_t kSubclassAudioCtrl      = 0x01;
inline constexpr uint8_t kSubclassTelephony      = 0x02;
inline constexpr uint8_t kSubclassAudioDevice    = 0x03;
inline constexpr uint8_t kSubclassMultimedia     = 0x80;
// Memory
inline constexpr uint8_t kSubclassRam            = 0x00;
inline constexpr uint8_t kSubclassFlash          = 0x01;
inline constexpr uint8_t kSubclassMemory         = 0x80;
// Bridge
inline constexpr uint8_t kSubclassHost           = 0x00;
inline constexpr uint8_t kSubclassIsa            = 0x01;
inline constexpr uint8_t kSubclassEisa           = 0x02;
inline constexpr uint8_t kSubclassMicrochannel   = 0x03;
inline constexpr uint8_t kSubclassPci            = 0x04;
inline constexpr uint8_t kSubclassPcmcia         = 0x05;
inline constexpr uint8_t kSubclassNubus          = 0x06;
inline constexpr uint8_t kSubclassCardbus        = 0x07;
inline constexpr uint8_t kSubclassRaceway        = 0x08;
inline constexpr uint8_t kSubclassPciToPci       = 0x09;
inline constexpr uint8_t kSubclassInfiPciHost    = 0x0A;
inline constexpr uint8_t kSubclassBridge         = 0x80;
// Communication
inline constexpr uint8_t kSubclassSerial         = 0x00;
inline constexpr uint8_t kSubclassParallel       = 0x01;
inline constexpr uint8_t kSubclassMultiSerial    = 0x02;
inline constexpr uint8_t kSubclassModem          = 0x03;
inline constexpr uint8_t kSubclassGpibCtrl       = 0x04;
inline constexpr uint8_t kSubclassSmartCard      = 0x05;
inline constexpr uint8_t kSubclassCommunication  = 0x80;
// Generic
inline constexpr uint8_t kSubclassPic            = 0x00;
inline constexpr uint8_t kSubclassDma            = 0x01;
inline constexpr uint8_t kSubclassTimer          = 0x02;
inline constexpr uint8_t kSubclassRtc            = 0x03;
inline constexpr uint8_t kSubclassPciHotplug     = 0x04;
inline constexpr uint8_t kSubclassSdHost         = 0x05;
inline constexpr uint8_t kSubclassIommu          = 0x06;
inline constexpr uint8_t kSubclassSystemPeriph   = 0x80;

// clang-format on

}  // namespace pci

#endif  // SRC_DEVICES_PCI_LIB_CPP_INCLUDE_LIB_PCI_HW_H_
