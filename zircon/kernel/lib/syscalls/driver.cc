// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <align.h>
#include <lib/fit/defer.h>
#include <lib/page/size.h>
#include <lib/user_copy/user_ptr.h>
#include <platform.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls/iommu.h>
#include <zircon/syscalls/smc.h>
#include <zircon/types.h>

#include <new>

#include <dev/interrupt.h>
#include <dev/iommu/iommu.h>
#include <fbl/inline_array.h>
#include <object/bus_transaction_initiator_dispatcher.h>
#include <object/handle.h>
#include <object/interrupt_dispatcher.h>
#include <object/interrupt_event_dispatcher.h>
#include <object/iommu_dispatcher.h>
#include <object/process_dispatcher.h>
#include <object/resource.h>
#include <object/vcpu_dispatcher.h>
#include <object/virtual_interrupt_dispatcher.h>
#include <object/vm_object_dispatcher.h>
#include <vm/vm.h>
#include <vm/vm_object_paged.h>
#include <vm/vm_object_physical.h>

#if ARCH_X86
#include <platform/pc/smbios.h>
#endif

#include <lib/syscalls/forward.h>

#define LOCAL_TRACE 0

// zx_status_t zx_vmo_create_contiguous
zx_status_t sys_vmo_create_contiguous(zx_handle_t bti, size_t size, uint32_t alignment_log2,
                                      zx_handle_t* out) {
  LTRACEF("size 0x%zu\n", size);

  if (size == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (!IsPageRounded(size)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (alignment_log2 == 0) {
    alignment_log2 = kPageShift;
  }
  // catch obviously wrong values
  if (alignment_log2 < kPageShift || alignment_log2 >= (8 * sizeof(uint64_t))) {
    return ZX_ERR_INVALID_ARGS;
  }

  auto up = ProcessDispatcher::GetCurrent();
  zx_status_t status = up->EnforceBasicPolicy(ZX_POL_NEW_VMO);
  if (status != ZX_OK) {
    return status;
  }

  fbl::RefPtr<BusTransactionInitiatorDispatcher> bti_dispatcher;
  status = up->handle_table().GetDispatcherWithRights(*up, bti, ZX_RIGHT_MAP, &bti_dispatcher);
  if (status != ZX_OK) {
    return status;
  }

  auto align_log2_arg = static_cast<uint8_t>(alignment_log2);

  // create a vm object
  fbl::RefPtr<VmObjectPaged> vmo;
  status = VmObjectPaged::CreateContiguous(PMM_ALLOC_FLAG_ANY, size, align_log2_arg, &vmo);
  if (status != ZX_OK) {
    return status;
  }

  // create a Vm Object dispatcher
  KernelHandle<VmObjectDispatcher> kernel_handle;
  zx_rights_t rights;
  status = VmObjectDispatcher::Create(ktl::move(vmo), size,
                                      VmObjectDispatcher::InitialMutability::kMutable,
                                      &kernel_handle, &rights);
  if (status != ZX_OK) {
    return status;
  }

  // create a handle and attach the dispatcher to it
  return up->MakeAndAddHandle(ktl::move(kernel_handle), rights, out);
}

// zx_status_t zx_vmo_create_physical
zx_status_t sys_vmo_create_physical(zx_handle_t hrsrc, zx_paddr_t paddr, size_t size,
                                    zx_handle_t* out) {
  LTRACEF("size 0x%zu\n", size);

  if (size == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (!IsPageRounded(size)) {
    return ZX_ERR_INVALID_ARGS;
  }

  auto up = ProcessDispatcher::GetCurrent();
  zx_status_t status = up->EnforceBasicPolicy(ZX_POL_NEW_VMO);
  if (status != ZX_OK) {
    return status;
  }

  // Memory should be subtracted from the PhysicalAspace allocators, so it's
  // safe to assume that if the caller has access to a resource for this specified
  // region of MMIO space then it is safe to allow the vmo to be created.
  if ((status = validate_resource_mmio(hrsrc, paddr, size)) != ZX_OK) {
    return status;
  }

  // create a vm object
  fbl::RefPtr<VmObjectPhysical> vmo;
  status = VmObjectPhysical::Create(paddr, size, &vmo);
  if (status != ZX_OK) {
    return status;
  }

  // create a Vm Object dispatcher
  KernelHandle<VmObjectDispatcher> kernel_handle;
  zx_rights_t rights;
  status = VmObjectDispatcher::Create(ktl::move(vmo), size,
                                      VmObjectDispatcher::InitialMutability::kMutable,
                                      &kernel_handle, &rights);
  if (status != ZX_OK) {
    return status;
  }

  // create a handle and attach the dispatcher to it
  return up->MakeAndAddHandle(ktl::move(kernel_handle), rights, out);
}

#if ARCH_X86
#include <arch/x86/descriptor.h>
#include <arch/x86/ioport.h>

// zx_status_t zx_ioports_request
zx_status_t sys_ioports_request(zx_handle_t hrsrc, uint16_t io_addr, uint32_t len) {
  zx_status_t status;
  if ((status = validate_resource_ioport(hrsrc, io_addr, len)) != ZX_OK) {
    return status;
  }

  LTRACEF("addr 0x%x len 0x%x\n", io_addr, len);

  return IoBitmap::GetCurrent()->SetIoBitmap(io_addr, len, /*enable=*/true);
}

// zx_status_t zx_ioports_release
zx_status_t sys_ioports_release(zx_handle_t hrsrc, uint16_t io_addr, uint32_t len) {
  zx_status_t status;
  if ((status = validate_resource_ioport(hrsrc, io_addr, len)) != ZX_OK) {
    return status;
  }

  LTRACEF("addr 0x%x len 0x%x\n", io_addr, len);

  return IoBitmap::GetCurrent()->SetIoBitmap(io_addr, len, /*enable=*/false);
}

#else
// zx_status_t zx_ioports_request
zx_status_t sys_ioports_request(zx_handle_t hrsrc, uint16_t io_addr, uint32_t len) {
  // doesn't make sense on non-x86
  return ZX_ERR_NOT_SUPPORTED;
}

// zx_status_t zx_ioports_release
zx_status_t sys_ioports_release(zx_handle_t hrsrc, uint16_t io_addr, uint32_t len) {
  return ZX_ERR_NOT_SUPPORTED;
}
#endif

// zx_status_t zx_interrupt_create
zx_status_t sys_interrupt_create(zx_handle_t src_obj, uint32_t src_num, uint32_t options,
                                 zx_handle_t* out_handle) {
  LTRACEF("options 0x%x\n", options);

  // The IRQ resource is required for all non-virtual interrupts, and virtual
  // interrupts which are configured as wake vectors.
  if (!(options & ZX_INTERRUPT_VIRTUAL) || (options & ZX_INTERRUPT_WAKE_VECTOR)) {
    zx_status_t status;
    if ((status = validate_resource_irq(src_obj, src_num)) != ZX_OK) {
      return status;
    }
  }

  KernelHandle<InterruptDispatcher> handle;
  zx_rights_t rights;
  zx_status_t result;
  if (options & ZX_INTERRUPT_VIRTUAL) {
    result = VirtualInterruptDispatcher::Create(&handle, &rights, options);
  } else {
    result = InterruptEventDispatcher::Create(&handle, &rights, src_num, options);
  }
  if (result != ZX_OK) {
    return result;
  }

  return ProcessDispatcher::GetCurrent()->MakeAndAddHandle(ktl::move(handle), rights, out_handle);
}

// zx_status_t zx_interrupt_bind
zx_status_t sys_interrupt_bind(zx_handle_t handle, zx_handle_t port_handle, uint64_t key,
                               uint32_t options) {
  LTRACEF("handle %x\n", handle);
  if ((options != ZX_INTERRUPT_BIND) && (options != ZX_INTERRUPT_UNBIND)) {
    return ZX_ERR_INVALID_ARGS;
  }

  zx_status_t status;
  auto up = ProcessDispatcher::GetCurrent();
  fbl::RefPtr<InterruptDispatcher> interrupt;
  status = up->handle_table().GetDispatcherWithRights(*up, handle, ZX_RIGHT_READ, &interrupt);
  if (status != ZX_OK) {
    return status;
  }

  fbl::RefPtr<PortDispatcher> port;
  status = up->handle_table().GetDispatcherWithRights(*up, port_handle, ZX_RIGHT_WRITE, &port);
  if (status != ZX_OK) {
    return status;
  }
  if (!port->can_bind_to_interrupt()) {
    return ZX_ERR_WRONG_TYPE;
  }

  if (options == ZX_INTERRUPT_BIND) {
    return interrupt->Bind(ktl::move(port), key);
  } else {
    return interrupt->Unbind(ktl::move(port));
  }
}

// zx_status_t zx_interrupt_ack
zx_status_t sys_interrupt_ack(zx_handle_t inth) {
  LTRACEF("handle %x\n", inth);

  zx_status_t status;
  auto up = ProcessDispatcher::GetCurrent();
  fbl::RefPtr<InterruptDispatcher> interrupt;
  status = up->handle_table().GetDispatcherWithRights(*up, inth, ZX_RIGHT_WRITE, &interrupt);
  if (status != ZX_OK) {
    return status;
  }
  return interrupt->Ack();
}

// zx_status_t zx_interrupt_wait
zx_status_t sys_interrupt_wait(zx_handle_t handle, user_out_ptr<zx_time_t> out_timestamp) {
  LTRACEF("handle %x\n", handle);

  zx_status_t status;
  auto up = ProcessDispatcher::GetCurrent();
  fbl::RefPtr<InterruptDispatcher> interrupt;
  status = up->handle_table().GetDispatcherWithRights(*up, handle, ZX_RIGHT_WAIT, &interrupt);
  if (status != ZX_OK) {
    return status;
  }

  zx_time_t timestamp;
  status = interrupt->WaitForInterrupt(&timestamp);
  if (status == ZX_OK && out_timestamp) {
    status = out_timestamp.copy_to_user(timestamp);
  }

  return status;
}

// zx_status_t zx_interrupt_destroy
zx_status_t sys_interrupt_destroy(zx_handle_t handle) {
  LTRACEF("handle %x\n", handle);

  zx_status_t status;
  auto up = ProcessDispatcher::GetCurrent();
  fbl::RefPtr<InterruptDispatcher> interrupt;
  status = up->handle_table().GetDispatcher(*up, handle, &interrupt);
  if (status != ZX_OK) {
    return status;
  }

  return interrupt->Destroy();
}

// zx_status_t zx_interrupt_trigger
zx_status_t sys_interrupt_trigger(zx_handle_t handle, uint32_t options, zx_time_t timestamp) {
  LTRACEF("handle %x\n", handle);

  if (options) {
    return ZX_ERR_INVALID_ARGS;
  }

  zx_status_t status;
  auto up = ProcessDispatcher::GetCurrent();
  fbl::RefPtr<InterruptDispatcher> interrupt;
  status = up->handle_table().GetDispatcherWithRights(*up, handle, ZX_RIGHT_SIGNAL, &interrupt);
  if (status != ZX_OK) {
    return status;
  }

  return interrupt->Trigger(timestamp);
}
