// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_ULIB_UART_INCLUDE_LIB_UART_UART_H_
#define ZIRCON_SYSTEM_ULIB_UART_INCLUDE_LIB_UART_UART_H_

#include <lib/arch/intrin.h>
#include <lib/devicetree/devicetree.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zircon-internal/macros.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <bit>
#include <cassert>
#include <concepts>
#include <cstdlib>
#include <optional>
#include <span>
#include <string_view>
#include <type_traits>
#include <utility>

#include <hwreg/mmio.h>
#include <hwreg/pio.h>

#include "chars-from.h"

// While this header is unused in this file, it provides the basic implementations
// for `Sync` types.
#include "sync.h"

namespace acpi_lite {
struct AcpiDebugPortDescriptor;
}

namespace uart {

// Config type for stub drivers, such that certain operations may be defined
// against them.
struct StubConfig {
  // This isn't an empty type just so it will be a zbitl::PayloadData type like
  // all the real ones are.
  uint64_t do_not_use = 0;
};

// Tagged configuration type, used to represent the configuration of `Driver` even if multiple types
// of driver have the same `config_type`.
template <typename Driver>
class Config {
 public:
  using uart_type = Driver;
  using config_type = typename Driver::config_type;

  constexpr Config() = default;
  explicit constexpr Config(const config_type& cfg) : config_(cfg) {}

  constexpr config_type* operator->() { return &config_; }
  constexpr const config_type* operator->() const { return &config_; }

  constexpr config_type& operator*() { return config_; }
  constexpr const config_type& operator*() const { return config_; }

  constexpr bool operator==(const Config& rhs) const
    requires(std::is_same_v<config_type, StubConfig>)
  {
    return true;
  }

  constexpr bool operator==(const Config& rhs) const
    requires(std::is_same_v<config_type, zbi_dcfg_simple_pio_t>)
  {
    return config_.base == rhs.config_.base && config_.irq == rhs.config_.irq;
  }

  constexpr bool operator==(const Config& rhs) const
    requires(std::is_same_v<config_type, zbi_dcfg_simple_t>)
  {
    return config_.mmio_phys == rhs.config_.mmio_phys && config_.irq == rhs.config_.irq &&
           config_.flags == rhs.config_.flags;
  }

  template <typename OtherDriver>
  constexpr bool operator==(const Config<OtherDriver>& rhs) const {
    return false;
  }

  constexpr std::span<const std::byte> as_bytes() const {
    return {reinterpret_cast<const std::byte*>(&config_), sizeof(config_)};
  }

 private:
  config_type config_ = {};
};

//
// These types are used in configuring the line control settings (i.e., in the
// `SetLineControl()` method).
//

// Number of bits transmitted per character.
enum class DataBits {
  k5,
  k6,
  k7,
  k8,
};

// The bit pattern mechanism to help detect transmission errors.
enum class Parity {
  kNone,  // No bits dedicated to parity.
  kEven,  // Parity bit present; is 0 iff the number of 1s in the word is even.
  kOdd,   // Parity bit present; is 0 iff the number of 1s in the word is odd.
};

// The duration of the stop period in terms of the transmitted bit rate.
enum class StopBits {
  k1,
  k2,
};

// This template is specialized by payload configuration type (see below).
// It parses bits out of strings from the "kernel.serial" boot option.
template <typename Config>
inline std::optional<Config> ParseConfig(std::string_view) {
  static_assert(std::is_void_v<Config>, "missing specialization");
  return {};
}

// This template is specialized by payload configuration type (see below).
// It recreates a string for Parse.
template <typename Config>
inline void UnparseConfig(const Config& config, FILE* out) {
  static_assert(std::is_void_v<Config>, "missing specialization");
}

enum class IoRegisterType {
  // Null/Stub drivers.
  kNone,

  // MMIO is performed without any scaling what so ever, this means that
  // registers offsets are treated as byte offsets from the base address.
  kMmio8,

  // MMIO is performed with an scaling factor of 4, this means that
  // register offsets are treated as 4-byte offsets from the base address.
  kMmio32,

