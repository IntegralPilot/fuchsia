// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_MANAGER_DUMP_PARSED_DUMP_PARSED_H_
#define LIB_DRIVER_DEVICETREE_MANAGER_DUMP_PARSED_DUMP_PARSED_H_

#include <fcntl.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/devicetree/manager/node.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <iostream>
#include <sstream>
#include <vector>

#include "lib/driver/devicetree/manager/publisher-host.h"

namespace fdf_devicetree {

inline std::string StringifyIrqMode(fuchsia_hardware_platform_bus::ZirconInterruptMode mode) {
  switch (mode) {
    case fuchsia_hardware_platform_bus::ZirconInterruptMode::kDefault:
      return "DEFAULT";
    case fuchsia_hardware_platform_bus::ZirconInterruptMode::kEdgeLow:
      return "EDGE_LOW";
    case fuchsia_hardware_platform_bus::ZirconInterruptMode::kEdgeHigh:
      return "EDGE_HIGH";
    case fuchsia_hardware_platform_bus::ZirconInterruptMode::kLevelLow:
      return "LEVEL_LOW";
    case fuchsia_hardware_platform_bus::ZirconInterruptMode::kLevelHigh:
      return "LEVEL_HIGH";
    case fuchsia_hardware_platform_bus::ZirconInterruptMode::kEdgeBoth:
      return "EDGE_BOTH";
    default:
      return "UNKNOWN (" + std::to_string(static_cast<uint32_t>(mode)) + ")";
  }
}

// These values are based on the legacy Banjo protocol IDs used by devicetree
// (see src/lib/ddk/include/lib/ddk/protodefs.h).
// Ideally we should use generated FIDL bindings, but these are maintained
// manually for the dumper tool as it is simpler.
inline std::string StringifyProtocol(uint64_t val) {
  std::string name;
  switch (val) {
    case 1:
      name = "BLOCK";
      break;
    case 20:
      name = "GPIO";
      break;
    case 24:
      name = "I2C";
      break;
    case 31:
      name = "PCI";
      break;
    case 33:
      name = "USB";
      break;
    case 47:
      name = "BT_HCI";
      break;
    case 54:
      name = "SDHCI";
      break;
    case 55:
      name = "SDMMC";
      break;
    case 72:
      name = "POWER";
      break;
    case 85:
      name = "PDEV";
      break;
    case 90:
      name = "CLOCK";
      break;
    case 121:
      name = "SPI";
      break;
    case 129:
      name = "CPU_CTRL";
      break;
    case 147:
      name = "ADC";
      break;
    case 152:
      name = "REGISTERS";
      break;
    case 153:
      name = "DAI";
      break;
    case 171:
      name = "AUDIO_COMPOSITE";
      break;
    default:
      return std::to_string(val);
  }
  return name + " (" + std::to_string(val) + ")";
}

inline std::string StringifyVid(uint64_t val) {
  std::string name;
  switch (val) {
    case PDEV_VID_GENERIC:
      name = "GENERIC";
      break;
    case PDEV_VID_QEMU:
      name = "QEMU";
      break;
    case PDEV_VID_GOOGLE:
      name = "GOOGLE";
      break;
    case PDEV_VID_AMLOGIC:
      name = "AMLOGIC";
      break;
    case PDEV_VID_BROADCOM:
      name = "BROADCOM";
      break;
    case PDEV_VID_NXP:
      name = "NXP";
      break;
    case PDEV_VID_QUALCOMM:
      name = "QUALCOMM";
      break;
    default:
      return std::to_string(val);
  }
  return name + " (" + std::to_string(val) + ")";
}

inline std::string StringifyPid(uint64_t vid, uint64_t val) {
  std::string name;
  if (vid == PDEV_VID_GOOGLE) {
    switch (val) {
      case PDEV_PID_GAUSS:
        name = "GAUSS";
        break;
      case PDEV_PID_MACHINA:
        name = "MACHINA";
        break;
      case PDEV_PID_ASTRO:
        name = "ASTRO";
        break;
      case PDEV_PID_SHERLOCK:
        name = "SHERLOCK";
        break;
      case PDEV_PID_CLEO:
        name = "CLEO";
        break;
      case PDEV_PID_EAGLE:
        name = "EAGLE";
        break;
      case PDEV_PID_VISALIA:
        name = "VISALIA";
        break;
      case PDEV_PID_C18:
        name = "C18";
        break;
      case PDEV_PID_NELSON:
        name = "NELSON";
        break;
      case PDEV_PID_VS680_EVK:
        name = "VS680_EVK";
        break;
      case PDEV_PID_LUIS:
        name = "LUIS";
        break;
      case PDEV_PID_GOLDFISH:
        name = "GOLDFISH";
        break;
      case PDEV_PID_MOTMOT:
        name = "MOTMOT";
        break;
      case PDEV_PID_AV400:
        name = "AV400";
        break;
      case PDEV_PID_PINECREST:
        name = "PINECREST";
        break;
      case PDEV_PID_CLOVER:
        name = "CLOVER";
        break;
      case PDEV_PID_VIOLET:
        name = "VIOLET";
        break;
      case PDEV_PID_KOLA:
        name = "KOLA";
        break;
      case PDEV_PID_IRIS:
        name = "IRIS";
        break;
      default:
        break;
    }
  }
  return name.empty() ? std::to_string(val) : name + " (" + std::to_string(val) + ")";
}

inline std::string StringifyDid(uint64_t vid, uint64_t pid, uint64_t val) {
  std::string name;
  if (vid == PDEV_VID_GENERIC && pid == PDEV_PID_GENERIC) {
    switch (val) {
      case PDEV_DID_DEVICETREE_NODE:
        name = "DEVICETREE";
        break;
      default:
        break;
    }
  }
  return name.empty() ? std::to_string(val) : name + " (" + std::to_string(val) + ")";
}

inline uint32_t GetVid(const std::vector<fuchsia_driver_framework::NodeProperty2>& props) {
  for (const auto& prop : props) {
    if (prop.key() == "fuchsia.BIND_PLATFORM_DEV_VID" &&
        prop.value().Which() == fuchsia_driver_framework::NodePropertyValue::Tag::kIntValue) {
      return prop.value().int_value().value();
    }
  }
  return 0;
}

inline uint32_t GetPid(const std::vector<fuchsia_driver_framework::NodeProperty2>& props) {
  for (const auto& prop : props) {
    if (prop.key() == "fuchsia.BIND_PLATFORM_DEV_PID" &&
        prop.value().Which() == fuchsia_driver_framework::NodePropertyValue::Tag::kIntValue) {
      return prop.value().int_value().value();
    }
  }
  return 0;
}

inline uint32_t GetVid(const std::vector<fuchsia_driver_framework::BindRule2>& rules) {
  for (const auto& rule : rules) {
    if (rule.key() == "fuchsia.BIND_PLATFORM_DEV_VID") {
      for (const auto& val : rule.values()) {
        if (val.Which() == fuchsia_driver_framework::NodePropertyValue::Tag::kIntValue) {
          return val.int_value().value();
        }
      }
    }
  }
  return 0;
}

inline uint32_t GetPid(const std::vector<fuchsia_driver_framework::BindRule2>& rules) {
  for (const auto& rule : rules) {
    if (rule.key() == "fuchsia.BIND_PLATFORM_DEV_PID") {
      for (const auto& val : rule.values()) {
        if (val.Which() == fuchsia_driver_framework::NodePropertyValue::Tag::kIntValue) {
          return val.int_value().value();
        }
      }
    }
  }
  return 0;
}

inline std::string StringifyPropertyValue(const fuchsia_driver_framework::NodePropertyValue& value,
                                          const std::string& key = "", uint32_t vid = 0,
                                          uint32_t pid = 0) {
  if (value.Which() == fuchsia_driver_framework::NodePropertyValue::Tag::kIntValue) {
    uint64_t val = value.int_value().value();
    if (key == "fuchsia.BIND_PROTOCOL") {
      return StringifyProtocol(val);
    }
    if (key == "fuchsia.BIND_PLATFORM_DEV_VID") {
      return StringifyVid(val);
    }
    if (key == "fuchsia.BIND_PLATFORM_DEV_PID") {
      return StringifyPid(vid, val);
    }
    if (key == "fuchsia.BIND_PLATFORM_DEV_DID") {
      return StringifyDid(vid, pid, val);
    }
  }

  switch (value.Which()) {
    case fuchsia_driver_framework::NodePropertyValue::Tag::kBoolValue:
      return value.bool_value().value() ? "true" : "false";
    case fuchsia_driver_framework::NodePropertyValue::Tag::kIntValue:
      return std::to_string(value.int_value().value());
    case fuchsia_driver_framework::NodePropertyValue::Tag::kEnumValue:
      return value.enum_value().value();
    case fuchsia_driver_framework::NodePropertyValue::Tag::kStringValue:
      return "\"" + value.string_value().value() + "\"";
    default:
      return "<unknown>";
  }
}

inline void StringifyProperties(const std::vector<fuchsia_driver_framework::NodeProperty2>& props,
                                std::ostream& os, const std::string& indent) {
  uint32_t vid = GetVid(props);
  uint32_t pid = GetPid(props);

  for (const auto& prop : props) {
    os << indent << "property { " << prop.key() << " = ";
    os << StringifyPropertyValue(prop.value(), prop.key(), vid, pid) << " }\n";
  }
}

inline void StringifyParentSpec(const fuchsia_driver_framework::ParentSpec2& spec, std::ostream& os,
                                const std::string& indent) {
  if (spec.bind_rules().empty() && spec.properties().empty()) {
    return;
  }

  uint32_t vid = GetVid(spec.bind_rules());
  if (vid == 0) {
    vid = GetVid(spec.properties());
  }
  uint32_t pid = GetPid(spec.bind_rules());
  if (pid == 0) {
    pid = GetPid(spec.properties());
  }

  os << indent << "bind_rules {\n";
  for (const auto& rule : spec.bind_rules()) {
    os << indent << "  ";
    switch (rule.condition()) {
      case fuchsia_driver_framework::Condition::kAccept:
        os << "Accept";
        break;
      case fuchsia_driver_framework::Condition::kReject:
        os << "Reject";
        break;
      default:
        os << "Unknown";
    }
    os << "(" << rule.key() << ", [";
    for (size_t i = 0; i < rule.values().size(); ++i) {
      os << StringifyPropertyValue(rule.values()[i], rule.key(), vid, pid)
         << (i == rule.values().size() - 1 ? "" : ", ");
    }
    os << "])\n";
  }
  os << indent << "}\n";

  if (!spec.properties().empty()) {
    os << indent << "properties {\n";
    StringifyProperties(spec.properties(), os, indent + "  ");
    os << indent << "}\n";
  }
}

inline std::vector<uint8_t> LoadBlob(const std::string& path) {
  int fd = open(path.c_str(), O_RDONLY);
  if (fd < 0) {
    std::cerr << "Failed to open " << path << ": " << strerror(errno) << std::endl;
    exit(1);
  }

  struct stat stat_out;
  if (fstat(fd, &stat_out) < 0) {
    std::cerr << "Failed to fstat " << path << ": " << strerror(errno) << std::endl;
    exit(1);
  }

  std::vector<uint8_t> vec(static_cast<size_t>(stat_out.st_size));
  size_t total_read = 0;
  while (total_read < vec.size()) {
    ssize_t bytes_read = read(fd, vec.data() + total_read, vec.size() - total_read);
    if (bytes_read < 0) {
      if (errno == EINTR) {
        continue;
      }
      std::cerr << "Failed to read " << path << ": " << strerror(errno) << std::endl;
      exit(1);
    }
    if (bytes_read == 0) {
      break;
    }
    total_read += static_cast<size_t>(bytes_read);
  }

  vec.resize(total_read);
  close(fd);
  return vec;
}

template <typename T>
inline std::string StringifyFidl(const T& val) {
  return "<not implemented>";
}

template <>
inline std::string StringifyFidl<fuchsia_hardware_platform_bus::Iommu>(
    const fuchsia_hardware_platform_bus::Iommu& iommu) {
  std::stringstream ss;
  switch (iommu.Which()) {
    case fuchsia_hardware_platform_bus::Iommu::Tag::kStubIommu:
      ss << "stub_iommu: {}";
      break;
    case fuchsia_hardware_platform_bus::Iommu::Tag::kArmSmmu:
      ss << "arm_smmu: { base_address: 0x" << std::hex << iommu.arm_smmu()->base_address()
         << std::dec << " }";
      break;
    default:
      ss << "<unknown union variant>";
  }
  return ss.str();
}

template <>
inline std::string StringifyFidl<fuchsia_driver_framework::BusInfo>(
    const fuchsia_driver_framework::BusInfo& val) {
  std::stringstream ss;
  ss << "{ ";
  if (val.bus().has_value()) {
    ss << "bus: " << static_cast<uint32_t>(*val.bus()) << " ";
  }
  ss << "}";
  return ss.str();
}

inline void StringifyPbusNode(const fuchsia_hardware_platform_bus::Node& node,
                              const std::vector<std::optional<std::string>>& metadata_text,
                              const std::vector<std::optional<std::string>>& power_config_text,
                              std::ostream& os) {
  std::string name = node.name() ? *node.name() : "unnamed";
  os << "  \"" << name << "\" {\n";
  if (node.vid()) {
    os << "    vid = " << fdf_devicetree::StringifyVid(*node.vid()) << "\n";
  }
  if (node.pid()) {
    os << "    pid = " << fdf_devicetree::StringifyPid(node.vid().value_or(0), *node.pid()) << "\n";
  }
  if (node.did()) {
    os << "    did = "
       << fdf_devicetree::StringifyDid(node.vid().value_or(0), node.pid().value_or(0), *node.did())
       << "\n";
  }
  if (node.instance_id()) {
    os << "    instance_id = " << *node.instance_id() << "\n";
  }
  if (node.interrupt_controller_id()) {
    os << "    interrupt_controller_id = " << *node.interrupt_controller_id() << "\n";
  }

  if (node.mmio() && !node.mmio()->empty()) {
    os << "    mmios {\n";
    for (const auto& mmio : *node.mmio()) {
      os << "      mmio {\n";
      if (mmio.base()) {
        os << "        base = 0x" << std::hex << *mmio.base() << std::dec << "\n";
      }
      if (mmio.length()) {
        os << "        length = 0x" << std::hex << *mmio.length() << std::dec << "\n";
      }
      if (mmio.name()) {
        os << "        name = \"" << *mmio.name() << "\"\n";
      }
      os << "      }\n";
    }
    os << "    }\n";
  }

  if (node.irq() && !node.irq()->empty()) {
    for (const auto& irq : *node.irq()) {
      os << "    irq {\n";
      if (irq.irq()) {
        if (irq.irq()->irq()) {
          os << "      number = " << irq.irq()->irq().value() << "\n";
        }
        if (irq.irq()->userspace_irq()) {
          os << "      number = " << irq.irq()->userspace_irq()->irq() << "\n";
          os << "      controller = " << irq.irq()->userspace_irq()->controller_id() << "\n";
        }
      }
      if (irq.mode()) {
        os << "      mode = " << StringifyIrqMode(*irq.mode()) << "\n";
      }
      if (irq.wake_vector()) {
        os << "      wake_vector = " << (*irq.wake_vector() ? "true" : "false") << "\n";
      }
      if (irq.name()) {
        os << "      name = \"" << *irq.name() << "\"\n";
      }
      if (irq.properties() && !irq.properties()->empty()) {
        os << "      properties {\n";
        fdf_devicetree::StringifyProperties(*irq.properties(), os, "        ");
        os << "      }\n";
      }
      os << "    }\n";
    }
  }

  if (node.bti() && !node.bti()->empty()) {
    for (const auto& bti : *node.bti()) {
      os << "    bti {\n";
      if (bti.iommu_id()) {
        os << "      iommu_id = " << *bti.iommu_id() << "\n";
      }
      if (bti.bti_id()) {
        os << "      bti_id = " << *bti.bti_id() << "\n";
      }
      if (bti.name()) {
        os << "      name = \"" << *bti.name() << "\"\n";
      }
      os << "    }\n";
    }
  }

  if (node.metadata() && !node.metadata()->empty()) {
    for (size_t i = 0; i < node.metadata()->size(); ++i) {
      const auto& md = (*node.metadata())[i];
      os << "    metadata {\n";
      if (md.id()) {
        os << "      id = \"" << *md.id() << "\"\n";
      }
      if (i < metadata_text.size() && metadata_text[i]) {
        os << "      data = { " << *metadata_text[i] << " }\n";
      } else if (md.data().has_value()) {
        os << "      data = [ <binary> size: " << md.data()->size() << " bytes ]\n";
      } else {
        os << "      data = [ <binary> ]\n";
      }
      os << "    }\n";
    }
  }

  if (node.power_config() && !node.power_config()->empty()) {
    for (size_t i = 0; i < node.power_config()->size(); ++i) {
      os << "    power_config {\n";
      if (i < power_config_text.size() && power_config_text[i]) {
        os << "      " << *power_config_text[i] << "\n";
      } else {
        os << "      <binary>\n";
      }
      os << "    }\n";
    }
  }

  if (node.smc() && !node.smc()->empty()) {
    for (const auto& smc : *node.smc()) {
      os << "    smc {\n";
      if (smc.service_call_num_base()) {
        os << "      base = " << *smc.service_call_num_base() << "\n";
      }
      if (smc.count()) {
        os << "      count = " << *smc.count() << "\n";
      }
      if (smc.exclusive()) {
        os << "      exclusive = " << (*smc.exclusive() ? "true" : "false") << "\n";
      }
      if (smc.name()) {
        os << "      name = \"" << *smc.name() << "\"\n";
      }
      os << "    }\n";
    }
  }

  if (node.boot_metadata() && !node.boot_metadata()->empty()) {
    for (const auto& bm : *node.boot_metadata()) {
      os << "    boot_metadata {\n";
      if (bm.zbi_type()) {
        os << "      zbi_type = " << *bm.zbi_type() << "\n";
      }
      if (bm.zbi_extra()) {
        os << "      zbi_extra = " << *bm.zbi_extra() << "\n";
      }
      os << "    }\n";
    }
  }

  if (node.properties() && !node.properties()->empty()) {
    fdf_devicetree::StringifyProperties(*node.properties(), os, "    ");
  }

  if (node.driver_host()) {
    os << "    driver_host = \"" << *node.driver_host() << "\"\n";
  }

  os << "  }\n";
}

inline void StringifyBoardChildNode(const fdf_devicetree::BoardChildNode& node, std::ostream& os) {
  os << "  \"" << node.name << "\" {\n";
  if (node.driver_host) {
    os << "    driver_host = \"" << *node.driver_host << "\"\n";
  }
  if (node.bus_info) {
    os << "    bus_info = " << fdf_devicetree::StringifyFidl(*node.bus_info) << "\n";
  }
  if (!node.properties.empty()) {
    fdf_devicetree::StringifyProperties(node.properties, os, "    ");
  }
  os << "  }\n";
}

inline void StringifyCompositeNodeSpec(const fdf_devicetree::CompositeNodeSpecInfo& spec_meta,
                                       std::ostream& os) {
  const auto& spec = spec_meta.spec;
  std::string name = spec.name() ? *spec.name() : "unnamed";
  os << "  \"" << name << "\" {\n";
  if (spec.parents2()) {
    for (const auto& parent : *spec.parents2()) {
      os << "    parent {\n";
      fdf_devicetree::StringifyParentSpec(parent, os, "      ");
      os << "    }\n";
    }
  }
  if (spec_meta.driver_host) {
    os << "    driver_host = \"" << *spec_meta.driver_host << "\"\n";
  }
  os << "  }\n";
}

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_MANAGER_DUMP_PARSED_DUMP_PARSED_H_
