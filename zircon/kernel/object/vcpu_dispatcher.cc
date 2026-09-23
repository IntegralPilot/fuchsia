// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/vcpu_dispatcher.h"

#include <lib/counters.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <arch/hypervisor.h>
#include <fbl/alloc_checker.h>
#include <ktl/type_traits.h>
#include <object/guest_dispatcher.h>
#include <vm/vm_object.h>

KCOUNTER(dispatcher_vcpu_create_count, "dispatcher.vcpu.create")
KCOUNTER(dispatcher_vcpu_destroy_count, "dispatcher.vcpu.destroy")

zx_status_t VcpuDispatcher::Create(const fbl::RefPtr<GuestDispatcher>& guest_dispatcher,
                                   zx_vaddr_t entry, KernelHandle<VcpuDispatcher>* handle,
                                   zx_rights_t* rights) {
  auto vcpu = Vcpu::Create(guest_dispatcher->guest(), entry);
  if (vcpu.is_error()) {
    return vcpu.status_value();
  }

  fbl::AllocChecker ac;
  KernelHandle new_handle(
      fbl::AdoptRef(new (&ac) VcpuDispatcher(guest_dispatcher, ktl::move(*vcpu))));
  if (!ac.check())
    return ZX_ERR_NO_MEMORY;

  *rights = default_rights();
  *handle = ktl::move(new_handle);
  return ZX_OK;
}

VcpuDispatcher::VcpuDispatcher(fbl::RefPtr<GuestDispatcher> guest_dispatcher,
                               ktl::unique_ptr<Vcpu> vcpu)
    : guest_dispatcher_(std::move(guest_dispatcher)), vcpu_(ktl::move(vcpu)) {
  kcounter_add(dispatcher_vcpu_create_count, 1);
}

VcpuDispatcher::~VcpuDispatcher() { kcounter_add(dispatcher_vcpu_destroy_count, 1); }

zx_status_t VcpuDispatcher::Enter(zx_port_packet_t& packet) {
  canary_.Assert();
  return vcpu_->Enter(packet).status_value();
}

void VcpuDispatcher::Kick() {
  canary_.Assert();
  vcpu_->Kick();
}

zx_status_t VcpuDispatcher::Interrupt(uint32_t vector) {
  canary_.Assert();
  return vcpu_->Interrupt(vector).status_value();
}

zx_status_t VcpuDispatcher::ReadState(zx_vcpu_state_t& vcpu_state) const {
  canary_.Assert();
  return vcpu_->ReadState(vcpu_state).status_value();
}

zx_status_t VcpuDispatcher::WriteState(const zx_vcpu_state_t& vcpu_state) {
  canary_.Assert();
  return vcpu_->WriteState(vcpu_state).status_value();
}

zx_status_t VcpuDispatcher::WriteState(const zx_vcpu_io_t& io_state) {
  canary_.Assert();
  return vcpu_->WriteState(io_state).status_value();
}

zx_info_vcpu_t VcpuDispatcher::GetInfo() const {
  canary_.Assert();
  return vcpu_->GetInfo();
}
