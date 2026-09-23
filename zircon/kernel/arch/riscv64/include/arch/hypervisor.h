// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_HYPERVISOR_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_HYPERVISOR_H_

#include <debug.h>
#include <lib/zx/result.h>
#include <zircon/syscalls/hypervisor.h>

#include <fbl/ref_ptr.h>
#include <ktl/unique_ptr.h>

// Nulled out RISC-V implementation of Guest and Vcpu objects. There is currently
// no support for this architecture, so this is the minimum to cleanly compile the
// layers above.

typedef struct zx_port_packet zx_port_packet_t;
class PortDispatcher;
class VmAddressRegion;

// Represents a guest within the hypervisor.
class Guest {
 public:
  static zx::result<ktl::unique_ptr<Guest>> Create() { return zx::error(ZX_ERR_NOT_SUPPORTED); }

  ~Guest() = default;

  Guest(Guest&&) = delete;
  Guest& operator=(Guest&&) = delete;
  Guest(const Guest&) = delete;
  Guest& operator=(const Guest&) = delete;

  zx::result<> SetTrap(uint32_t kind, zx_vaddr_t addr, size_t len, fbl::RefPtr<PortDispatcher> port,
                       uint64_t key) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  fbl::RefPtr<VmAddressRegion> RootVmar() const { return nullptr; }
};

using NormalGuest = Guest;

// Represents a virtual CPU within a guest.
class Vcpu {
 public:
  static zx::result<ktl::unique_ptr<Vcpu>> Create(Guest& guest, zx_vaddr_t entry) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  ~Vcpu() = default;

  Vcpu(Vcpu&&) = delete;
  Vcpu& operator=(Vcpu&&) = delete;
  Vcpu(const Vcpu&) = delete;
  Vcpu& operator=(const Vcpu&) = delete;

  zx::result<> Enter(zx_port_packet_t& packet) { return zx::error(ZX_ERR_NOT_SUPPORTED); }
  void Kick() { PANIC_UNIMPLEMENTED; }
  zx::result<> Interrupt(uint32_t vector) { return zx::error(ZX_ERR_NOT_SUPPORTED); }
  zx::result<> ReadState(zx_vcpu_state_t& state) const { return zx::error(ZX_ERR_NOT_SUPPORTED); }
  zx::result<> WriteState(const zx_vcpu_state_t& state) { return zx::error(ZX_ERR_NOT_SUPPORTED); }
  zx::result<> WriteState(const zx_vcpu_io_t& io_state) { return zx::error(ZX_ERR_NOT_SUPPORTED); }

  zx_info_vcpu_t GetInfo() const { return {}; }
};

using NormalVcpu = Vcpu;

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_HYPERVISOR_H_
