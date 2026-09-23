// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/processargs.h>
#include <lib/zircon-internal/unique-backtrace.h>
#include <zircon/assert.h>
#include <zircon/startup.h>

#include <cinttypes>
#include <cstddef>
#include <new>

#include "../zircon/vmar.h"
#include "processargs.h"
#include "src/__support/math_extras.h"

namespace LIBC_NAMESPACE_DECL {
namespace {

using Buffer = Processargs::Buffer;

// Each string table is just concatenated NUL-terminated strings.  There's no
// way to determine the size of the whole table before scanning it.  But it
// won't be scanned until it's being copied.  That's done by considering the
// tail of the message to be a single string table that goes from the
// earliest of the individual string tables to the end of the whole message.
uint32_t StrtabStart(const Buffer& buffer, Buffer::Actual actual) {
  uint32_t first_strtab = std::min({
      buffer.header.args_off,
      buffer.header.environ_off,
      buffer.header.names_off,
  });
  return std::min(first_strtab, actual.bytes);
}

zx::result<size_t> AllocationSize(const Buffer& buffer, Buffer::Actual actual,
                                  uint32_t strtab_off) {
  // Each of these members is uint32_t, but promoting them to size_t should
  // prevent some overflow when size_t is 64 bits.
  size_t args_num = buffer.header.args_num;
  size_t environ_num = buffer.header.environ_num;
  size_t handles = actual.handles;
  size_t bytes = actual.bytes;

  size_t size = sizeof(Processargs);
  size_t total_ptrs;

  // We group all pointer counts together to reduce operations.
  if (add_overflow(args_num, environ_num, total_ptrs))
    return zx::error(ZX_ERR_NO_RESOURCES);

  // +1 for nullptr terminator after argv().last()
  // +1 for nullptr terminator after envp().last()
  // +2 for two more zero words to stand in for the traditional
  // ELF-based Unix layout where the envp array is followed by the auxv array
  // of two-word entries AT_NULL (zero) being the terminator.  This wastes
  // only two words just in case some program tries to look past envp[envc]
  // for an absent auxv.
  if (add_overflow(total_ptrs, size_t{4}, total_ptrs))
    return zx::error(ZX_ERR_NO_RESOURCES);

  size_t total_ptrs_size;
  if (mul_overflow(total_ptrs, sizeof(char*), total_ptrs_size))
    return zx::error(ZX_ERR_NO_RESOURCES);
  if (add_overflow(size, total_ptrs_size, size))
    return zx::error(ZX_ERR_NO_RESOURCES);

  // Then there's space for the handle-indexed arrays.
  size_t handles_part_size = sizeof(uint32_t) + sizeof(zx_handle_t);
  if (mul_overflow(handles, handles_part_size, handles_part_size))
    return zx::error(ZX_ERR_NO_RESOURCES);
  if (add_overflow(size, handles_part_size, size))
    return zx::error(ZX_ERR_NO_RESOURCES);

  // And finally the strings.
  size_t strings_size;
  if (sub_overflow(bytes, static_cast<size_t>(strtab_off), strings_size))
    return zx::error(ZX_ERR_NO_RESOURCES);
  if (add_overflow(size, strings_size, size))
    return zx::error(ZX_ERR_NO_RESOURCES);

  return zx::ok(size);
}

}  // namespace

// This is only called here, so it doesn't need to be in the header.
Processargs::Processargs(const Buffer& msg, std::span<const zx_handle_t> msg_handles,
                         Buffer::Actual actual, uint32_t strtab_off)
    : argc_{msg.header.args_num},
      envc_{msg.header.environ_num},
      namec_{msg.header.names_num},
      handlec_{actual.handles},
      strtab_size_{actual.bytes - strtab_off} {
  // Copy the handles and the handle info table.
  assert(msg_handles.size() == handles().size());
  std::ranges::copy(msg_handles, handles().begin());
  std::ranges::copy(msg.handle_info(actual.handles), handle_info().begin());

  // The copied strings will go after those tables.
  uint32_t* arrays_end = handle_arrays().data() + handle_arrays().size();
  std::span strtab{reinterpret_cast<char*>(arrays_end), strtab_size_};

  // Fill the string arrays while copying the strings.
  auto fill_array = [&strtab](std::span<char*> array) {
    return [it = array.begin(), &strtab](std::string_view str) mutable {
      size_t len = str.copy(strtab.data(), strtab.size());
      *it++ = strtab.data();
      // The block is zero-initialized, so there's a NUL terminator already.
      strtab = strtab.subspan(len + 1);
    };
  };

  // The array sizes can be reduced if the message is truncated.
  argc_ = msg.GetArgsStrings(actual.bytes, fill_array(argv()));
  envc_ = msg.GetEnvStrings(actual.bytes, fill_array(envp()));
  namec_ = msg.GetNameStrings(actual.bytes, fill_array(names()));
}

zx_startup_handles_t Processargs::GetHandles(zx_handle_t bootstrap_handle) {
  zx::channel bootstrap{bootstrap_handle};

  // First just check the size of the incoming message without reading it.
  Processargs::Buffer::Actual actual;
  if (zx::result peek = Processargs::Buffer::Peek(bootstrap.borrow()); peek.is_ok()) [[likely]] {
    actual = *peek;
  } else {
    ZX_PANIC("cannot fetch bootstrap message size: %s", peek.status_string());
  }

  // Use stack space for the message and handles buffers.
  alignas(Processargs::Buffer) std::byte buffer[actual.bytes];
  zx_handle_t handles_buffer[actual.handles];
  Processargs::Buffer* message = new (buffer) Processargs::Buffer;
  std::span handles{handles_buffer, actual.handles};

  // Now read the message onto the stack.
  if (zx::result read = message->ReadAfterPeek(bootstrap.borrow(), actual, handles);
      read.is_error()) [[unlikely]] {
    ZX_PANIC("cannot read bootstrap message of %" PRIu32 " bytes, %" PRIu32 " handles: %s",
             actual.bytes, actual.handles, read.status_string());
  }

  if (!message->Valid(actual)) [[unlikely]] {
    ZX_PANIC(
        "invalid zx_proc_args_t format in bootstrap message"
        " of %" PRIu32 " bytes, %" PRIu32 " handles",
        actual.bytes, actual.handles);
  }

  // Collect the essential handles consumed by basic libc startup.
  zx_startup_handles_t startup_handles = {};
  std::span handle_info = message->handle_info(actual.handles);
  for (auto [info, take] : Processargs::HandleTakers(handle_info, handles)) {
    switch (PA_HND_TYPE(info)) {
      case PA_PROC_SELF:
        startup_handles.process_self = take().release();
        continue;
      case PA_THREAD_SELF:
        startup_handles.thread_self = take().release();
        continue;
      case PA_VMAR_ROOT:
        startup_handles.allocation_vmar = take().release();
        continue;
      case PA_VMAR_LOADED:
        startup_handles.executable_image_vmar = take().release();
        continue;
      default:  // Everything else will be picked up in the second phase.
        break;
    }
  }

  zx::unowned_vmar vmar{startup_handles.allocation_vmar};
  if (!*vmar) [[unlikely]] {
    ZX_PANIC("no root VMAR in processargs message; cannot allocate memory");
  }

  // Allocate a block to contain the saved data.
  PageRoundedSize guard_size = PageRoundedSize::Page();
  const uint32_t strtab_off = StrtabStart(*message, actual);

  zx::result<size_t> size_result = AllocationSize(*message, actual, strtab_off);
  if (size_result.is_error()) [[unlikely]] {
    ZX_PANIC("process arguments allocation size overflow: environ_num=%" PRIu32
             ", args_num=%" PRIu32 ", bytes=%" PRIu32 ", strtab_off=%" PRIu32,
             message->header.environ_num, message->header.args_num, actual.bytes, strtab_off);
  }
  const PageRoundedSize block_size = *PageRoundedSize::From(*size_result);
  zx::result vmo = AllocationVmo::New(block_size);
  if (vmo.is_error()) [[unlikely]] {
    ZX_PANIC("cannot allocate VMO %zu bytes for process arguments", block_size.get());
  }
  GuardedPageBlock block;
  zx::result allocate = block.Allocate<Processargs>(  //
      vmar->borrow(), *vmo, block_size, guard_size, guard_size);
  if (allocate.is_error()) [[unlikely]] {
    ZX_PANIC("cannot map block of %zu bytes for process arguments", block_size.get());
  }

  // Save everything into the new block.
  startup_handles.hook = new (allocate->data()) Processargs(  //
      *message, handles, actual, strtab_off);

  // The block will never be freed; its ownership is no longer tracked.
  std::ignore = block.release();

  return startup_handles;
}

}  // namespace LIBC_NAMESPACE_DECL

// Give it the <zircon/startup.h> name too.
decltype(_zx_startup_get_handles) _zx_startup_get_handles
    [[gnu::alias(LIBC_ASM_LINKAGE_STRING(ProcessargsGetHandles))]];
