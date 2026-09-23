// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_DEVICETREE_H_
#define ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_DEVICETREE_H_

#include <lib/arch/arm64/system.h>
#include <lib/boot-shim/devicetree-boot-shim.h>
#include <lib/boot-shim/item-base.h>
#include <lib/boot-shim/tty.h>
#include <lib/boot-shim/watchdog.h>
#include <lib/devicetree/devicetree.h>
#include <lib/devicetree/matcher.h>
#include <lib/fit/function.h>
#include <lib/fit/result.h>
#include <lib/memalloc/range.h>
#include <lib/uart/all.h>
#include <lib/zbi-format/cpu.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zbi-format/memory.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/item.h>
#include <lib/zbitl/storage-traits.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <algorithm>
#include <array>
#include <concepts>
#include <cstdint>
#include <optional>
#include <source_location>
#include <span>
#include <string_view>
#include <type_traits>

#include <fbl/intrusive_hash_table.h>
#include <fbl/intrusive_single_list.h>
#include <fbl/type_info.h>

namespace boot_shim {

// Base class for DevicetreeItems, providing default implementations for the Matcher API.
// Derived classes MUST implement OnNode.
template <typename T, size_t MaxScans>
class DevicetreeItemBase {
 public:
  static constexpr size_t kMaxScans = MaxScans;
  static constexpr bool kMatchOkNodesOnly = true;

  constexpr DevicetreeItemBase() = default;
  constexpr DevicetreeItemBase(const char* shim_name, FILE* log)
      : log_(log), shim_name_(shim_name) {
    // Note: `T` is incomplete when this template instantiates, so at the time of template
    // instantiation `T` will not meet the API requirements of `Matcher<T>`. Deferring this to a
    // static_assert on a function body, delay's the evaluation of `Matcher<T>` until `T` is fully
    // defined.
    static_assert(devicetree::Matcher<T>);
  }

  void OnError(std::string_view error) {
    Log("Error on %s, %*s\n", fbl::TypeInfo<T>::Name(), static_cast<int>(error.length()),
        error.data());
  }

  devicetree::ScanState OnSubtree(const devicetree::NodePath&) {
    return devicetree::ScanState::kActive;
  }

  devicetree::ScanState OnScan() { return devicetree::ScanState::kActive; }

  void OnDone() {}

  template <typename Shim>
  void Init(const Shim& shim) {
    static_assert(devicetree::Matcher<T>);
    shim_name_ = shim.shim_name();
    log_ = shim.log();
  }

 protected:
  // Helper for logging in to |log_|.
  void Log(const char* fmt, ...) __PRINTFLIKE(2, 3) {
    fprintf(log_, "%s: ", shim_name_);
    va_list ap;
    va_start(ap, fmt);
    vfprintf(log_, fmt, ap);
    va_end(ap);
  }

 private:
  FILE* log_;
  const char* shim_name_;
};

// Helper class for decoding interrupt cells and obtaining IRQ numbers.
class DevicetreeIrqResolver {
 public:
  struct IrqConfig {
    uint32_t irq = 0;
    uint32_t flags = 0;
  };

  constexpr DevicetreeIrqResolver() = default;
  explicit constexpr DevicetreeIrqResolver(devicetree::ByteView bytes) : interrupt_bytes_(bytes) {}

  // Attempts to either resolve |interrupt-parent| property from the |decoder| hierarchy
  // or find the |interrupt-controller| along the way.
  //
  // On success with a return value |true|, the |interrupt-controller| node has been resolved,
  // On success with a return value |false|,the |interrupt-parent| was resolved but the
  // |interrupt-controller| has not. On failure, a malformed node or property has been encountered
  // and no further actions can be performed.
  fit::result<fit::failed, bool> ResolveIrqController(const devicetree::PropertyDecoder& decoder);

  // Obtains the IRQ number from the interrupt described by the |index|-th element in the interrupt
  // property.
  IrqConfig GetIrqConfig(size_t index) const {
    ZX_ASSERT(irq_resolver_);
    const size_t entry_size = *interrupt_cells_ * sizeof(uint32_t);
    return irq_resolver_(interrupt_bytes_.subspan(index * entry_size, entry_size),
                         *interrupt_cells_)
        .value_or(IrqConfig{.irq = 0, .flags = 0});
  }

  // May only be called after resolving the IRQ Controller, see |ResolveIrqController()|.
  size_t num_entries() const {
    ZX_ASSERT(interrupt_cells_);
    return (interrupt_bytes_.size() / sizeof(uint32_t)) / *interrupt_cells_;
  }

  // Returns whether additional scans are required to resolve the |interrupt_parent|.
  // This is only meaningful if |ResolveIrqController| returned false.
  bool NeedsInterruptParent() const { return !error_ && interrupt_parent_ && !irq_resolver_; }

 private:
  fit::inline_function<std::optional<IrqConfig>(devicetree::ByteView, uint32_t)> irq_resolver_;
  devicetree::ByteView interrupt_bytes_;
  std::optional<uint32_t> interrupt_parent_;
  std::optional<uint32_t> interrupt_cells_;
  bool error_ = false;
};

// Decodes PSCI information from a devicetree and synthesizes a
// DRIVER_CONFIG ZBI item for it.
//
// A PSCI device is encoded within a node called "psci" with a "compatible" property
// giving its compatible PSCI revisions (i.e., values of `kCompatibleDevices` below).
//
// For example,
//
// psci {
//      compatible  = "arm,psci-0.2";
//      method      = "smc";
// };
//
// For more details please see
// https://www.kernel.org/doc/Documentation/devicetree/bindings/arm/psci.txt
class ArmDevicetreePsciItem
    : public DevicetreeItemBase<ArmDevicetreePsciItem, 1>,
      public SingleOptionalItem<zbi_dcfg_arm_psci_driver_t, ZBI_TYPE_KERNEL_DRIVER,
                                ZBI_KERNEL_DRIVER_ARM_PSCI> {
 public:
  static constexpr auto kCompatibleDevices = std::to_array<std::string_view>({
      // PSCI 0.1 : Not Supported.
      // "arm,psci",
      "arm,psci-0.2",
      "arm,psci-1.0",
  });

  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);

  devicetree::ScanState OnScan() { return devicetree::ScanState::kDone; }

 private:
  devicetree::ScanState HandlePsciNode(const devicetree::NodePath& path,
                                       const devicetree::PropertyDecoder& decoder);
};