  // PIO.
  kPio,
};

template <IoRegisterType IoRegType>
using IoSlotType = std::conditional_t<IoRegType == IoRegisterType::kPio, uint16_t, size_t>;

// Constant indicating that the number of `io_slots()` is to be determined at
// runtime.
constexpr size_t kDynamicIoSlot = std::numeric_limits<size_t>::max();

// Communicates the range where the configuration dictates the registers are located.
//
// It may need to be translated if the addressing used for the configuration is different from
// the one used for execution (e.g. physical and virtual addressing).
struct MmioRange {
  constexpr MmioRange AlignedTo(uint64_t alignment) const {
    assert(alignment > 0);
    assert(std::has_single_bit(alignment));
    const uint64_t aligned_start = address & -alignment;
    const uint64_t aligned_end = (address + size + alignment - 1) & -alignment;
    return {.address = aligned_start, .size = aligned_end - aligned_start};
  }

  constexpr bool empty() const { return size == 0; }
  constexpr uint64_t end() const { return address + size; }

  uint64_t address = 0;
  uint64_t size = 0;
};

// Communicates the range of I/O Ports that the device is using.
struct IoPortRange {
  // Address of the first port in the address space that is used by the device.
  uint16_t base = 0;
  // Number of consecutive I/O Ports used by the device.
  uint32_t count = 0;
};

// This matches either a Driver API object or a KernelDriver wrapper
// instantiation for an MMIO-based driver.
template <class Driver>
concept MmioDriver = requires(const Driver& driver) {
  { driver.mmio_range() } -> std::same_as<MmioRange>;
};

template <class Driver>
concept PioDriver = std::same_as<typename std::decay_t<Driver>::config_type, zbi_dcfg_simple_pio_t>;

template <class Driver>
concept NonMmioDriver = !MmioDriver<Driver>;

// Specific hardware support is implemented in a class uart::xyz::Driver,
// referred to here as UartDriver.  The uart::DriverBase template class
// provides a helper base class for UartDriver implementations.
//
// The UartDriver object represents the hardware itself.  Many UartDriver
// classes hold no state other than the initial configuration data used in the
// constructor, but a UartDriver is not required to be stateless.  However, a
// UartDriver is required to be copy-constructible, trivially destructible,
// and contain no pointers.  This makes it safe to copy an object set up by
// physboot into a new object in the virtual-memory kernel to hand off the
// configuration and the state of the hardware.
//
// All access to the UartDriver object is serialized by its caller, so it does
// no synchronization of its own.  This serves to serialize the actual access
// to the hardware.
//
// The UartDriver API fills four roles:
//  1. Match a ZBI item that configures this driver.
//  2. Generate a ZBI item for another kernel to match this configuration.
//  3. Configure the IoProvider (see below).
//  4. Drive the actual hardware.
//
// The first three are handled by DriverBase.  The KdrvExtra and KdrvConfig
// template arguments give the ZBI_KERNEL_DRIVER_* value and the zbi_dcfg_*_t type for the ZBI
// item.  The Pio template argument tells the IoProvider whether this driver
// uses MMIO or PIO (including PIO via MMIO): the number of consecutive PIO
// ports used, or 0 for simple MMIO.
//
// Item matching is done by the MaybeCreate static method.  If the item
// matches KdrvExtra, then the UartDriver(KdrvConfig) constructor is called.
// DriverBase provides this constructor to fill the cfg_ field, which the
// UartDriver can then refer to.  The UartDriver copy constructor copies it.
//
// The calls to drive the hardware are all template functions passed an
// IoProvider object (see below).  The driver accesses device registers using
// hwreg ReadFrom and WriteTo calls on the pointers from the provider.  The
// IoProvider constructed is passed uart.config() and uart.pio_size().
//
// `IoSlots` is an opaque parameter whose meaning is tied to the value of `IoRegType`.
// A very broad description would be the number of 'slots' to perform I/O operations.
// * `kMmio8` and `kMmio32` represents the number of bytes from the base address with the proper
//    scaling factor applied, 1 and 4 respectively.
// * `kPio` represents the number the port count.
template <typename Driver, uint32_t KdrvExtra, typename KdrvConfig, IoRegisterType IoRegType,
          IoSlotType<IoRegType> IoSlots = kDynamicIoSlot>
class DriverBase {
 public:
  using config_type = KdrvConfig;

