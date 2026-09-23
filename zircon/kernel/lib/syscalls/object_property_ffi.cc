// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/user_copy/user_ptr.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <object/dispatcher.h>
#include <object/exception_dispatcher.h>
#include <object/stream_dispatcher.h>
#include <object/thread_dispatcher.h>
#include <object/vm_object_dispatcher.h>

#include "object_property_priv.h"

#include <ktl/enforce.h>

extern "C" {

zx_status_t cpp_object_get_property_cpp_types(const Dispatcher* dispatcher, uint32_t property,
                                              void* _value, size_t size) {
  user_out_ptr<void> value(_value);
  switch (property) {
    case ZX_PROP_EXCEPTION_STATE: {
      if (size < sizeof(uint32_t)) {
        return ZX_ERR_BUFFER_TOO_SMALL;
      }
      auto exception = DownCastDispatcher<const ExceptionDispatcher>(dispatcher);
      if (!exception) {
        return ZX_ERR_WRONG_TYPE;
      }
      return value.reinterpret<uint32_t>().copy_to_user(exception->GetDisposition());
    }
    case ZX_PROP_EXCEPTION_STRATEGY: {
      if (size < sizeof(uint32_t)) {
        return ZX_ERR_BUFFER_TOO_SMALL;
      }
      auto exception = DownCastDispatcher<const ExceptionDispatcher>(dispatcher);
      if (!exception) {
        return ZX_ERR_WRONG_TYPE;
      }
      bool second_chance = exception->IsSecondChance();
      return value.reinterpret<uint32_t>().copy_to_user(
          second_chance ? ZX_EXCEPTION_STRATEGY_SECOND_CHANCE : ZX_EXCEPTION_STRATEGY_FIRST_CHANCE);
    }
    case ZX_PROP_VMO_CONTENT_SIZE: {
      if (size < sizeof(uint64_t)) {
        return ZX_ERR_BUFFER_TOO_SMALL;
      }
      auto vmo = DownCastDispatcher<const VmObjectDispatcher>(dispatcher);
      if (!vmo) {
        return ZX_ERR_WRONG_TYPE;
      }

      uint64_t stream_size = vmo->GetStreamSize();
      return value.reinterpret<uint64_t>().copy_to_user(stream_size);
    }
    case ZX_PROP_STREAM_MODE_APPEND: {
      if (size < sizeof(uint8_t)) {
        return ZX_ERR_BUFFER_TOO_SMALL;
      }
      auto stream = DownCastDispatcher<const StreamDispatcher>(dispatcher);
      if (!stream) {
        return ZX_ERR_WRONG_TYPE;
      }
      uint8_t val = stream->IsInAppendMode();
      return value.reinterpret<uint8_t>().copy_to_user(val);
    }
    default:
      return ZX_ERR_NOT_SUPPORTED;
  }
}

zx_status_t cpp_object_set_property_cpp_types(Dispatcher* dispatcher, uint32_t property,
                                              const void* _value, size_t size, zx_rights_t rights) {
  user_in_ptr<const void> value(_value);
  switch (property) {
    case ZX_PROP_EXCEPTION_STATE: {
      if (size < sizeof(uint32_t)) {
        return ZX_ERR_BUFFER_TOO_SMALL;
      }
      auto exception = DownCastDispatcher<ExceptionDispatcher>(dispatcher);
      if (!exception) {
        return ZX_ERR_WRONG_TYPE;
      }
      uint32_t val = 0;
      zx_status_t status = value.reinterpret<const uint32_t>().copy_from_user(&val);
      if (status != ZX_OK) {
        return status;
      }
      if (val == ZX_EXCEPTION_STATE_HANDLED) {
        exception->SetDisposition(ZX_EXCEPTION_STATE_HANDLED);
      } else if (val == ZX_EXCEPTION_STATE_TRY_NEXT) {
        exception->SetDisposition(ZX_EXCEPTION_STATE_TRY_NEXT);
      } else if (val == ZX_EXCEPTION_STATE_THREAD_EXIT) {
        exception->SetDisposition(ZX_EXCEPTION_STATE_THREAD_EXIT);
      } else {
        return ZX_ERR_INVALID_ARGS;
      }
      return ZX_OK;
    }
    case ZX_PROP_EXCEPTION_STRATEGY: {
      if (size < sizeof(uint32_t)) {
        return ZX_ERR_BUFFER_TOO_SMALL;
      }
      auto exception = DownCastDispatcher<ExceptionDispatcher>(dispatcher);
      if (!exception) {
        return ZX_ERR_WRONG_TYPE;
      }
      const zx_info_thread_t info = exception->thread()->GetInfoForUserspace();
      // Invalid if the exception handle is not held by a debugger.
      if (info.wait_exception_channel_type != ZX_EXCEPTION_CHANNEL_TYPE_DEBUGGER) {
        return ZX_ERR_BAD_STATE;
      }
      uint32_t val = 0;
      zx_status_t status = value.reinterpret<const uint32_t>().copy_from_user(&val);
      if (status != ZX_OK) {
        return status;
      }
      if (val == ZX_EXCEPTION_STRATEGY_FIRST_CHANCE) {
        exception->SetWhetherSecondChance(false);
      } else if (val == ZX_EXCEPTION_STRATEGY_SECOND_CHANCE) {
        exception->SetWhetherSecondChance(true);
      } else {
        return ZX_ERR_INVALID_ARGS;
      }
      return ZX_OK;
    }
    case ZX_PROP_VMO_CONTENT_SIZE: {
      if ((rights & ZX_RIGHT_WRITE) == 0) {
        return ZX_ERR_ACCESS_DENIED;
      }
      if (size < sizeof(uint64_t)) {
        return ZX_ERR_BUFFER_TOO_SMALL;
      }
      auto vmo = DownCastDispatcher<VmObjectDispatcher>(dispatcher);
      if (!vmo) {
        return ZX_ERR_WRONG_TYPE;
      }
      uint64_t stream_size = 0;
      zx_status_t status = value.reinterpret<const uint64_t>().copy_from_user(&stream_size);
      if (status != ZX_OK) {
        return status;
      }
      return vmo->SetStreamSize(stream_size);
    }
    case ZX_PROP_STREAM_MODE_APPEND: {
      if (size < sizeof(uint8_t)) {
        return ZX_ERR_BUFFER_TOO_SMALL;
      }
      auto stream = DownCastDispatcher<StreamDispatcher>(dispatcher);
      if (!stream) {
        return ZX_ERR_WRONG_TYPE;
      }
      uint8_t val = 0;
      zx_status_t status = value.reinterpret<const uint8_t>().copy_from_user(&val);
      if (status != ZX_OK) {
        return status;
      }
      return stream->SetAppendMode(val);
    }
    default:
      return ZX_ERR_NOT_SUPPORTED;
  }
}

}  // extern "C"
