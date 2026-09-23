// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_PROCESSARGS_H_
#define LIB_LD_PROCESSARGS_H_

#include <lib/fdio/processargs.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <zircon/processargs.h>
#include <zircon/types.h>

#include <array>
#include <cassert>
#include <concepts>
#include <cstddef>
#include <span>
#include <string_view>
#include <type_traits>

namespace ld {

// To avoid dynamic stack allocation (via variable-length arrays), use fixed
// limits for the size of the processargs protocol message.  The version of the
// message sent to the dynamic linker (PT_INTERP) is always quite limited in
// contents.  The system's maximum number of handles does not make for overly
// large stack buffers.  The argument strings are not actually sent, and the
// environment strings only include the LD_DEBUG and/or LD_TRACE settings so
// the total string size will remain small.
constexpr uint32_t kProcessargsInterpStringSpace = 128;

// Given a whole <zircon/processargs.h> message buffer as a std::string_view,
// extract (up to) num strings by calling take as void(std::string_view).  This
// count should match the zx_proc_args_t::*_num value corresponding to the
// zx_proc_args_t::*_off value passed here.  The return value is exactly num if
// everything was normal, or shorter if the strings in the message were
// incorrectly truncated for the given *_off and *_num values from the header.
template <std::invocable<std::string_view> Take>
constexpr uint32_t GetProcessargsStrings(  //
    std::string_view message, uint32_t off, uint32_t num, Take&& take) {
  if (off > message.size()) [[unlikely]] {
    return 0;
  }
  std::string_view strings = message.substr(off);
  for (uint32_t i = 0; i < num; ++i) {
    size_t pos = strings.find_first_of('\0');
    if (pos == std::string_view::npos) [[unlikely]] {
      // Return a short count if the message is truncated.
      return i;
    }
    take(strings.substr(0, pos));
    strings.remove_prefix(pos + 1);
  }
  return num;
}

// Returns true if this is PA_FD type handle_info value value denotes the
// logging handle (stderr file descriptor).
constexpr bool IsProcessargsLogHandle(uint32_t info) {
  return PA_HND_TYPE(info) == PA_FD &&                     // Right type.
         ((PA_HND_ARG(info) & FDIO_FLAG_USE_FOR_STDIO) ||  // std{in,out,err}
          PA_HND_ARG(info) == 2);                          // STDERR_FILENO
}

// This helps manage a buffer to receive a message on the bootstrap channel in
// the legacy <zircon/processargs.h> protocol.  For the PT_INTERP message
// received by a startup dynamic linker, it can use small fixed buffer sizes.
// For the general processargs message (sent second when there is a PT_INTERP)
// that may be much larger, this can be used with 0 sizes and embedded as the
// first member in a larger buffer of dynamic size (or just `reinterpret_cast`
// from a sufficiently-aligned and -sized byte buffer pointer).  Sizing and
// allocating the zx_handle_t buffer for the channel read is not handled here.
template <uint32_t StringSpace = kProcessargsInterpStringSpace,
          uint32_t MaxHandles = ZX_CHANNEL_MAX_MSG_HANDLES>
struct ProcessargsBuffer {
  using HandlesBuffer = std::array<zx_handle_t, MaxHandles>;

  struct Actual {
    constexpr auto operator<=>(const Actual&) const = default;
    uint32_t bytes = 0, handles = 0;
  };

  // Returns true if the buffer starting with this and extending for a total of
  // actual_bytes and with actual_handles corresponding handles (as read from
  // the bootstrap channel) is a valid message in the <zircon/processargs.h>
  // protocol.  If so, it's safe to use the other accessor methods and public
  // members below.
  constexpr bool Valid(Actual actual) const {
    auto valid_magic = [&]() {
      return actual.bytes >= sizeof(header) &&           // Safe to look.
             header.protocol == ZX_PROCARGS_PROTOCOL &&  // Magic number OK.
             header.version == ZX_PROCARGS_VERSION;      // Version OK.
    };
    auto valid_handles = [&]() {
      return header.handle_info_off >= sizeof(header) &&
             header.handle_info_off % alignof(uint32_t) == 0 &&
             header.handle_info_off <= actual.bytes &&
             actual.bytes - (header.handle_info_off / sizeof(uint32_t)) >= actual.handles;
    };
    auto valid_strings = [bytes = actual.bytes](uint32_t off, uint32_t num) {
      return num == 0 || (off >= sizeof(zx_proc_args_t) &&
                          // The strings are only fully valid if there are
                          // `num` NUL terminators inside the buffer starting
                          // at `off`, but this just checks that it's even
                          // possible without actually finding all the NULs.
                          off < bytes && num <= bytes - off);
    };
    return valid_magic() && valid_handles() &&  //
           valid_strings(header.args_off, header.args_num) &&
           valid_strings(header.environ_off, header.environ_num) &&
           valid_strings(header.names_off, header.names_num);
  }