  // No devicetree bindings by default.
  static constexpr std::array<std::string_view, 0> kDevicetreeBindings{};

  // Register Io Type.
  static constexpr IoRegisterType kIoType = IoRegType;

  static constexpr uint32_t kType = ZBI_TYPE_KERNEL_DRIVER;
  static constexpr uint32_t kExtra = KdrvExtra;

  static std::optional<uart::Config<Driver>> TryMatch(const zbi_header_t& header,
                                                      const void* payload) {
    static_assert(alignof(config_type) <= ZBI_ALIGNMENT);
    if (header.type == ZBI_TYPE_KERNEL_DRIVER && header.extra == KdrvExtra &&
        header.length >= sizeof(config_type)) {
      return Config<Driver>{*reinterpret_cast<const config_type*>(payload)};
    }
    return {};
  }

  static std::optional<uart::Config<Driver>> TryMatch(std::string_view string) {
    const auto config_name = Driver::kConfigName;
    if (string.substr(0, config_name.size()) == config_name) {
      string.remove_prefix(config_name.size());
      auto config = ParseConfig<KdrvConfig>(string);
      if (config) {
        return Config<Driver>{*config};
      }
    }
    return {};
  }

  // API to match DBG2 Table (ACPI). Currently only 16550 compatible uarts are supported.
  static std::optional<uart::Config<Driver>> TryMatch(
      const acpi_lite::AcpiDebugPortDescriptor& debug_port) {
    return {};
  }

  // API to match a devicetree bindings.
  static bool TrySelect(const devicetree::PropertyDecoder& decoder) {
    if constexpr (Driver::kDevicetreeBindings.size() == 0) {
      return false;
    } else {
      auto compatible = decoder.FindProperty("compatible");
      if (!compatible) {
        return false;
      }

      auto compatible_list = compatible->AsStringList();
      if (!compatible_list) {
        return false;
      }

      return std::find_first_of(compatible_list->begin(), compatible_list->end(),
                                Driver::kDevicetreeBindings.begin(),
                                Driver::kDevicetreeBindings.end()) != compatible_list->end();
    }
  }

  explicit DriverBase(const config_type& cfg) : cfg_(cfg) {}
  explicit DriverBase(const Config<Driver>& tagged_config) : DriverBase(*tagged_config) {}

  // API to fill a ZBI item describing this UART.
  void FillItem(void* payload) const { memcpy(payload, &cfg_, sizeof(config_type)); }

  // API to reproduce a configuration string.
  void Unparse(FILE* out) const {
    fprintf(out, "%.*s", static_cast<int>(Driver::kConfigName.size()), Driver::kConfigName.data());
    UnparseConfig<KdrvConfig>(cfg_, out);
  }

  // TODO(https://fxbug.dev/42053694): Remove once all drivers define this method.
  template <class IoProvider>
  void SetLineControl(IoProvider& io, std::optional<DataBits> data_bits,
                      std::optional<Parity> parity, std::optional<StopBits> stop_bits) {
    static_assert(!std::is_same_v<IoProvider, IoProvider>,
                  "TODO(https://fxbug.dev/42053694): implment me");
  }

  // API for use in IoProvider setup.
  const config_type& config() const { return cfg_; }

  // Number of 'slots' to perform I/O operations.
  template <IoSlotType<IoRegType> _IoSlots = IoSlots,
            std::enable_if_t<_IoSlots != kDynamicIoSlot, bool> = true>
  constexpr IoSlotType<IoRegType> io_slots() const {
    return IoSlots;
  }

  template <IoSlotType<IoRegType> _IoSlots = IoSlots,
            std::enable_if_t<_IoSlots == kDynamicIoSlot, bool> = true>
  constexpr IoSlotType<IoRegType> io_slots() const {
    static_assert(
        !std::is_same_v<void, void>,
        "|IoSlots| must be different from |kDynamicIoSlot| or |io_slots| implementation must be provided in derived class.");
    return IoSlots;
  }