// Parses either GIC v2 or GIC v3 device node into proper ZBI item.
//
// This item will scan the devicetree for either a node compatible with GIC v2 bindings or GIC v3
// bindings. Upon finding such node it will generate either a |zbi_dcfg_arm_gic_v2_driver_t| for
// GIC v2 or a |zbi_dcfg_arm_gic_v3_driver_t| for GIC v3.
//
// In case of GIC v2, it will determine whether the MSI extension is supported or not by looking
// at the children of the GIC v2 node.
//
// Each interrupt controller contains uses a custom format for their 'reg' property, which defines
// the different address ranges required for the driver.
//
// See for GIC v2:
// * https://www.kernel.org/doc/Documentation/devicetree/bindings/interrupt-controller/arm%2Cgic.txt
// See for GIC v3:
// * https://www.kernel.org/doc/Documentation/devicetree/bindings/interrupt-controller/arm%2Cgic-v3.txt
class ArmDevicetreeGicItem
    : public DevicetreeItemBase<ArmDevicetreeGicItem, 1>,
      public SingleVariantItemBase<ArmDevicetreeGicItem, zbi_dcfg_arm_gic_v2_driver_t,
                                   zbi_dcfg_arm_gic_v3_driver_t> {
 public:
  static constexpr auto kGicV2CompatibleDevices = std::to_array<std::string_view>({
      "arm,gic-400",
      "arm,cortex-a15-gic",
      "arm,cortex-a9-gic",
      "arm,cortex-a7-gic",
      "arm,arm11mp-gic",
      "brcm,brahma-b15-gic",
      "arm,arm1176jzf-devchip-gic",
      "qcom,msm-8660-qgic",
      "qcom,msm-qgic2",
  });

  static constexpr auto kGicV3CompatibleDevices = std::to_array<std::string_view>({"arm,gic-v3"});

  // Boot Shim Item API.
  template <typename Shim>
  void Init(const Shim& shim) {
    DevicetreeItemBase::Init(shim);
    mmio_observer_ = &shim.mmio_observer();
  }

  // Matcher API.
  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState OnSubtree(const devicetree::NodePath& path);
  devicetree::ScanState OnScan() { return devicetree::ScanState::kDone; }

  // Boot Shim Item API.
  static constexpr zbi_header_t ItemHeader(const zbi_dcfg_arm_gic_v2_driver_t& driver) {
    return {.type = ZBI_TYPE_KERNEL_DRIVER, .extra = ZBI_KERNEL_DRIVER_ARM_GIC_V2};
  }

  static constexpr zbi_header_t ItemHeader(const zbi_dcfg_arm_gic_v3_driver_t& driver) {
    return {.type = ZBI_TYPE_KERNEL_DRIVER, .extra = ZBI_KERNEL_DRIVER_ARM_GIC_V3};
  }

 private:
  devicetree::ScanState HandleGicV2(const devicetree::NodePath& path,
                                    const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState HandleGicV3(const devicetree::NodePath& path,
                                    const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState HandleGicChildNode(const devicetree::NodePath& path,
                                           const devicetree::PropertyDecoder& decoder);

  constexpr bool IsGicChildNode() const { return gic_ != nullptr; }

  const devicetree::Node* gic_ = nullptr;
  const DevicetreeBootShimMmioObserver* mmio_observer_ = nullptr;
  bool matched_ = false;
};

// This matcher parses the 'chosen' node, which is a child of the root node('/chosen'). This node
// contains information about the commandline, ramdisk and UART.
//
// * The cmdline is contained as part of the string block of the devicetree.
//
// * The ramdisk is represented as a range in memory where the firmware loaded it, usually a ZBI.
//
// * The UART on the other hand, is represented as path(which may be aliased). Is the job of this
//   item to bootstrap the UART, which means determining which drItemiver needs to be used.
//
// For more details on the chosen node please see:
//  https://devicetree-specification.readthedocs.io/en/latest/chapter3-devicenodes.html#chosen-node
class DevicetreeChosenNodeMatcherBase
    : public DevicetreeItemBase<DevicetreeChosenNodeMatcherBase, 3> {
 public:
  DevicetreeChosenNodeMatcherBase(const char* shim_name, FILE* log)
      : DevicetreeItemBase(shim_name, log) {}

  // Matcher API.
  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState OnScan() {
    if (found_chosen_) {
      // If we had to match a tty arg, but we didn't find a UART matching
      // within a single scan, then it's over.
      if (tty_) {
        // Wait a full iteration before counting, so enumeration is determined
        // by the node position in the tree, and not by the relative position
        // from the chosen node.
        if (!tty_index_) {
          tty_index_ = 0;
          return devicetree::ScanState::kActive;
        }
        // After we do a full clean scan, and the index do not match, then
        // it means the argument is invalid, since we couldn't see enough ttys
        // to satisfy the argument. (E.g. ttys0 with 10 uarts)
        if (tty_index_ < tty_->index) {
          return devicetree::ScanState::kDone;
        }
      }
      return devicetree::ScanState::kActive;
    }
    return devicetree::ScanState::kDone;
  }

  // Accessors

  // Input ramdisk from devicetree.
  constexpr zbitl::ByteView ramdisk() const { return ramdisk_; }

  // Command line arguments from devicetree.
  constexpr std::optional<std::string_view> cmdline() const { return cmdline_; }

  // Resolved path for stdout device(e.g. uart) from the devicetree.
  constexpr std::optional<devicetree::ResolvedPath> stdout_path() const { return resolved_stdout_; }

 protected:
  auto& uart_selector() { return uart_selector_; }

  void set_uart_config(zbi_dcfg_simple_t* uart_config) { uart_config_ = uart_config; }

 private:
  devicetree::ScanState HandleBootstrapStdout(const devicetree::NodePath& path,
                                              const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState HandleUartInterruptParent(const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState HandleTtyNode(const devicetree::NodePath& path,
                                      const devicetree::PropertyDecoder& decoder);

  devicetree::ScanState SetUpUart(const devicetree::PropertyDecoder& decoder,
                                  devicetree::RegProperty& reg,
                                  const std::optional<devicetree::PropertyValue>& reg_offset,
                                  const std::optional<devicetree::PropertyValue>& interrupts);

  // May only be called after |uart_irq_.ResolveIrqController| returns |fit::ok(true)|.
  void SetUartIrq() {
    auto irq_config = uart_irq_.GetIrqConfig(0);
    uart_config_->irq = irq_config.irq;
    uart_config_->flags = irq_config.flags;
  }

  // Path to device node containing the stdout device (uart).
  bool found_chosen_ = false;
  std::string_view stdout_path_;
  std::optional<Tty> tty_;
  std::optional<size_t> tty_index_;
  std::optional<devicetree::ResolvedPath> resolved_stdout_;

  DevicetreeIrqResolver uart_irq_;

  std::array<std::string_view, 32> serial_aliases_;

  // Command line provided by the devicetree.
  std::string_view cmdline_;

  zbitl::ByteView ramdisk_;

  // Type erased match.
  fit::inline_function<bool(const devicetree::PropertyDecoder&)> uart_selector_ = nullptr;
  zbi_dcfg_simple_t* uart_config_ = nullptr;
};

template <typename AllUartDrivers = uart::all::Driver>
class DevicetreeChosenNodeMatcher : public DevicetreeChosenNodeMatcherBase {
 public:
  explicit DevicetreeChosenNodeMatcher(const char* shim_name, FILE* log = stdout)
      : DevicetreeChosenNodeMatcherBase(shim_name, log) {
    uart_selector() = [this](const devicetree::PropertyDecoder& decoder) -> bool {
      std::optional cfg = uart::all::Config<AllUartDrivers>::Select(decoder);
      if (cfg) {
        uart_config_ = *cfg;
        uart_config_->Visit([this]<typename UartType>(uart::Config<UartType>& cfg) {
          if constexpr (std::is_same_v<typename UartType::config_type, zbi_dcfg_simple_t>) {
            set_uart_config(&(*cfg));
          }
        });
      }
      return cfg.has_value();
    };
  }

  // Resolved configuration for the uart described by `stdout_path` or best effort cmdline
  // interpretation.
  constexpr std::optional<uart::all::Config<AllUartDrivers>> uart_config() const {
    return uart_config_;
  }

 private:
  std::optional<uart::all::Config<AllUartDrivers>> uart_config_;
};

// This matcher parses 'memory' and 'reserved_memory' device nodes and 'memranges' from the
// devicetree and makes them available.
//
// The memory regions are encoded in three different sources, whose layout and number of ranges
// pero node may vary.
//  * Each 'memory' nodes defines a collection of ranges that represent ram. Memory nodes
//    are childs of the root node and contain an address as part of the name(E.g. "/memory@1234").
//  * 'reserved-memory' is a container node, whose children define collections of memory ranges
//  that should be reserved. The 'reserved-memory' node is located under the root node
//  '/reserved-memory'.
//  * 'memreseve' represents the memory reservation block, which encodes pairs describing base
//  address and length of reserved memory ranges.
//  * `ramoops` node is a child of `reserverd-memory` node which is tracked as a special range.
//  When present will eventually be used to generate `ZBI_TYPE_NVRAM`.
//
// For more information and examples of each source see :
// '/memory' :
// https://devicetree-specification.readthedocs.io/en/latest/chapter3-devicenodes.html#memory-node
// '/reserved-memory' :
// https://devicetree-specification.readthedocs.io/en/latest/chapter3-devicenodes.html#reserved-memory-node
// 'memreserve' :
// https://devicetree-specification.readthedocs.io/en/latest/chapter5-flattened-format.html#memory-reservation-block
// `ramoops`:
// https://www.kernel.org/doc/Documentation/admin-guide/ramoops.rst'
//
class DevicetreeMemoryMatcher : public DevicetreeItemBase<DevicetreeMemoryMatcher, 1> {
 public:
  // Matcher API.
  constexpr DevicetreeMemoryMatcher(const char* shim_name, FILE* log,
                                    std::span<memalloc::Range> storage)
      : DevicetreeItemBase(shim_name, log), ranges_(storage) {}

  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState OnScan() { return devicetree::ScanState::kDone; }

  // Memory Item API for the bootshim to initialize the memory layout.
  // An empty set of memory ranges indicates an error while parsing the devicetree
  // memory ranges.
  constexpr std::span<const memalloc::Range> ranges() const {
    if (ranges_count_ <= ranges_.size()) {
      return std::span{ranges_.data(), ranges_count_};
    }
    return {};
  }

  // When valid `ramoops` node is found, then this range is filled.
  constexpr const std::optional<zbi_nvram_t>& nvram() const { return nvram_; }

 private:
  // Supports capturing `this`, a flag(whether there was an error) and the type
  // of the memory range being generated, for `AppendRangesFromReg`.
  using RangeVisitor =
      fit::inline_function<bool(uint64_t address, uint64_t size), 4 * sizeof(void*)>;

  // Append special ranges to the memory regions. This will be used later for
  // initializing the pool allocation memory.
  constexpr bool AppendRange(const memalloc::Range& range) {
    if (ranges_count_ >= ranges_.size()) {
      if (ranges_count_ == ranges_.size()) {
        OnError("Not enough preallocated ranges.");
      }
      ranges_count_ = ranges_.size() + 1;
      return false;
    }
    ranges_[ranges_count_++] = range;
    return true;
  }

  bool AppendRangesFromReg(const devicetree::PropertyDecoder& decoder,
                           memalloc::Type memrange_type);

  void OnEachRangeFromReg(const devicetree::PropertyDecoder& decoder, RangeVisitor range_visitor);

  devicetree::ScanState HandleMemoryNode(const devicetree::NodePath& path,

                                         const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState HandleReservedMemoryNode(const devicetree::NodePath& path,
                                                 const devicetree::PropertyDecoder& decoder);

  std::span<memalloc::Range> ranges_;
  size_t ranges_count_ = 0;
  std::optional<zbi_nvram_t> nvram_;
};

// This routine passes each memory reservation to a provided callback,
// excluding the ranges that overlap with a select number of "exclusions". The
// The callback should return `true` if it wishes to proceed with the iteration
// and `false` if it wishes to short-circuit; in the latter case the routine
// itself will return `false`. The exclusions should be non-overlapping and in
// order.
//
// While contrary to the devicetree spec - which says that a memory reservation
// is memory that should not be used by the kernel - we have encountered
// bootloaders that do generate reservations for the devicetree blob and
// ramdisk. This routine works around that to ensure that such ranges do not
// end up accounted for as RESERVED.
//
// This logic is separate from any matcher as memory reservations are not
// encoded within a devicetree blob's tree structure, and the ramdisk - one of
// the intended exclusions - is the product itself of a 'chosen' matcher.
template <typename Callback>
bool ForEachDevicetreeMemoryReservation(const devicetree::Devicetree& fdt,
                                        std::span<const memalloc::Range> exclusions,
                                        Callback&& cb) {
  using Reservation = devicetree::MemoryReservation;
  static_assert(std::is_invocable_r_v<bool, Callback, Reservation>);

  ZX_ASSERT(std::is_sorted(exclusions.begin(), exclusions.end(), [](auto a, auto b) {
    return (a.addr < b.addr) || (a.addr == b.addr && a.size < b.size);
  }));
  for (size_t i = 0; i + 1 < exclusions.size(); ++i) {
    ZX_ASSERT_MSG(exclusions[i].end() <= exclusions[i + 1].addr,
                  "Overlapping memory reservation exclusions: [%#" PRIx64 ", %#" PRIx64
                  "), [%#" PRIx64 ", %#" PRIx64 ")",                 //
                  exclusions[i].addr, exclusions[i].end(),           //
                  exclusions[i + 1].addr, exclusions[i + 1].end());  //
  }

  auto filter_exclusions = [&](Reservation res) -> bool {
    for (auto exclusion : exclusions) {
      //              [ res )
      // [ exclusion ) ...
      if (exclusion.end() <= res.start) {
        continue;
      }
      // [ res )
      //         [ exclusion ) ...
      if (res.end() <= exclusion.addr) {
        return cb(res);
      }

      // [ res )
      //     [ exclusion ) ...
      //
      // or
      //
      // [        res        )
      //     [ exclusion ) ...
      if (res.start < exclusion.addr) {
        if (!cb(Reservation{
                .start = res.start,
                .size = exclusion.addr - res.start,
            })) {
          return false;
        }
      }
      if (res.end() <= exclusion.end()) {  // First case.
        return true;
      }
      res = {.start = exclusion.end(), .size = res.end() - exclusion.end()};
    }
    return cb(res);
  };
  for (auto res : fdt.memory_reservations()) {
    if (!filter_exclusions(res)) {
      return false;
    }
  }
  return true;
}

// This item parses the '/cpus' 'timebase-frequency property to generate a timer driver
// configuration ZBI item.
//
// The timebase frequency specifies the clock frequency of the RISC-V timer device.
//
// See:
// https://www.kernel.org/doc/Documentation/devicetree/bindings/timer/riscv%2Ctimer.yaml
class RiscvDevicetreeTimerItem
    : public DevicetreeItemBase<RiscvDevicetreeTimerItem, 1>,
      public SingleOptionalItem<zbi_dcfg_riscv_generic_timer_driver_t, ZBI_TYPE_KERNEL_DRIVER,
                                ZBI_KERNEL_DRIVER_RISCV_GENERIC_TIMER> {
 public:
  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState OnScan() { return devicetree::ScanState::kDone; }
};

// Parses interrupt controller node that is compatible with PLIC (Platform Level Interrupt
// Controller bindings. For the time being, it only parses the mmio base for the plic register bank
// and the number of IRQs. Until the zbi item representing the riscv PLIC is extended to represent
// the contexts(hart_id, priority), the 'interrupt-extended' property is not yet decoded.
//
// See:
// https://www.kernel.org/doc/Documentation/devicetree/bindings/interrupt-controller/sifive%2Cplic-1.0.0.txt
class RiscvDevicetreePlicItem
    : public DevicetreeItemBase<RiscvDevicetreePlicItem, 1>,
      public SingleOptionalItem<zbi_dcfg_riscv_plic_driver_t, ZBI_TYPE_KERNEL_DRIVER,
                                ZBI_KERNEL_DRIVER_RISCV_PLIC> {
 public:
  static constexpr auto kCompatibleDevices = std::to_array({"sifive,plic-1.0.0", "riscv,plic0"});

  // Matcher API.
  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState OnScan() { return devicetree::ScanState::kDone; }

 private:
  devicetree::ScanState HandlePlicNode(const devicetree::NodePath& path,
                                       const devicetree::PropertyDecoder& decoder);
};

// Parses '/cpus' node to generate |ZBI_TYPE_CPU_TOPOLOGY| item. This involves both parsing CPU
// nodes and the '/cpus/cpu-map' node when present. Lack of a 'cpu-map' means all nodes are
// considered siblings which is reflected with none of them having a parent.
//
// A cluster's performance class is the normalized capacity of a cluster based on the maximum
// capacity of all clusters.
//
// cluster-performance-class[i] = cluster-capacity[i] * 255 / max(cluster-capacity[0]....N)
//
// When a cluster-capacity is not able to be determined because no property in the node provides
// this value then all clusters are given a performance class of 1. Its important to realize that
// the actual value of the performance class is only a representative of the relative difference
// between difference clusters.
//
// See:
// https://www.kernel.org/doc/Documentation/devicetree/bindings/arm/cpu-capacity.txt
// https://www.kernel.org/doc/Documentation/devicetree/bindings/cpu/cpu-topology.txt
class DevicetreeCpuTopologyItem : public DevicetreeItemBase<DevicetreeCpuTopologyItem, 2>,
                                  public ItemBase {
 public:
  // Matcher API.
  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState OnSubtree(const devicetree::NodePath& path);
  devicetree::ScanState OnScan() {
    return found_cpus_ ? devicetree::ScanState::kActive : devicetree::ScanState::kDone;
  }

  // Finalizes cpu_entries() (excluding skipped entries due to malformed
  // fields) and sorts them by ID for normalization's sake, which
  // is convenient at the very least for test purposes.
  void OnDone();

  size_t size_bytes() const { return ItemSize(node_element_count() * sizeof(zbi_topology_node_t)); }

  fit::result<DataZbi::Error> AppendItems(DataZbi& zbi) const;

 protected:
  // Used for decoding CPU-related properties.
  struct CpuEntry {
    std::optional<uint32_t> phandle;
    std::optional<devicetree::RegProperty> reg;
    devicetree::Properties properties;
  };

  std::span<const CpuEntry> cpu_entries() const { return cpu_entries_; }

  // Callback used during matching for checking architecture-specific processor
  // information. The returned boolean indicates whether matching should
  // record the current CPU, false indicating that crucial information was
  // missing or malformed. For finer-grained reporting, the expectation is that
  // `this` can be captured to leverage the devicetree item's logging
  // facilities.
  using CheckArchCpuInfo = fit::inline_function<bool(const CpuEntry& entry)>;

  // Callback used during AppendItems() for setting architecture-specific
  // processor information.
  using SetArchCpuInfo =
      fit::inline_function<void(zbi_topology_processor_t&, const CpuEntry& entry)>;

  template <typename Shim>
  void Init(const Shim& shim, CheckArchCpuInfo arch_info_checker, SetArchCpuInfo arch_info_setter) {
    DevicetreeItemBase<DevicetreeCpuTopologyItem, 2>::Init(shim);
    allocator_ = &shim.allocator();
    arch_info_checker_ = std::move(arch_info_checker);
    arch_info_setter_ = std::move(arch_info_setter);
  }

  template <std::default_initializable T>
  std::span<T> Allocate(size_t count, fbl::AllocChecker& ac,
                        std::source_location location = std::source_location::current()) const {
    size_t alloc_size = sizeof(T) * count;
    void* alloc = (*allocator_)(alloc_size, alignof(T), ac);
    if (!alloc) {
      // Log allocation failure. The effect is that the matcher will keep looking and will fail to
      // make progress. But the error will be logged.
      auto* self = const_cast<DevicetreeCpuTopologyItem*>(this);
      self->OnError("Allocation Failed.");
      self->Log("at %s:%u\n", location.file_name(), static_cast<unsigned int>(location.line()));
      return {};
    }
    return std::span{new (alloc) T[count]{}, count};
  }

  template <typename T>
  T* Allocate(fbl::AllocChecker& ac,
              std::source_location location = std::source_location::current()) const {
    return Allocate<T>(1, ac, location).data();
  }

 private:
  // Devicetree 'cpu-map' entities.
  enum class TopologyEntryType {
    kSocket,
    kCluster,
    kCore,
    kThread,
  };

  // Generic entry in the devicetree, maintains parent relationship and a view into the properties.
  struct CpuMapEntry {
    // Type of the entry.
    TopologyEntryType type;
    // Index of the parent entry on the cpu map.
    size_t parent_index;
    // Index of the cluster entry where this node is contained within the cpu map.
    std::optional<uint32_t> cluster_index;
    // 'phandle' obtained from the 'core' or 'thread' entries. Nodes containing this 'phandle'
    // represent a processing unit, and are leaf nodes in the cpu map.
    std::optional<uint32_t> cpu_phandle;
    // Index of the |CpuEntry| in the |cpus_| representing the resolved link of the |cpu_phandle|
    // to a |cpu| node.
    std::optional<uint32_t> cpu_index;
    // Index of |zbi_topology_node_t| in the |ZBI_ITEM_TYPE_CPU_TOPOLOGY| that was generated from
    // this |CpuMapEntry|.
    std::optional<size_t> topology_node_index;

    // Whether this entry should be skipped when generating the zbi output.
    bool skip = false;
  };

  // May only be called after |Init| and a full match sequence has been performed.
  constexpr size_t node_element_count() const { return topology_node_count_; }

  devicetree::ScanState IncreaseEntryNodeCountFirstScan(const devicetree::NodePath& path,
                                                        const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState AddEntryNodeSecondScan(const devicetree::NodePath& path,
                                               const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState IncreaseCpuNodeCountFirstScan(const devicetree::NodePath& path,
                                                      const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState AddCpuNodeSecondScan(const devicetree::NodePath& path,
                                             const devicetree::PropertyDecoder& decoder);

  static constexpr bool IsCpuMapNode(std::string_view node_name, std::string_view prefix) {
    if (!node_name.starts_with(prefix)) {
      return false;
    }
    // Must match prefix[0-9].
    return node_name.substr(prefix.length()).find_first_not_of("01234567890") ==
           std::string_view::npos;
  }

  // Remove any entries from the map that point to cpu entries whose status is not okay.
  // This MUST be called AFTER |UpdateEntryCpuLinks|.
  fit::result<ItemBase::DataZbi::Error, size_t> MarkSkippedMapEntries() const;

  // After both |entries_| and |cpus_| have been filled this routine will fill up
  // the reference from an entry to a 'cpu' node.
  fit::result<ItemBase::DataZbi::Error> UpdateEntryCpuLinks() const;

  // Recalculates performance class based on CPU capacity related properties.
  fit::result<ItemBase::DataZbi::Error> CalculateClusterPerformanceClass(
      std::span<zbi_topology_node_t> nodes) const;

  // Flattened 'cpu-map'.
  std::span<CpuMapEntry> map_entries_;
  uint32_t map_entry_index_ = 0;
  uint32_t map_entry_count_ = 0;
  bool has_cpu_map_ = false;

  // Used to track parent-child relationships when building the flattened cpu-map.
  std::optional<uint32_t> current_socket_;
  std::optional<uint32_t> current_cluster_;
  std::optional<uint32_t> current_core_;

  std::span<CpuEntry> cpu_entries_;
  uint32_t cpu_entry_count_ = 0;
  uint32_t cpu_entry_index_ = 0;
  uint32_t cluster_count_ = 0;

  size_t topology_node_count_ = 0;

  // Allocation is environment specific, so we delegate that to a lambda.
  mutable const Allocator* allocator_ = nullptr;

  CheckArchCpuInfo arch_info_checker_;
  SetArchCpuInfo arch_info_setter_;
  bool found_cpus_ = false;
};

class RiscvDevicetreeCpuTopologyItemBase : public DevicetreeCpuTopologyItem,
                                           public SingleItem<ZBI_TYPE_RISCV64_ISA_STRTAB> {
 public:
  explicit RiscvDevicetreeCpuTopologyItemBase(uint64_t boot_hart_id)
      : boot_hart_id_(boot_hart_id) {}

  ~RiscvDevicetreeCpuTopologyItemBase() { id_to_index_.clear_unsafe(); }

  template <typename Shim>
  void Init(Shim& shim) {
    DevicetreeCpuTopologyItem::Init(
        shim,  //
        [this](const CpuEntry& cpu_entry) {
          // The presence of "reg" should already have been validated.
          std::optional<devicetree::RegProperty> reg = cpu_entry.reg;
          ZX_DEBUG_ASSERT(reg);
          if (reg->size() != 1) {
            OnError("'reg' property in 'cpu' node contains an unexpected number of cells.");
            return false;
          }
          if (!(*reg)[0].address()) {
            OnError("Could not parse first cell of 'reg' property in 'cpu' node.");
            return false;
          }

          // No "riscv,isa"-less CPU should be recorded.
          devicetree::PropertyDecoder decoder(cpu_entry.properties);
          auto isa_string =
              decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsString>("riscv,isa");
          if (!isa_string) {
            OnError("Missing \"riscv,isa\" property");
            return false;
          }
          return true;
        },
        [this](zbi_topology_processor_t& node, const CpuEntry& cpu_entry) -> void {
          node.architecture_info.discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64;

          auto reg = cpu_entry.reg;
          ZX_DEBUG_ASSERT(reg);
          std::optional<uint64_t> hart_id = (*reg)[0].address();
          ZX_DEBUG_ASSERT(hart_id);
          node.architecture_info.riscv64.hart_id = *hart_id;

          if (*hart_id == boot_hart_id_) {
            node.flags |= ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY;
          } else {
            node.flags &= ~ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY;
          }
          auto it = id_to_index_.find(*hart_id);
          if (it != id_to_index_.end()) {
            node.architecture_info.riscv64.isa_strtab_index =
                static_cast<uint16_t>(it->strtab_index);
          }
        });
  }

  size_t size_bytes() const {
    return DevicetreeCpuTopologyItem::size_bytes() + IsaStrtabItem::size_bytes();
  }

  // Finalizes the ISA string table.
  void OnDone();

  fit::result<DataZbi::Error> AppendItems(DataZbi& zbi) const {
    if (auto result = DevicetreeCpuTopologyItem::AppendItems(zbi); result.is_error()) {
      return result;
    }
    return IsaStrtabItem::AppendItems(zbi);
  }

 private:
  using IsaStrtabItem = SingleItem<ZBI_TYPE_RISCV64_ISA_STRTAB>;

  struct IsaStrtabIndex
      : public fbl::SinglyLinkedListable<IsaStrtabIndex*, fbl::NodeOptions::AllowClearUnsafe> {
    // Required to instantiate fbl::DefaultKeyedObjectTraits.
    uint64_t GetKey() const { return hart_id; }

    // Required to instantiate fbl::DefaultHashTraits.
    static size_t GetHash(uint64_t key) { return static_cast<size_t>(key); }

    uint64_t hart_id = 0;
    size_t strtab_index = 0;
  };

  uint64_t boot_hart_id_;
  std::span<char> isa_strtab_;
  fbl::HashTable<uint64_t, IsaStrtabIndex*> id_to_index_;
};

// BootHartIdGetter provides a static means of accessing the boot hart ID, which
// derives from outside of the devicetree: it must provide a method of the form
// `static uint64_t Get()`.
template <typename BootHartIdGetter>
class RiscvDevicetreeCpuTopologyItem : public RiscvDevicetreeCpuTopologyItemBase {
 public:
  RiscvDevicetreeCpuTopologyItem() : RiscvDevicetreeCpuTopologyItemBase(BootHartIdGetter::Get()) {}
};

// ArmMpidr is expected to have a static `Read()` method that returns an
// `arch::ArmMultiprocessorAffinityRegister`.
template <typename ArmMpidr = arch::ArmMpidrEl1>
class ArmDevicetreeCpuTopologyItem : public DevicetreeCpuTopologyItem {
 public:
  template <typename Shim>
  void Init(Shim& shim) {
    DevicetreeCpuTopologyItem::Init(
        shim,  //
        [this](const CpuEntry& cpu_entry) {
          // The presence of "reg" should already have been validated.
          std::optional<devicetree::RegProperty> reg = cpu_entry.reg;
          ZX_DEBUG_ASSERT(reg);
          // Note: this is the number of register elements and not the number of
          // individual cells. The number of individual cells can be 1 or 2, but there
          // should be only one register element.
          if (reg->size() != 1) {
            OnError("'reg' property in 'cpu' node must contain exactly one register element.");
            return false;
          }
          if (!(*reg)[0].address()) {
            OnError("Could not parse first cell of 'reg' property in 'cpu' node.");
            return false;
          }
          return true;
        },
        [](zbi_topology_processor_t& node, const CpuEntry& cpu_entry) {
          node.architecture_info.discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_ARM64;

          auto reg = cpu_entry.reg;
          auto address = (*reg)[0].address();
          ZX_DEBUG_ASSERT(address);  // Validated in the arch info checker.
          node.architecture_info.arm64.gic_id =
              static_cast<uint8_t>(node.logical_ids[node.logical_id_count - 1]);

          // Save the CPU affinities from this node's reg's "address" element,
          // which is the mpidr value.
          auto entry_mpidr = arch::ArmMultiprocessorAffinityRegister{};
          entry_mpidr.set_reg_value(*address);
          node.architecture_info.arm64.cpu_id = static_cast<uint8_t>(entry_mpidr.aff0());
          node.architecture_info.arm64.cluster_1_id = static_cast<uint8_t>(entry_mpidr.aff1());
          node.architecture_info.arm64.cluster_2_id = static_cast<uint8_t>(entry_mpidr.aff2());
          node.architecture_info.arm64.cluster_3_id = static_cast<uint8_t>(entry_mpidr.aff3());

          // Flag the boot CPU by comparing the entry's affinity with the boot CPU's affinity.
          if (entry_mpidr.affinity() == ArmMpidr::Read().affinity()) {
            node.flags |= ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY;
          } else {
            node.flags &= ~ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY;
          }
        });
  }
};

// See https://www.kernel.org/doc/Documentation/devicetree/bindings/arm/arch_timer.txt
class ArmDevicetreeTimerItem
    : public DevicetreeItemBase<ArmDevicetreeTimerItem, 2>,
      public SingleOptionalItem<zbi_dcfg_arm_generic_timer_driver_t, ZBI_TYPE_KERNEL_DRIVER,
                                ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER> {
 public:
  static constexpr auto kCompatibleDevices = std::to_array({"arm,armv7-timer", "arm,armv8-timer"});

  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState OnScan() {
    return found_timer_ ? devicetree::ScanState::kActive : devicetree::ScanState::kDone;
  }

 private:
  bool found_timer_ = false;
  DevicetreeIrqResolver irq_;
  // Optional, maps to frequency override.
  std::optional<uint64_t> frequency_;
};

// A flat Devicetree ZBI Item.
using DevicetreeDtbItem = SingleItem<ZBI_TYPE_DEVICETREE>;

// Define an item that maps an arbitrary set of `Watchdogs` to `zbi_dcfg_generic32_watchdog_t`, that
// is a generic representation of a watchdog driver described by a set of actions that are mapped to
// a sequence of R-M-W operations in specific MMIO regions.
//
// Specifically given the set of `Wacthdogs` the first non-disabled matching device node will be
// used to fill the payload.
//
// See '<lib/boot-shim/watchdog.h>' for Watchdog Item API contract.
template <WatchdogMmioHelper MmioHelper, Watchdog<MmioHelper>... Watchdogs>
class GenericWatchdogItemBase
    : public DevicetreeItemBase<GenericWatchdogItemBase<MmioHelper, Watchdogs...>, 1>,
      public SingleOptionalItem<zbi_dcfg_generic32_watchdog_t, ZBI_TYPE_KERNEL_DRIVER,
                                ZBI_KERNEL_DRIVER_GENERIC32_WATCHDOG> {
  using Base = DevicetreeItemBase<GenericWatchdogItemBase<MmioHelper, Watchdogs...>, 1>;

 public:
  template <typename Shim>
  void Init(const Shim& shim) {
    Base::Init(shim);
    mmio_observer_ = &shim.mmio_observer();
  }

  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder) {
    auto compatibles =
        decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsStringList>("compatible");
    if (!compatibles) {
      return devicetree::ScanState::kActive;
    }

    for (auto compatible : *compatibles) {
      auto result = (Match<Watchdogs>(compatible, decoder) || ...);
      // Matched and maybe filled the payload.
      if (result) {
        if (payload() && payload()->enable_action.addr != 0) {
          const zbi_dcfg_generic32_watchdog_action_t& enable_action = payload()->enable_action;
          bool enabled = (MmioHelper::Read(enable_action.addr) & enable_action.set_mask) ==
                         enable_action.set_mask;
          if (enabled) {
            payload()->flags |= ZBI_KERNEL_DRIVER_GENERIC32_WATCHDOG_FLAGS_ENABLED;
          }
        }
        return devicetree::ScanState::kDone;
      }
    }
    return devicetree::ScanState::kActive;
  }

  // If after a full scan no watchdog is found, we should end.
  devicetree::ScanState OnScan() { return devicetree::ScanState::kDone; }

 private:
  template <class Watchdog>
  bool Match(std::string_view compatible, const devicetree::PropertyDecoder& decoder) {
    for (std::string_view device_compatible : Watchdog::kCompatibleDevices) {
      if (device_compatible == compatible) {
        if (auto payload = Watchdog::template MaybeCreate<MmioHelper>(decoder, mmio_observer_);
            payload) {
          set_payload(*payload);
        }
        return true;
      }
    }
    return false;
  }

  const DevicetreeBootShimMmioObserver* mmio_observer_ = nullptr;
};

template <WatchdogMmioHelper MmioHelper>
using GenericWatchdogItem = WithAllWatchdogs<GenericWatchdogItemBase, MmioHelper>;
using NvramItem = SingleOptionalItem<zbi_nvram_t, ZBI_TYPE_NVRAM>;

// Serial number can be provided as "serial-number" property in the root node, or as a boot argument
// in some cases. This matcher will prefer the root-node property if available, or fallback to
// chosen node's `bootargs` property providing the right item.
class DevicetreeSerialNumberItem : public DevicetreeItemBase<DevicetreeSerialNumberItem, 1>,
                                   public SingleItem<ZBI_TYPE_SERIAL_NUMBER> {
  using Base = DevicetreeItemBase<DevicetreeSerialNumberItem, 1>;

 public:
  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);

  void InitCmdline(std::string_view cmdline) { cmdline_ = cmdline; }

  template <typename Shim>
  void Init(const Shim& shim) {
    Base::Init(shim);
    cmdline_ = shim.legacy_cmdline();
  }

 private:
  std::string_view cmdline_;
};

// Extracts randomness from the bootloader provided bytes in the devicetree.
//
// `/chosen` may contain two properties that provide random bytes:
//    * 'kaslr-seed` see
//    https://github.com/devicetree-org/dt-schema/blob/main/dtschema/schemas/chosen.yaml#L35
//    * `rng-seed` see
//    https://github.com/devicetree-org/dt-schema/blob/main/dtschema/schemas/chosen.yaml#L55C1-L56C1
//
// When extracted, the devicetree bytes are cleared.
class DevicetreeSecureEntropyItem : public DevicetreeItemBase<DevicetreeSecureEntropyItem, 1>,
                                    public ItemBase {
 public:
  static constexpr uint8_t kFillPattern = 0xAD;

  size_t size_bytes() const {
    return ItemSize(rng_bytes_.size_bytes()) + ItemSize(kaslr_bytes_.size_bytes());
  }

  fit::result<DataZbi::Error> AppendItems(DataZbi& zbi);

  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);

 private:
  std::span<std::byte> rng_bytes_;
  std::span<std::byte> kaslr_bytes_;
};

class ArmDevicetreeTimerMmioItem
    : public DevicetreeItemBase<ArmDevicetreeTimerMmioItem, 2>,
      public SingleOptionalItem<zbi_dcfg_arm_generic_timer_mmio_driver_t, ZBI_TYPE_KERNEL_DRIVER,
                                ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER_MMIO> {
 public:
  static constexpr auto kCompatibleDevices = std::to_array({"arm,armv7-timer-mem"});
  static constexpr bool kMatchOkNodesOnly = false;

  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);

  devicetree::ScanState OnSubtree(const devicetree::NodePath& path) {
    // We just finished visiting all timer frames.
    // Note that this callback implies that the matcher is still in `kActive` state, meaning
    // IRQs need to be resolved.
    if (timer_ != nullptr && timer_ == &path.back()) {
      timer_ = nullptr;
      if (unresolved_irqs_ == 0) {
        return devicetree::ScanState::kDone;
      }
    }
    return devicetree::ScanState::kActive;
  }

  devicetree::ScanState OnScan() {
    if (unresolved_irqs_ == 0) {
      return devicetree::ScanState::kDone;
    }
    return devicetree::ScanState::kActive;
  }

  // When the scans are done, if by any chance all frames have some errors, do not emit an item.
  void OnDone() {
    if (present_ == 0 && payload()) {
      set_payload();
    } else {
      payload()->active_frames_mask = active_;
    }
  }

 private:
  devicetree::ScanState OnTimerNode(const devicetree::NodePath& path,
                                    const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState OnFrameNode(const devicetree::NodePath& path,
                                    const devicetree::PropertyDecoder& decoder);

  constexpr std::span<zbi_dcfg_arm_generic_timer_mmio_frame_t> frames() {
    return payload()->frames;
  }
  constexpr bool IsFramePresent(size_t frame_index) const {
    return (present_ & (1 << frame_index)) != 0;
  }

  // This is called AT MOST once per frame.
  void SetFrameIrq(size_t frame_index) {
    auto& irq_resolver = irq_resolvers_[frame_index];
    auto& frame = frames()[frame_index];
    ZX_DEBUG_ASSERT(frame.irq_phys == 0);
    ZX_DEBUG_ASSERT(frame.irq_virt == 0);
    if (irq_resolver.num_entries() > 0) {
      auto irq_config = irq_resolver.GetIrqConfig(0);
      frame.irq_phys = irq_config.irq;
      frame.irq_phys_flags = irq_config.flags;
    }
    if (irq_resolver.num_entries() > 1) {
      auto irq_config = irq_resolver.GetIrqConfig(1);
      frame.irq_virt = irq_config.irq;
      frame.irq_virt_flags = irq_config.flags;
    }
    unresolved_irqs_--;
  }

  const devicetree::Node* timer_ = nullptr;
  uint8_t active_ = 0;
  uint8_t present_ = 0;
  std::array<DevicetreeIrqResolver, 8> irq_resolvers_;
  size_t unresolved_irqs_ = 0;
};

// Extract `idle-states` and `domain-idle-states` information.
//
// For `idle-states` see
// https://www.kernel.org/doc/Documentation/devicetree/bindings/arm/idle-states.txt For
// `domain-idle-states see
// https://www.kernel.org/doc/Documentation/devicetree/bindings/power/domain-idle-state.yaml
//
// Note: There is inconsistency in the use of `domain-idle-states` and `idle-states, specifically on
// where they are located in the devicetree with respect to the spec.
class ArmDevicetreePsciCpuSuspendItem
    : public DevicetreeItemBase<ArmDevicetreePsciCpuSuspendItem, 2>,
      public ItemBase {
  using Base = DevicetreeItemBase<ArmDevicetreePsciCpuSuspendItem, 2>;

 public:
  template <typename Shim>
  void Init(Shim& shim) {
    Base::Init(shim);
    allocator_ = &shim.allocator();
  }

  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);

  devicetree::ScanState OnSubtree(const devicetree::NodePath& root);
  devicetree::ScanState OnScan();

  constexpr size_t size_bytes() const {
    return num_idle_states_ == 0
               ? 0
               : ItemSize(num_idle_states_ * sizeof(zbi_dcfg_arm_psci_cpu_suspend_state_t));
  }

  fit::result<DataZbi::Error> AppendItems(DataZbi& zbi);

 private:
  devicetree::ScanState OnIdleStateCount(const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState OnIdleStateFill(const devicetree::PropertyDecoder& decoder);

  std::span<zbi_dcfg_arm_psci_cpu_suspend_state_t> idle_states() {
    return {states_, num_idle_states_};
  }

  // Allocation is environment specific, so we delegate that to a lambda.
  const Allocator* allocator_ = nullptr;

  const devicetree::Node* idle_states_ = nullptr;
  const devicetree::Node* domain_idle_states_ = nullptr;
  uint32_t subtree_visited_count_ = 0;

  uint32_t current_idle_state_ = 0;
  uint32_t num_idle_states_ = 0;
  zbi_dcfg_arm_psci_cpu_suspend_state_t* states_ = nullptr;
};

// QCOM Rng device
//
// Note: No binding documentation exists.
class ArmDevicetreeQcomRngItem
    : public DevicetreeItemBase<ArmDevicetreePsciCpuSuspendItem, 2>,
      public SingleOptionalItem<zbi_dcfg_qcom_rng_t, ZBI_TYPE_KERNEL_DRIVER,
                                ZBI_KERNEL_DRIVER_QCOM_RNG> {
 public:
  static constexpr std::string_view kCompatible = "qcom,msm-rng";

  template <typename Shim>
  void Init(const Shim& shim) {
    DevicetreeItemBase::Init(shim);
    mmio_observer_ = &shim.mmio_observer();
  }

  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);

  devicetree::ScanState OnScan() { return devicetree::ScanState::kDone; }

 private:
  const DevicetreeBootShimMmioObserver* mmio_observer_ = nullptr;
};

// Reference: https://www.kernel.org/doc/Documentation/devicetree/bindings/iommu/arm%2Csmmu.txt
class ArmDevicetreeSmmuItem : public DevicetreeItemBase<ArmDevicetreeSmmuItem, 2>, public ItemBase {
 public:
  static constexpr auto kCompatibleDevices = std::to_array({
      // clang-format off
      "arm,smmu-v1",
      "arm,smmu-v2",
      "arm,mmu-400",
      "arm,mmu-401",
      "arm,mmu-500",
      "cavium,smmu-v2",
      "qcom,smmu-v2",
      "qcom,msm8996-smmu-v2",
      "qcom,sdm845-smmu-v2",
      "qcom,sdm845-smmu-500",
      "qcom,qsmmu-v500",
      // clang-format on
  });

  template <typename Shim>
  void Init(const Shim& shim) {
    DevicetreeItemBase::Init(shim);
    allocator_ = &shim.allocator();
  }

  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState OnScan();

  size_t size_bytes() const { return ItemSize(smmu_cnt_ * sizeof(zbi_dcfg_arm_smmu_driver_t)); }
  fit::result<DataZbi::Error> AppendItems(DataZbi& zbi) const;

 private:
  template <typename T>
  std::span<T> Allocate(size_t count, fbl::AllocChecker& ac,
                        std::source_location location = std::source_location::current()) const {
    size_t alloc_size = sizeof(T) * smmu_cnt_;
    auto* alloc = static_cast<T*>((*allocator_)(alloc_size, alignof(T), ac));

    if (!alloc) {
      // Log allocation failure. The effect is that the matcher will keep looking and will fail to
      // make progress. But the error will be logged.
      auto* self = const_cast<ArmDevicetreeSmmuItem*>(this);
      self->OnError("Allocation Failed.");
      self->Log("at %s:%u\n", location.file_name(), static_cast<unsigned int>(location.line()));
    }

    memset(alloc, 0, alloc_size);
    return std::span<T>(alloc, smmu_cnt_);
  }

  const Allocator* allocator_{nullptr};
  std::span<zbi_dcfg_arm_smmu_driver_t> data_{};
  uint32_t pass_cnt_{0};
  uint32_t smmu_cnt_{0};
};

}  // namespace boot_shim

#endif  // ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_DEVICETREE_H_