  // Make sure the channel has a message ready to be read.  The parent or
  // service that started the process might have started this process before
  // sending its bootstrap message.
  static zx::result<> Wait(zx::unowned_channel bootstrap) {
    zx_signals_t pending;
    zx_status_t status = bootstrap->wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), &pending);
    if (status != ZX_OK) [[unlikely]] {
      return zx::error{status};
    }
    assert(pending & ZX_CHANNEL_READABLE);
    return zx::ok();
  }

  // Wait as necessary and read the message into the buffer formed by this
  // object and optional additional buffer space after it if the optional
  // num_bytes argument is passed with the total buffer size starting at this.
  // The buffer size (sizeof(*this) or implicit) must be large enough for a
  // message that could fill all of handles.size() with handles.  On successful
  // return, the Actual::handles leading subspan of handles have been filled
  // along with the Actual::bytes of this buffer.  The message must be checked
  // with Valid() before the accessor methods below are used.
  zx::result<Actual> Read(zx::unowned_channel bootstrap, std::span<zx_handle_t> handles,
                          uint32_t num_bytes = sizeof(ProcessargsBuffer)) {
    assert(num_bytes >= sizeof(*this));
    assert(handles.size() >= handle_info_space.size());

    if (zx::result wait = Wait(bootstrap->borrow()); wait.is_error()) {
      return wait.take_error();
    }

    // Read the message into the buffer.
    const uint32_t num_handles = static_cast<uint32_t>(handles.size());
    Actual actual;
    zx_status_t status = bootstrap->read(0, this, handles.data(), num_bytes, num_handles,
                                         &actual.bytes, &actual.handles);
    return zx::make_result(status, actual);
  }

  // Wait as necessary and then peek to see how much buffer space is needed for
  // the pending message.
  static zx::result<Actual> Peek(zx::unowned_channel bootstrap) {
    if (zx::result wait = Wait(bootstrap->borrow()); wait.is_error()) {
      return wait.take_error();
    }
    Actual actual;
    zx_status_t status = bootstrap->read(0, nullptr, nullptr, 0, 0, &actual.bytes, &actual.handles);
    if (status == ZX_ERR_BUFFER_TOO_SMALL) [[likely]] {
      status = ZX_OK;  // This is the normal case.
    }
    return zx::make_result(status, actual);
  }

  // Read after successful Peek(), where the Actual Read() would return on
  // success is already known.
  zx::result<> ReadAfterPeek(zx::unowned_channel bootstrap, Actual num,
                             std::span<zx_handle_t> handles) {
    assert(handles.size() >= num.handles);
    Actual actual;
    if (zx_status_t status = bootstrap->read(0, this, handles.data(), num.bytes, num.handles,
                                             &actual.bytes, &actual.handles);
        status != ZX_OK) [[unlikely]] {
      return zx::error{status};
    }
    assert(actual == num);
    return zx::ok();
  }

  // Get the whole message buffer as raw chars.
  std::string_view message_chars(uint32_t actual_bytes) const {
    return {reinterpret_cast<const char*>(this), actual_bytes};
  }

  // Get the whole handle info table in the message.
  std::span<const uint32_t> handle_info(uint32_t actual_handles) const {
    const size_t off = header.handle_info_off / sizeof(uint32_t);
    return {reinterpret_cast<const uint32_t*>(this) + off, actual_handles};
  }
  std::span<uint32_t> handle_info(uint32_t actual_handles) {
    const size_t off = header.handle_info_off / sizeof(uint32_t);
    return {reinterpret_cast<uint32_t*>(this) + off, actual_handles};
  }

  // This returns chars containing at least the environ strings, but
  // possibly more.  This can be used to scan the NUL-terminated sequences
  // directly rather than splitting into the expected number of strings via
  // the environ_strings() method.
  std::string_view environ_chars(uint32_t actual_bytes) const {
    if (header.environ_num == 0) {
      return {};
    }
    return message_chars(actual_bytes).substr(header.environ_off);
  }

  uint32_t GetArgsStrings(uint32_t actual_bytes, std::invocable<std::string_view> auto take) const {
    return GetProcessargsStrings(message_chars(actual_bytes), header.args_off, header.args_num,
                                 std::move(take));
  }

  uint32_t GetEnvStrings(uint32_t actual_bytes, std::invocable<std::string_view> auto take) const {
    return GetProcessargsStrings(message_chars(actual_bytes), header.environ_off,
                                 header.environ_num, std::move(take));
  }

  uint32_t GetNameStrings(uint32_t actual_bytes, std::invocable<std::string_view> auto take) const {
    return GetProcessargsStrings(message_chars(actual_bytes), header.names_off, header.names_num,
                                 std::move(take));
  }

  // Aside from the header, this is not necessarily the actual layout of the
  // message buffer that will be received (though it's the optimal one for the
  // sender to choose), but it serves to approximate the maximum size it's
  // reasonable to handle.  The actual handle_info and strings portions of the
  // buffer are at whatever offsets the header fields indicate.
  zx_proc_args_t header;  // Must be first.
  [[no_unique_address]] std::array<uint32_t, MaxHandles> handle_info_space;
  [[no_unique_address]] std::array<char, StringSpace> string_space;
};
static_assert(std::is_trivially_default_constructible_v<ProcessargsBuffer<>>);
static_assert(offsetof(ProcessargsBuffer<>, header) == 0);

// The default template parameters result in a large but not unreasonable
// contribution to the stack frame size.
static_assert(sizeof(ProcessargsBuffer<>) < 1024);

// Just the header alone contributing to a dynamic buffer size is not large.
static_assert(sizeof(ProcessargsBuffer<0, 0>) < 48);

}  // namespace ld

#endif  // LIB_LD_PROCESSARGS_H_