  constexpr MmioRange mmio_range() const
    requires(Driver::kIoType == IoRegisterType::kMmio32)
  {
    return GetMmioRange<uint32_t>();
  }

  constexpr MmioRange mmio_range() const
    requires(Driver::kIoType == IoRegisterType::kMmio8)
  {
    return GetMmioRange<uint8_t>();
  }

  constexpr IoPortRange ioports() const
    requires(PioDriver<Driver>)
  {
    return {.base = config().base, .count = io_slots()};
  }

 protected:
  config_type cfg_;

 private:
  template <typename T>
  static constexpr bool Uninstantiated = false;

  template <typename MmioType>
  constexpr MmioRange GetMmioRange() const {
    return {
        .address = config().mmio_phys,
        .size = io_slots() * sizeof(MmioType),
    };
  }

  // uart::KernelDriver API
  //
  // These are here just to cause errors if Driver is missing its methods.
  // They also serve to document the API required by uart::KernelDriver.
  // They should all be overridden by Driver methods.
  //
  // Each method is a template parameterized by an hwreg-compatible type for
  // accessing the hardware registers via hwreg ReadFrom and WriteTo methods.
  // This lets Driver types be used with hwreg::Mock in tests independent of
  // actual hardware access.

  template <typename IoProvider>
  void Init(IoProvider& io) {
    static_assert(Uninstantiated<IoProvider>, "derived class is missing Init");
  }

  // Return true if Write can make forward progress right now.
  // The return value can be anything contextually convertible to bool.
  // The value will be passed on to Write.
  template <typename IoProvider>
  bool TxReady(IoProvider& io) {
    static_assert(Uninstantiated<IoProvider>, "derived class is missing TxReady");
    return false;
  }

  // This is called only when TxReady() has just returned something that
  // converts to true; that's passed here so it can convey more information
  // such as a count.  Advance the iterator at least one and as many as is
  // convenient but not past end, outputting each character before advancing.
  template <typename IoProvider, typename It1, typename It2>
  auto Write(IoProvider& io, bool ready, It1 it, const It2& end) {
    static_assert(Uninstantiated<IoProvider>, "derived class is missing Write");
    return end;
  }

  // Poll for an incoming character and return one if there is one.
  template <typename IoProvider>
  std::optional<uint8_t> Read(IoProvider& io) {
    static_assert(Uninstantiated<IoProvider>, "derived class is missing Read");
    return {};
  }

  // Set the UART up to deliver interrupts.  This is called after Init.
  // After this, Interrupt (below) may be called from interrupt context.
  template <typename IoProvider, typename IrqProvider>
  void InitInterrupt(IoProvider& io, IrqProvider& irq) {
    static_assert(Uninstantiated<IoProvider>, "derived class is missing InitInterrupt");
  }

  // Enable transmit interrupts so Interrupt will be called when TxReady().
  template <typename IoProvider>
  void EnableTxInterrupt(IoProvider& io) {
    static_assert(Uninstantiated<IoProvider>, "derived class is missing EnableTxInterrupt");
  }

  // Service an interrupt.
  // Call tx(sync, disable_tx_irq) if transmission has become ready.
  // If receiving has become ready, call rx(sync, read_char, full) one or more
  // times, where read_char() -> uint8_t if there is receive buffer
  // space and full() -> void if there is no space.
  // |sync| provides access to the environment specific synchronization primitives(if any),
  // and synchronization related data structures(if any).
  template <typename IoProvider, typename Sync, typename Tx, typename Rx>
  void Interrupt(IoProvider& io, Sync& sync, Tx&& tx, Rx&& rx) {
    static_assert(Uninstantiated<IoProvider>, "derived class is missing Interrupt");
  }

 public:
  // Prepare/Wake-up the UART hardware before going into, and after coming out
  // of a suspended state. Unlike other kernel API methods, these methods are
  // considered optional, and default to no-ops.
  template <typename IoProvider, typename IrqProvider>
  void PrepareForSuspend(IoProvider& io, IrqProvider& irq) {}

