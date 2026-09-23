// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dlfcn.h>
#include <lib/elfldltl/container.h>
#include <lib/elfldltl/diagnostics.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/machine.h>
#include <lib/elfldltl/vmar-loader.h>
#include <lib/elfldltl/vmo.h>
#include <lib/elfldltl/zircon.h>
#include <lib/zx/vmo.h>
#include <stdint.h>
#include <zircon/process.h>
#include <zircon/processargs.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <mini-process/mini-process.h>

#include "subprocess.h"
#include "subprocess_blob.h"

template <typename Fn>
static Fn* get_syscall_addr(Fn* syscall_fn, uintptr_t vdso_base) {
  Dl_info dl_info;
  if (dladdr(reinterpret_cast<const void*>(syscall_fn), &dl_info) == 0)
    return nullptr;
  return reinterpret_cast<Fn*>(vdso_base + (reinterpret_cast<uintptr_t>(dl_info.dli_saddr) -
                                            reinterpret_cast<uintptr_t>(dl_info.dli_fbase)));
}

static zx_status_t write_ctx_message(zx_handle_t channel, uintptr_t vdso_base,
                                     zx_handle_t transferred_handle) {
  minip_ctx_t ctx = {
      .handle_close = get_syscall_addr(&zx_handle_close, vdso_base),
      .object_wait_async = get_syscall_addr(&zx_object_wait_async, vdso_base),
      .object_wait_one = get_syscall_addr(&zx_object_wait_one, vdso_base),
      .object_signal = get_syscall_addr(&zx_object_signal, vdso_base),
      .event_create = get_syscall_addr(&zx_event_create, vdso_base),
      .profile_create = get_syscall_addr(&zx_profile_create, vdso_base),
      .channel_create = get_syscall_addr(&zx_channel_create, vdso_base),
      .channel_read = get_syscall_addr(&zx_channel_read, vdso_base),
      .channel_write = get_syscall_addr(&zx_channel_write, vdso_base),
      .process_exit = get_syscall_addr(&zx_process_exit, vdso_base),
      .object_get_info = get_syscall_addr(&zx_object_get_info, vdso_base),
      .port_cancel_key = get_syscall_addr(&zx_port_cancel_key, vdso_base),
      .port_create = get_syscall_addr(&zx_port_create, vdso_base),
      .pager_create = get_syscall_addr(&zx_pager_create, vdso_base),
      .pager_create_vmo = get_syscall_addr(&zx_pager_create_vmo, vdso_base),
      .vmo_contiguous_create = get_syscall_addr(&zx_vmo_create_contiguous, vdso_base),
      .vmo_physical_create = get_syscall_addr(&zx_vmo_create_physical, vdso_base),
      .vmo_replace_as_executable = get_syscall_addr(&zx_vmo_replace_as_executable, vdso_base),
      .thread_exit = get_syscall_addr(&zx_thread_exit, vdso_base),
      .system_get_page_size = get_syscall_addr(&zx_system_get_page_size, vdso_base),
      .vmo_read = get_syscall_addr(&zx_vmo_read, vdso_base),
      .vmo_write = get_syscall_addr(&zx_vmo_write, vdso_base),
  };
  uint32_t actual_handles = (transferred_handle == ZX_HANDLE_INVALID) ? 0u : 1u;
  return zx_channel_write(channel, 0u, &ctx, sizeof(ctx), &transferred_handle, actual_handles);
}

// Sets up a VMO on the given VMAR which contains the mini process code and space for a stack.
__EXPORT
zx_status_t mini_process_load_stack(zx_handle_t vmar, bool with_code, zx_vaddr_t* stack_base,
                                    zx_vaddr_t* sp) {
  // Allocate a single VMO for the child. It doubles as the stack on the top
  // and as the executable code (minipr_thread_loop()) at the bottom. In
  // theory, actual stack usage is minimal, like 160 bytes or less.

  cpp20::span<const std::byte> code_blob;
  if (with_code) {
    code_blob = subprocess_blob();
  }

  uint64_t stack_size = (16u << 10) + code_blob.size_bytes();
  const uint64_t page_size = zx_system_get_page_size();
  stack_size = (stack_size + page_size - 1) & -page_size;

  zx_handle_t stack_vmo = ZX_HANDLE_INVALID;
  zx_status_t status = zx_vmo_create(stack_size, 0, &stack_vmo);
  if (status != ZX_OK)
    return status;
  // Try to set the name, but ignore any errors since it's purely for
  // debugging and diagnostics.
  static const char vmo_name[] = "mini-process:stack";
  zx_object_set_property(stack_vmo, ZX_PROP_NAME, vmo_name, sizeof(vmo_name));

  zx_vm_option_t perms = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE;
  if (!code_blob.empty()) {
    status = zx_vmo_write(stack_vmo, code_blob.data(), 0, code_blob.size_bytes());
    if (status != ZX_OK)
      goto exit;

    // TODO(mdempsky): Separate minipr_thread_loop and stack into
    // separate VMOs to enforce W^X.
    status = zx_vmo_replace_as_executable(stack_vmo, ZX_HANDLE_INVALID, &stack_vmo);
    if (status != ZX_OK)
      goto exit;

    perms |= ZX_VM_PERM_EXECUTE;
  }

  status = zx_vmar_map(vmar, perms, 0, stack_vmo, 0, stack_size, stack_base);
  if (status != ZX_OK)
    goto exit;

  // Compute a valid starting SP for the machine's ABI.
  *sp = elfldltl::AbiTraits<>::InitialStackPointer(*stack_base, stack_size);

exit:
  // Close the VMO handle no matter what; if we failed we want to release it, and if
  // we succeeded the VMAR retains a reference to it so we don't need a handle.
  zx_handle_close(stack_vmo);
  return status;
}

