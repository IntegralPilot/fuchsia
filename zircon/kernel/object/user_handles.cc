// Copyright 2019 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/user_handles.h"

#include <ktl/algorithm.h>

#include <ktl/enforce.h>

namespace {

// Basic checks for a |handle| to be able to be sent via |channel|.
static zx_status_t common_handle_checks_locked(const Handle& handle, const Dispatcher* channel,
                                               zx_rights_t desired_rights, zx_obj_type_t type) {
  if (!handle.HasRights(ZX_RIGHT_TRANSFER))
    return ZX_ERR_ACCESS_DENIED;
  if (handle.dispatcher().get() == channel)
    return ZX_ERR_NOT_SUPPORTED;
  if (type != ZX_OBJ_TYPE_NONE && handle.dispatcher()->get_type() != type)
    return ZX_ERR_WRONG_TYPE;
  if (desired_rights != ZX_RIGHT_SAME_RIGHTS) {
    if ((handle.rights() & desired_rights) != desired_rights) {
      return ZX_ERR_INVALID_ARGS;
    }
  }
  return ZX_OK;
}

zx::result<HandleOwner> move_handle_for_transfer(HandleOwner handle, const Dispatcher* channel,
                                                 zx_handle_t handle_val, zx_obj_type_t type,
                                                 zx_rights_t desired_rights) {
  zx_status_t check_status = common_handle_checks_locked(*handle, channel, desired_rights, type);
  if (check_status != ZX_OK) {
    return zx::error(check_status);
  }

  // If the caller has requested a different set of rights, we have to mint a new handle for them.
  if (desired_rights != ZX_RIGHT_SAME_RIGHTS && desired_rights != handle->rights()) {
    // common_handle_checks_locked verifies that the desired rights are a subset of the handle's
    // current rights.
    auto descoped_rights_handle = Handle::Dup(*handle, desired_rights);
    if (!descoped_rights_handle) {
      return zx::error(ZX_ERR_NO_MEMORY);
    }
    handle = ktl::move(descoped_rights_handle);
  }

  return zx::ok(ktl::move(handle));
}

zx::result<HandleOwner> duplicate_handle_for_transfer(const Handle& source,
                                                      const Dispatcher* channel,
                                                      zx_handle_t handle_val, zx_obj_type_t type,
                                                      zx_rights_t desired_rights) {
  zx_status_t check_status = common_handle_checks_locked(source, channel, desired_rights, type);
  if (check_status != ZX_OK) {
    return zx::error(check_status);
  }

  if (!source.HasRights(ZX_RIGHT_DUPLICATE))
    return zx::error(ZX_ERR_ACCESS_DENIED);

  // common_handle_checks_locked verifies that the desired rights are a subset of the handle's
  // current rights.
  const auto dest_rights =
      (desired_rights == ZX_RIGHT_SAME_RIGHTS) ? source.rights() : desired_rights;

  auto duped_handle = Handle::Dup(source, dest_rights);
  if (!duped_handle) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(ktl::move(duped_handle));
}

}  // namespace

zx_status_t get_user_handles_to_consume(user_in_ptr<const zx_handle_t> user_handles, size_t offset,
                                        size_t chunk_size, zx_handle_t* handles) {
  return user_handles.copy_array_from_user(handles, chunk_size, offset);
}

zx_status_t get_user_handles_to_consume(user_inout_ptr<zx_handle_disposition_t> user_handles,
                                        size_t offset, size_t chunk_size, zx_handle_t* handles) {
  zx_handle_disposition_t local_handle_disposition[kMaxMessageHandles] = {};

  chunk_size = ktl::min<size_t>(chunk_size, kMaxMessageHandles);

  zx_status_t status =
      user_handles.copy_array_from_user(local_handle_disposition, chunk_size, offset);
  if (status != ZX_OK) {
    return status;
  }

  for (size_t i = 0; i < chunk_size; i++) {
    // !ZX_HANDLE_OP_DUPLICATE is used to capture the case where we failed
    // due to a bad operational arg.
    if (local_handle_disposition[i].operation != ZX_HANDLE_OP_DUPLICATE) {
      handles[i] = local_handle_disposition[i].handle;
    }
  }
  return ZX_OK;
}

// This overload is used by zx_channel_write.
zx::result<HandleOwner> get_handle_for_message_locked(ProcessDispatcher* process,
                                                      const Dispatcher* channel,
                                                      zx_handle_t handle_val) {
  HandleOwner source = process->handle_table().RemoveHandleLocked(*process, handle_val);
  if (!source) {
    return zx::error(ZX_ERR_BAD_HANDLE);
  }
  return move_handle_for_transfer(ktl::move(source), channel, handle_val, ZX_OBJ_TYPE_NONE,
                                  ZX_RIGHT_SAME_RIGHTS);
}

// This overload is used by zx_channel_write_etc.
zx::result<HandleOwner> get_handle_for_message_locked(ProcessDispatcher* process,
                                                      const Dispatcher* channel,
                                                      zx_handle_disposition_t& handle_disposition) {
  const zx_handle_op_t operation = handle_disposition.operation;
  const zx_rights_t desired_rights = handle_disposition.rights;
  const zx_obj_type_t type = handle_disposition.type;
  const zx_handle_t handle_val = handle_disposition.handle;

  zx::result<HandleOwner> operation_result =
      [&]() TA_REQ(process->handle_table().get_lock()) -> zx::result<HandleOwner> {
    auto source = process->handle_table().GetHandleLocked(*process, handle_val);
    if (!source) {
      return zx::error(ZX_ERR_BAD_HANDLE);
    }

    // The documentation for zx_channel_write_etc says this about the operation performed on
    // handles:
    /// The operation applied to *handle* is one of:
    ///
    /// *   `ZX_HANDLE_OP_MOVE` This is equivalent to first issuing [`zx_handle_replace()`] then
    ///      [`zx_channel_write()`]. The source handle is always closed.
    ///
    /// *   `ZX_HANDLE_OP_DUPLICATE` This is equivalent to first issuing [`zx_handle_duplicate()`]
    ///     then [`zx_channel_write()`]. The source handle always remains open and accessible to
    ///     the caller.
    // So when duplicating a handle, we leave the source handle in the handle table. For all other
    // operations (including invalid operations) we immediately remove the source handle from the
    // handle table and then attempt to move it.

    if (operation == ZX_HANDLE_OP_DUPLICATE) {
      return duplicate_handle_for_transfer(*source, channel, handle_val, type, desired_rights);
    }

    HandleOwner source_owner = process->handle_table().RemoveHandleLocked(source);
    if (operation == ZX_HANDLE_OP_MOVE) {
      return move_handle_for_transfer(ktl::move(source_owner), channel, handle_val, type,
                                      desired_rights);
    }
    return zx::error(ZX_ERR_INVALID_ARGS);
  }();

  if (operation_result.is_error()) {
    handle_disposition.result = operation_result.error_value();
  }
  return operation_result;
}
