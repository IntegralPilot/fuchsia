// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_UIC_UIC_COMMANDS_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_UIC_UIC_COMMANDS_H_

#include <lib/zx/result.h>
#include <zircon/types.h>

#include <optional>

namespace ufs {

// MIPI UniPro specification v2.0, section 5.8.1 "PHY Adapter Common Attributes"
constexpr uint16_t PA_AvailTxDataLanes = 0x1520;
constexpr uint16_t PA_AvailRxDataLanes = 0x1540;

// MIPI UniPro specification v2.0, section 5.8.2 "PHY Adapter M-PHY-Specific Attributes"
constexpr uint16_t PA_ConnectedTxDataLanes = 0x1561;
constexpr uint16_t PA_ConnectedRxDataLanes = 0x1581;
constexpr uint16_t PA_MaxRxHSGear = 0x1587;
constexpr uint16_t PA_Granularity = 0x15AA;
constexpr uint16_t PA_TActivate = 0x15A8;

constexpr uint16_t PA_RemoteVerInfo = 0x15A0;
constexpr uint16_t PA_LocalVerInfo = 0x15A9;

// UFSHCI Specification Version 3.0, section 7.4 "UIC Power Mode Change".
constexpr uint16_t PA_ActiveTxDataLanes = 0x1560;
constexpr uint16_t PA_ActiveRxDataLanes = 0x1580;
constexpr uint16_t PA_TxGear = 0x1568;
constexpr uint16_t PA_RxGear = 0x1583;
constexpr uint16_t PA_TxTermination = 0x1569;
constexpr uint16_t PA_RxTermination = 0x1584;
constexpr uint16_t PA_HSSeries = 0x156A;
constexpr uint16_t PA_PWRModeUserData0 = 0x15B0;
constexpr uint16_t PA_PWRModeUserData1 = 0x15B1;
constexpr uint16_t PA_PWRModeUserData2 = 0x15B2;
constexpr uint16_t PA_PWRModeUserData3 = 0x15B3;
constexpr uint16_t PA_PWRModeUserData4 = 0x15B4;
constexpr uint16_t PA_PWRModeUserData5 = 0x15B5;

constexpr uint16_t PA_TxHsAdaptType = 0x15D4;
constexpr uint16_t PA_PWRMode = 0x1571;

constexpr uint16_t DME_LocalFC0ProtectionTimeOutVal = 0xD041;
constexpr uint16_t DME_LocalTC0ReplayTimeOutVal = 0xD042;
constexpr uint16_t DME_LocalAFC0ReqTimeOutVal = 0xD043;

// Currently, UFS uses TC0 only.
constexpr uint16_t DL_FC0ProtectionTimeOutVal_Default = 8191;
constexpr uint16_t DL_TC0ReplayTimeOutVal_Default = 65535;
constexpr uint16_t DL_AFC0ReqTimeOutVal_Default = 32767;

constexpr uint32_t kUicTimeoutUsec = 5000000;

enum class UicCommandOpcode {
  // Configuration
  kDmeGet = 0x01,
  kDmeSet = 0x02,
  kDmePeerGet = 0x03,
  kDmePeerSet = 0x04,
  // Control
  kDmePowerOn = 0x10,
  kDmePowerOff = 0x11,
  kDmeEnable = 0x12,
  kDmeReset = 0x14,
  kDmeEndpointReset = 0x15,
  kDmeLinkStartUp = 0x16,
  kDmeHibernateEnter = 0x17,
  kDmeHibernateExit = 0x18,
  kDmeTestMode = 0x1a,
};

enum class AttrSetType : uint8_t {
  kVolatile = 0,
  kStatic = 1,
};

class Ufs;

// UFS Specification Version 3.1, section 9.4 "UniPro/UFS Control Interface (Control Plane)".
class UicCommand {
 public:
  virtual ~UicCommand() = default;
  UicCommand() = delete;
  explicit UicCommand(const UicCommand &) = delete;
  UicCommand &operator=(const UicCommand &) = delete;
  UicCommand(UicCommand &&) = delete;
  UicCommand &operator=(UicCommand &&) = delete;
  // Among the UIC commands, the DME_GET and DME_PEER_GET commands return the value of the attribute
  // requested in the UICCMDARG3 register. The other commands do not return a value.
  zx::result<std::optional<uint32_t>> SendCommand();

  UicCommandOpcode GetOpcode() const { return opcode_; }

  // For testing
  void SetTimeoutUsec(uint32_t time) { timeout_usec_ = time; }