  template <typename IoProvider, typename IrqProvider>
  void WakeupFromSuspend(IoProvider& io, IrqProvider& irq) {}
};

// The IoProvider is a template class parameterized by UartDriver::config_type,
// i.e. the zbi_dcfg_*_t type for the ZBI item's format.  This class is responsible
// for supplying pointers to be passed to hwreg types' ReadFrom and WriteTo.
//
// The
// ```
// IoProvider(UartDriver::config_type, uint16_t pio_size, volatile void* base)
// ```
// constructor initializes the object with the provided virtual base address
// (in the case of MMIO), while the constructor that omits the address
// initializes things for PIO or identity-mapped MMIO.  Then IoProvider::io()
// is called to yield the pointer to pass to hwreg calls.
//
template <typename Config, IoRegisterType IoType>
class BasicIoProvider;

// Specialization for Stub drivers, such as `null::Driver`.
template <typename ConfigType>
class BasicIoProvider<ConfigType, IoRegisterType::kNone> {
 public:
  constexpr BasicIoProvider(const ConfigType& cfg, size_t io_slots) {}
  constexpr BasicIoProvider(const ConfigType& cfg, size_t io_slots, volatile void* base) {}
  constexpr BasicIoProvider& operator=(BasicIoProvider&& other) {}

  auto io() { return nullptr; }
};

// The specialization used most commonly handles simple MMIO devices.
template <IoRegisterType IoType>
class BasicIoProvider<zbi_dcfg_simple_t, IoType> {
 public:
  // Just install the MMIO base pointer.  The third argument can be passed by
  // a subclass constructor method to map the physical address to a virtual
  // address.
  BasicIoProvider(const zbi_dcfg_simple_t& cfg, size_t io_slots, volatile void* base) {
    if constexpr (IoType == IoRegisterType::kMmio8) {
      io_.emplace<hwreg::RegisterMmio>(base);
    } else if constexpr (IoType == IoRegisterType::kMmio32) {
      io_.emplace<hwreg::RegisterMmioScaled<uint32_t>>(base);
    } else {
      // Pio uses a different specialization, this should never be reached.
      static_assert(!std::is_same_v<IoRegisterType, IoRegisterType>);
    }
  }

  BasicIoProvider(const zbi_dcfg_simple_t& cfg, size_t io_slots)
      : BasicIoProvider(cfg, io_slots, reinterpret_cast<volatile void*>(cfg.mmio_phys)) {}

  BasicIoProvider& operator=(BasicIoProvider&& other) {
    io_.swap(other.io_);
    return *this;
  }

  auto* io() { return &io_; }

 private:
  std::variant<hwreg::RegisterMmio, hwreg::RegisterMmioScaled<uint32_t>> io_{std::in_place_index<0>,
                                                                             nullptr};
};

// The specialization for devices using actual PIO only occurs on x86.
#if defined(__x86_64__) || defined(__i386__)
template <IoRegisterType IoType>
class BasicIoProvider<zbi_dcfg_simple_pio_t, IoType> {
 public:
  explicit BasicIoProvider(const zbi_dcfg_simple_pio_t& cfg, uint16_t io_slots) : io_(cfg.base) {
    static_assert(IoType == IoRegisterType::kPio);
    ZX_DEBUG_ASSERT(io_slots > 0);
  }

  auto* io() { return &io_; }

 private:
  hwreg::RegisterDirectPio io_;
};
#endif

// A no-op implementation of an IrqProvider.  Useful in interrupts which cannot
// (or do not want to) use interrupts.
class NullIrqProvider {
 public:
  NullIrqProvider() = default;
  ~NullIrqProvider() = default;

  // No copy, no move.
  NullIrqProvider(const NullIrqProvider&) = delete;
  NullIrqProvider& operator=(const NullIrqProvider&) = delete;
  NullIrqProvider(NullIrqProvider&&) = delete;
  NullIrqProvider& operator=(NullIrqProvider&&) = delete;