__EXPORT
zx_status_t mini_process_load_vdso(zx_handle_t process, zx_handle_t vmar, uintptr_t* base,
                                   uintptr_t* entry) {
  // This steals the startup handle, so it's not compatible with anything else
  // that also needs to steal the startup handle.
  static const zx::vmo vdso_vmo{zx_take_startup_handle(PA_HND(PA_VMO_VDSO, 0))};
  ZX_ASSERT(vdso_vmo);

  // Ignore any error details, but capture the zx_status_t of a SystemError.
  // Tell the toolkit code to keep going after a SystemError if possible.  No
  // other kinds of errors should be possible since those would indicate an
  // invalid vDSO image.
  zx_status_t status = ZX_OK;
  auto report = [&status](auto&&... args) -> bool {
    auto check = [&status](auto arg) -> bool {
      if constexpr (std::is_same_v<decltype(arg), elfldltl::ZirconError>) {
        status = arg.status;
        return true;
      }
      return false;
    };
    return (check(args) || ...);
  };
  auto diag = elfldltl::Diagnostics(report, elfldltl::DiagnosticsPanicFlags());

  elfldltl::UnownedVmoFile file(vdso_vmo.borrow(), diag);
  elfldltl::LoadInfo<elfldltl::Elf<>, elfldltl::StdContainer<std::vector>::Container> load_info;
  elfldltl::RemoteVmarLoader loader{*zx::unowned_vmar{vmar}};
  if (auto headers = elfldltl::LoadHeadersFromFile<elfldltl::Elf<>>(
          diag, file,
          elfldltl::ContainerArrayFromFile<
              elfldltl::StdContainer<std::vector>::Container<elfldltl::Elf<>::Phdr>>(
              diag, "impossible"))) {
    auto& [ehdr, phdrs_result] = *headers;
    cpp20::span<const elfldltl::Elf<>::Phdr> phdrs = phdrs_result;
    if (elfldltl::DecodePhdrs(diag, phdrs, load_info.GetPhdrObserver(loader.page_size())) &&
        loader.Load(diag, load_info, vdso_vmo.borrow())) {
      ZX_ASSERT(status == ZX_OK);
      ZX_ASSERT(load_info.vaddr_start() == 0);
      if (base) {
        *base = loader.load_bias();
      }
      if (entry) {
        *entry = ehdr.entry + loader.load_bias();
      }
      std::ignore = std::move(loader).Commit(decltype(load_info)::Region{});
      return ZX_OK;
    }
  }

  ZX_ASSERT(status != ZX_OK);
  return status;
}

__EXPORT
zx_status_t mini_process_wait_for_ack(zx_handle_t control_channel) {
  zx_signals_t observed;
  zx_status_t status = zx_object_wait_one(
      control_channel, ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED, ZX_TIME_INFINITE, &observed);
  if (status != ZX_OK) {
    return status;
  }

  if (observed & ZX_CHANNEL_PEER_CLOSED) {
    // the child process died prematurely.
    return ZX_ERR_UNAVAILABLE;
  }

  if (observed & ZX_CHANNEL_READABLE) {
    uint32_t ack[2];
    uint32_t actual_handles;
    uint32_t actual_bytes;
    status = zx_channel_read(control_channel, 0u, ack, NULL, sizeof(uint32_t) * 2, 0u,
                             &actual_bytes, &actual_handles);
  } else {
    status = ZX_ERR_BAD_STATE;
  }

  return status;
}