 protected:
  explicit UicCommand(Ufs &ufs, UicCommandOpcode opcode, uint16_t mib_attribute = 0,
                      uint16_t gen_selector_index = 0)
      : controller_(ufs),
        opcode_(opcode),
        mib_attribute_(mib_attribute),
        gen_selector_index_(gen_selector_index) {}

  zx::result<> SendUicCommand();

  Ufs &GetController() { return controller_; }
  uint16_t GetMibAttribute() const { return mib_attribute_; }
  uint16_t GetGenSelectorIndex() const { return gen_selector_index_; }
  uint32_t GetTimeoutUsec() const { return timeout_usec_; }

  virtual zx::result<> UicPreProcess() { return zx::ok(); }
  virtual std::tuple<uint32_t, uint32_t, uint32_t> Arguments() const {
    return std::make_tuple(0, 0, 0);
  }
  // Among the UIC commands, the DME_RESET command does nothing on |UicPostProcess()|.
  virtual zx::result<> UicPostProcess();
  virtual std::optional<uint32_t> ReturnValue() { return std::nullopt; }

 private:
  Ufs &controller_;
  const UicCommandOpcode opcode_;
  const uint16_t mib_attribute_ = 0;
  const uint16_t gen_selector_index_ = 0;

  uint32_t timeout_usec_ = kUicTimeoutUsec;
};

class DmeGetUicCommand : public UicCommand {
 public:
  explicit DmeGetUicCommand(Ufs &ufs, uint16_t mib_attribute, uint16_t gen_selector_index)
      : UicCommand(ufs, UicCommandOpcode::kDmeGet, mib_attribute, gen_selector_index) {}

 protected:
  std::tuple<uint32_t, uint32_t, uint32_t> Arguments() const override;
  std::optional<uint32_t> ReturnValue() override;
};

class DmeSetUicCommand : public UicCommand {
 public:
  explicit DmeSetUicCommand(Ufs &ufs, uint16_t mib_attribute, uint16_t gen_selector_index,
                            AttrSetType attr_set_type, uint32_t value)
      : UicCommand(ufs, UicCommandOpcode::kDmeSet, mib_attribute, gen_selector_index),
        attr_set_type_(attr_set_type),
        value_(value) {}

 protected:
  std::tuple<uint32_t, uint32_t, uint32_t> Arguments() const override;

 private:
  AttrSetType attr_set_type_;
  uint32_t value_;
};

class DmePeerGetUicCommand : public UicCommand {
 public:
  explicit DmePeerGetUicCommand(Ufs &ufs, uint16_t mib_attribute, uint16_t gen_selector_index)
      : UicCommand(ufs, UicCommandOpcode::kDmePeerGet, mib_attribute, gen_selector_index) {}

 protected:
  std::tuple<uint32_t, uint32_t, uint32_t> Arguments() const override;
  std::optional<uint32_t> ReturnValue() override;
};

class DmePeerSetUicCommand : public UicCommand {
 public:
  explicit DmePeerSetUicCommand(Ufs &ufs, uint16_t mib_attribute, uint16_t gen_selector_index,
                                AttrSetType attr_set_type, uint32_t value)
      : UicCommand(ufs, UicCommandOpcode::kDmePeerSet, mib_attribute, gen_selector_index),
        attr_set_type_(attr_set_type),
        value_(value) {}

 protected:
  std::tuple<uint32_t, uint32_t, uint32_t> Arguments() const override;

 private:
  AttrSetType attr_set_type_;
  uint32_t value_;
};

class DmeLinkStartUpUicCommand : public UicCommand {
 public:
  explicit DmeLinkStartUpUicCommand(Ufs &ufs)
      : UicCommand(ufs, UicCommandOpcode::kDmeLinkStartUp) {}
  zx::result<> UicPostProcess() override;
};

class DmeHibernateCommand : public UicCommand {
 public:
  explicit DmeHibernateCommand(Ufs &ufs, UicCommandOpcode opcode) : UicCommand(ufs, opcode) {}

 protected:
  zx::result<> UicPostProcess() override;
  virtual uint32_t GetFlag() = 0;
};

class DmeHibernateEnterCommand : public DmeHibernateCommand {
 public:
  explicit DmeHibernateEnterCommand(Ufs &ufs)
      : DmeHibernateCommand(ufs, UicCommandOpcode::kDmeHibernateEnter) {}

 protected:
  uint32_t GetFlag() override;
};

class DmeHibernateExitCommand : public DmeHibernateCommand {
 public:
  explicit DmeHibernateExitCommand(Ufs &ufs)
      : DmeHibernateCommand(ufs, UicCommandOpcode::kDmeHibernateExit) {}

 protected:
  uint32_t GetFlag() override;
};

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_UIC_UIC_COMMANDS_H_