  void Init() {}
  void SetInterruptsEnabled(bool enabled) {}
};

// Forward declaration.
namespace mock {
class Driver;
}

// The KernelDriver template class is parameterized by those three to implement
// actual driver logic for some environment.
//
// The KernelDriver constructor just passes its arguments through to the
// UartDriver constructor.  So it can be created directly from a configuration
// struct or copied from another UartDriver object.  In this way, the device is
// handed off from one KernelDriver instantiation to a different one using a
// different IoProvider (physboot vs kernel) and/or Sync (polling vs blocking).
//
template <class UartDriver, template <typename, IoRegisterType> class IoProvider, class SyncPolicy,
          class IrqProvider>
class KernelDriver {
 public:
  using Waiter = typename SyncPolicy::Waiter;

  template <typename LockPolicy>
  using Guard = typename SyncPolicy::template Guard<LockPolicy>;

  template <typename MemberOf>
  using Lock = typename SyncPolicy::template Lock<MemberOf>;

  using DefaultLockPolicy = typename SyncPolicy::DefaultLockPolicy;

  using uart_type = UartDriver;
  using config_type = UartDriver::config_type;
  static_assert(std::is_copy_constructible_v<uart_type> || std::is_same_v<uart_type, mock::Driver>);
  static_assert(std::is_trivially_destructible_v<uart_type> ||
                std::is_same_v<uart_type, mock::Driver>);

  // This sets up the object but not the device itself.  The device might
  // already have been set up by a previous instantiation's Init function,
  // or might never actually be set up because this instantiation gets
  // replaced with a different one before ever calling Init.
  template <typename... Args>
  explicit KernelDriver(Args&&... args)
      : uart_(std::forward<Args>(args)...), io_(uart_.config(), uart_.io_slots()) {
    if constexpr (std::is_same_v<mock::Driver, uart_type>) {
      // Initialize the mock sync object with the mock driver.
      lock_.Init(uart_);
      waiter_.Init(uart_);
    }
  }

  template <typename LockPolicy = DefaultLockPolicy>
    requires(MmioDriver<UartDriver>)
  constexpr MmioRange mmio_range() const {
    Guard<LockPolicy> lock(&lock_, SOURCE_TAG);
    return uart_.mmio_range();
  }

  template <typename LockPolicy = DefaultLockPolicy>
    requires(PioDriver<UartDriver>)
  constexpr IoPortRange ioports() const {
    Guard<LockPolicy> lock(&lock_, SOURCE_TAG);
    return uart_.ioports();
  }

  template <typename LockPolicy = DefaultLockPolicy>
  uart_type TakeUart() && {
    Guard<LockPolicy> lock(&lock_, SOURCE_TAG);
    return std::move(uart_);
  }

  // Returns a copy of the underlying uart config.
  template <typename LockPolicy = DefaultLockPolicy>
  config_type config() const {
    Guard<LockPolicy> lock(&lock_, SOURCE_TAG);
    return uart_.config();
  }

  // Access IoProvider object.
  auto& io() { return io_; }

  // Access IrqProvider object.
  auto& irq() { return irq_; }

  // Set up the device for nonblocking output and polling input.
  // If the device is handed off from a different instantiation,
  // this won't be called in the new instantiation.
  template <typename LockPolicy = DefaultLockPolicy>
  void Init() {
    Guard<LockPolicy> lock(&lock_, SOURCE_TAG);
    uart_.Init(io_);
  }

  // Write out a string that Parse() can read back to recreate the driver
  // state.  This doesn't preserve the driver state, only the configuration.
  template <typename LockPolicy = DefaultLockPolicy>
  void Unparse(FILE* out) const {
    Guard<LockPolicy> lock(&lock_, SOURCE_TAG);
    uart_.Unparse(out);
  }

  // Configure the UART line control settings.
  //
  // An individual setting given by std::nullopt signifies that it should be
  // left as previously configured.
  //
  // TODO(https://fxbug.dev/42053694): Separate out the setting of baud rate.
  template <typename LockPolicy = DefaultLockPolicy>
  void SetLineControl(std::optional<DataBits> data_bits = DataBits::k8,
                      std::optional<Parity> parity = Parity::kNone,
                      std::optional<StopBits> stop_bits = StopBits::k1) {
    Guard<LockPolicy> lock(&lock_, SOURCE_TAG);
    uart_.SetLineControl(io_, data_bits, parity, stop_bits);
  }