__EXPORT
zx_status_t start_mini_process_etc(zx_handle_t process, zx_handle_t thread, zx_handle_t vmar,
                                   zx_handle_t transferred_handle, bool wait_for_ack,
                                   zx_handle_t* control_channel) {
  zx_vaddr_t stack_base = 0;
  zx_vaddr_t sp = 0;
  zx_status_t status = mini_process_load_stack(vmar, true, &stack_base, &sp);
  if (status != ZX_OK)
    return status;

  zx_handle_t chn[2] = {ZX_HANDLE_INVALID, ZX_HANDLE_INVALID};

  if (!control_channel) {
    // Simple mode /////////////////////////////////////////////////////////////
    // Don't map the VDSO, so the only thing the mini-process can do is busy-loop.
    // The handle sent to the process is just the caller's handle.
    status = zx_process_start(process, thread, stack_base, sp, transferred_handle, 0);
    transferred_handle = ZX_HANDLE_INVALID;

  } else {
    // Complex mode ////////////////////////////////////////////////////////////
    // The mini-process is going to run a simple request-response over a channel
    // So we need to:
    // 1- map the VDSO in the child process, without launchpad.
    // 2- create a channel and give one end to the child process.
    // 3- send a message with the rest of the syscall function addresses.
    // 4- wait for reply.

    status = zx_channel_create(0u, &chn[0], &chn[1]);
    if (status != ZX_OK)
      goto exit;

    uintptr_t vdso_base = 0;
    status = mini_process_load_vdso(process, vmar, &vdso_base, NULL);
    if (status != ZX_OK)
      goto exit;

    status = write_ctx_message(chn[0], vdso_base, transferred_handle);
    transferred_handle = ZX_HANDLE_INVALID;
    if (status != ZX_OK)
      goto exit;

    uintptr_t channel_read = (uintptr_t)get_syscall_addr(&zx_channel_read, vdso_base);

    status = zx_process_start(process, thread, stack_base, sp, chn[1], channel_read);
    chn[1] = ZX_HANDLE_INVALID;
    if (status != ZX_OK)
      goto exit;

    if (wait_for_ack) {
      status = mini_process_wait_for_ack(chn[0]);
      if (status != ZX_OK) {
        goto exit;
      }
    }

    *control_channel = chn[0];
    chn[0] = ZX_HANDLE_INVALID;
  }

exit:
  if (transferred_handle != ZX_HANDLE_INVALID)
    zx_handle_close(transferred_handle);
  if (chn[0] != ZX_HANDLE_INVALID)
    zx_handle_close(chn[0]);
  if (chn[1] != ZX_HANDLE_INVALID)
    zx_handle_close(chn[1]);

  return status;
}

__EXPORT
zx_status_t mini_process_cmd_send(zx_handle_t cntrl_channel, uint32_t what, zx_handle_t handle,
                                  uint64_t data) {
  minip_cmd_t cmd = {.what = what, .status = ZX_OK, .data = data};

  return zx_channel_write(cntrl_channel, 0, &cmd, sizeof(cmd),
                          (handle != ZX_HANDLE_INVALID) ? &handle : NULL,
                          (handle != ZX_HANDLE_INVALID) ? 1 : 0);
}

__EXPORT
zx_status_t mini_process_cmd_read_reply(zx_handle_t cntrl_channel, zx_handle_t* handle) {
  zx_status_t status = zx_object_wait_one(
      cntrl_channel, ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED, ZX_TIME_INFINITE, NULL);
  if (status != ZX_OK)
    return status;
  minip_cmd_t reply;
  uint32_t handle_count = handle ? 1 : 0;
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  status = zx_channel_read(cntrl_channel, 0, &reply, handle, sizeof(reply), handle_count,
                           &actual_bytes, &actual_handles);
  if (status != ZX_OK)
    return status;
  return reply.status;
}

__EXPORT
zx_status_t mini_process_cmd(zx_handle_t cntrl_channel, uint32_t what, zx_handle_t* handle,
                             uint64_t data) {
  zx_status_t status =
      mini_process_cmd_send(cntrl_channel, what, handle ? *handle : ZX_HANDLE_INVALID, data);
  if (status != ZX_OK)
    return status;
  return mini_process_cmd_read_reply(cntrl_channel, handle);
}

__EXPORT
zx_status_t start_mini_process(zx_handle_t job, zx_handle_t transferred_handle,
                               zx_handle_t* process, zx_handle_t* thread) {
  *process = ZX_HANDLE_INVALID;
  zx_handle_t vmar = ZX_HANDLE_INVALID;
  zx_handle_t channel = ZX_HANDLE_INVALID;

  zx_status_t status = zx_process_create(job, "minipr", 6u, 0u, process, &vmar);
  if (status != ZX_OK)
    goto exit;

  *thread = ZX_HANDLE_INVALID;
  status = zx_thread_create(*process, "minith", 6u, 0, thread);
  if (status != ZX_OK) {
    goto exit;
  }

  status = start_mini_process_etc(*process, *thread, vmar, transferred_handle, true, &channel);
  transferred_handle = ZX_HANDLE_INVALID;  // The transferred_handle gets consumed.
exit:
  if (status != ZX_OK) {
    if (transferred_handle != ZX_HANDLE_INVALID)
      zx_handle_close(transferred_handle);
    if (*process != ZX_HANDLE_INVALID)
      zx_handle_close(*process);
    if (*thread != ZX_HANDLE_INVALID)
      zx_handle_close(*thread);
  }

  if (channel != ZX_HANDLE_INVALID)
    zx_handle_close(channel);

  if (vmar != ZX_HANDLE_INVALID) {
    zx_handle_close(vmar);
  }

  return status;
}

__EXPORT
zx_status_t start_mini_process_thread(zx_handle_t thread, zx_handle_t vmar) {
  zx_vaddr_t stack_base = 0;
  zx_vaddr_t sp = 0;
  zx_status_t status = mini_process_load_stack(vmar, true, &stack_base, &sp);
  if (status != ZX_OK)
    return status;

  return zx_thread_start(thread, stack_base, sp, 0, 0);
}