  // TODO(https://fxbug.dev/42079801): Asses the need of |enable_interrupt_callback|.
  template <typename LockPolicy = DefaultLockPolicy, typename... Args>
  void InitInterrupt(Args&&... args) {
    Guard<LockPolicy> lock(&lock_, SOURCE_TAG);
    irq_.Init(std::forward<Args>(args)...);
    uart_.InitInterrupt(io_, irq_);
  }

  template <typename LockPolicy = DefaultLockPolicy>
  void PrepareForSuspend() {
    Guard<LockPolicy> lock(&lock_, SOURCE_TAG);
    uart_.PrepareForSuspend(io_, irq_);
  }

  template <typename LockPolicy = DefaultLockPolicy>
  void WakeupFromSuspend() {
    Guard<LockPolicy> lock(&lock_, SOURCE_TAG);
    uart_.WakeupFromSuspend(io_, irq_);
  }

  template <typename Tx, typename Rx>
  void Interrupt(Tx&& tx, Rx&& rx) TA_NO_THREAD_SAFETY_ANALYSIS {
    // Interrupt is responsible for properly acquiring and releasing sync
    // where needed.
    uart_.Interrupt(io_, lock_, waiter_, std::forward<Tx>(tx), std::forward<Rx>(rx));
  }

  // This is the FILE-compatible API: `FILE::stdout_ = FILE{&driver};`.
  template <typename LockPolicy = DefaultLockPolicy, typename... Args>
  int Write(std::string_view str, Args&&... waiter_args) {
    uart::CharsFrom chars(str);  // Massage into uint8_t with \n -> CRLF.
    auto it = chars.begin();
    Guard<LockPolicy> lock(&lock_, SOURCE_TAG);
    while (it != chars.end()) {
      // Wait until the UART is ready for Write.
      auto ready = uart_.TxReady(io_);
      while (!ready) {
        // Block or just unlock and spin or whatever "wait" means to Sync.
        // If that means blocking for interrupt wakeup, enable tx interrupts.
        waiter_.Wait(
            lock,
            [this]() {
              SyncPolicy::AssertHeld(lock_);
              uart_.EnableTxInterrupt(io_);
            },
            std::forward<Args>(waiter_args)...);
        ready = uart_.TxReady(io_);
      }
      // Advance the iterator by writing some.
      it = uart_.Write(io_, std::move(ready), it, chars.end());
    }
    return static_cast<int>(str.size());
  }

  // This is a direct polling read, not used in interrupt-based operation.
  template <typename LockPolicy = DefaultLockPolicy>
  auto Read() {
    Guard<LockPolicy> lock(&lock_, SOURCE_TAG);
    return uart_.Read(io_);
  }

  template <typename LockPolicy = DefaultLockPolicy>
  void EnableRxInterrupt() {
    Guard<LockPolicy> lock(&lock_, SOURCE_TAG);
    uart_.EnableRxInterrupt(io_);
  }

 private:
  Lock<KernelDriver> lock_;
  Waiter waiter_ TA_GUARDED(lock_);
  uart_type uart_ TA_GUARDED(lock_);

  IoProvider<typename uart_type::config_type, uart_type::kIoType> io_;
  IrqProvider irq_;
};

// These specializations are defined in the library to cover all the ZBI item
// payload types used by the various drivers.

template <>
std::optional<zbi_dcfg_simple_t> ParseConfig<zbi_dcfg_simple_t>(std::string_view string);

template <>
void UnparseConfig(const zbi_dcfg_simple_t& config, FILE* out);

template <>
std::optional<zbi_dcfg_simple_pio_t> ParseConfig<zbi_dcfg_simple_pio_t>(std::string_view string);

template <>
void UnparseConfig(const zbi_dcfg_simple_pio_t& config, FILE* out);

}  // namespace uart

#endif  // ZIRCON_SYSTEM_ULIB_UART_INCLUDE_LIB_UART_UART_H_
