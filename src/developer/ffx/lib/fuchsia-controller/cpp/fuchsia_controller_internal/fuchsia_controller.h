// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FUCHSIA_CONTROLLER_INTERNAL_FUCHSIA_CONTROLLER_H_
#define SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FUCHSIA_CONTROLLER_INTERNAL_FUCHSIA_CONTROLLER_H_
// LINT.IfChange
#include <stdint.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#ifdef __cplusplus
extern "C" {
#endif  // __cplusplus

struct ffx_lib_context_t;
struct ffx_env_context_t;

typedef struct _ffx_config_t {
  const char* key;
  const char* value;
} ffx_config_t;

// Determines the type of error from `ffx_get_last_error`.
typedef int32_t fc_err_type_t;

#define FC_ERR_TYPE_NONE (0)
#define FC_ERR_TYPE_STRING (1)
#define FC_ERR_TYPE_FIDL (2)

// Similar to zx_status_t but for fuchsia-controller specifically.
typedef int32_t fc_status_t;
// Lint suppressed because we are asserting on the sizeof(long),
// not int64. This is used to ensure that the exceptions we're setting can
// actually fit (when using them).
_Static_assert(sizeof(long) >= sizeof(fc_status_t));  // NOLINT

// Non-fdomain definitions. These are all zx_status_t analogues (though the
// numbers are different).
#define FC_OK (0)
#define FC_ERR_INVALID_ARGS (-44444)
#define FC_ERR_NOT_SUPPORTED (-55555)
#define FC_ERR_NOT_FOUND (-66666)
#define FC_ERR_BUFFER_TOO_SMALL (-77777)
#define FC_ERR_SHOULD_WAIT (-88888)
#define FC_ERR_INTERNAL (-99999)
#define FC_ERR_INTERRUPTED (-100000)

// FDomain top-level error definitions.
#define FC_ERR_SOCKET_WRITE (-1)
#define FC_ERR_CHANNEL_WRITE (-2)
#define FC_ERR_FDOMAIN (-3)
#define FC_ERR_PROTOCOL (-4)
#define FC_ERR_PROTOCOL_OBJECT_TYPE_INCOMPATIBLE (-5)
#define FC_ERR_PROTOCOL_RIGHTS_INCOMPATIBLE (-6)
#define FC_ERR_PROTOCOL_SIGNALS_INCOMPATIBLE (-7)
#define FC_ERR_PROTOCOL_STREAM_EVENT_INCOMPATIBLE (-8)
#define FC_ERR_TRANSPORT (-9)
#define FC_ERR_CONNECTION_MISMATCH (-10)
#define FC_ERR_STREAMING_ABORTED (-11)

extern void create_ffx_lib_context(ffx_lib_context_t** ctx);

extern fc_status_t create_ffx_env_context(ffx_env_context_t** env_ctx, ffx_lib_context_t* lib_ctx,
                                          ffx_config_t* config, uint64_t config_len,
                                          const char* isolate_dir);

extern void destroy_ffx_env_context(ffx_env_context_t* ctx);

extern void destroy_ffx_lib_context(ffx_lib_context_t* env_ctx);

extern fc_status_t ffx_connect_remote_control_proxy(ffx_env_context_t* ctx, zx_handle_t* out);
extern fc_status_t ffx_connect_device_proxy(ffx_env_context_t* ctx, const char* moniker,
                                            const char* capability_name, zx_handle_t* out);
// Attempts to wait (blocking) for a target to become available. Waits for `timeout_seconds`
// seconds before timing out. Passing a timeout of zero means this function will wait an indefinite
// amount of time.
extern fc_status_t ffx_target_wait(ffx_env_context_t* ctx, uint64_t timeout_seconds, bool offline);

// Attempts to close the channel. In the vast majority of cases this will return
// an FC_OK status. The only other error this can return is FC_ERR_INTERRUPTED
// in the event that the SIGINT signal was received while closing the channel.
extern fc_status_t ffx_close_handle(ffx_lib_context_t* ctx, zx_handle_t handle);

extern fc_status_t ffx_channel_create(ffx_env_context_t* ctx, uint32_t options, zx_handle_t* out0,
                                      zx_handle_t* out1);
extern fc_status_t ffx_channel_write(ffx_lib_context_t* ctx, zx_handle_t handle,
                                     const char* out_buf, uint64_t out_len, zx_handle_t* hdls,
                                     uint64_t hdls_len);
extern fc_status_t ffx_channel_write_etc(ffx_lib_context_t* ctx, zx_handle_t handle,
                                         const char* out_buf, uint64_t out_len,
                                         zx_handle_disposition_t* hdls, uint64_t hdls_len);
extern fc_status_t ffx_handle_get_koid(ffx_lib_context_t* ctx, zx_handle_t handle, zx_koid_t* out);
// Attempts to get a config value from the environment context.
//
// Will try to coerce said value into a string. `out_buf` should point to a caller-owned buffer,
// which this function will write to. `out_buf_len` is a caller-owned pointer to the length of
// `out_buf`. On a ZX_OK return, `out_buf_len` will have the length of the found config string value
// written to it.
//
// If the config value cannot be found, ZX_ERR_NOT_FOUND will be returned. If the `out_buf_len` is
// not sufficient to fit the found value, ZX_ERR_BUFFER_TOO_SMALL will be returned. For any other
// internal errors ZX_ERR_INTERNAL will be returned and the error scratch buffer will be set.
extern fc_status_t ffx_config_get_string(ffx_env_context_t* ctx, const char* config_key,
                                         uint64_t config_key_len, char* out_buf,
                                         uint64_t* out_buf_len);
extern fc_status_t ffx_channel_read(ffx_lib_context_t* ctx, zx_handle_t handle, char* out_buf,
                                    uint64_t out_len, zx_handle_t* hdls, uint64_t hdls_len,
                                    uint64_t* actual_bytes_count, uint64_t* actual_handles_count);

extern fc_status_t ffx_socket_create(ffx_env_context_t* ctx, uint32_t options, zx_handle_t* out0,
                                     zx_handle_t* out1);
extern fc_status_t ffx_socket_read(ffx_lib_context_t* ctx, zx_handle_t handle, char* out_buf,
                                   uint64_t out_len, uint64_t* bytes_read);
extern fc_status_t ffx_socket_write(ffx_lib_context_t* ctx, zx_handle_t handle, const char* buf,
                                    uint64_t buf_len);
extern fc_status_t ffx_event_create(ffx_env_context_t* ctx, uint32_t options, zx_handle_t* out);
extern fc_status_t ffx_eventpair_create(ffx_env_context_t* ctx, uint32_t options, zx_handle_t* out0,
                                        zx_handle_t* out1);
extern fc_status_t ffx_object_signal(ffx_lib_context_t* ctx, zx_handle_t hdl, uint32_t clear_mask,
                                     uint32_t set_mask);
extern fc_status_t ffx_object_signal_peer(ffx_lib_context_t* ctx, zx_handle_t hdl,
                                          uint32_t clear_mask, uint32_t set_mask);
// Attempts to poll the object for any of the following signals masked in "signals." This does not
// have an analogue regarding zircon object syscalls, as this does not accept a timeout or something
// similar. This is intended for higher level asynchronous programs to use. Similar to the other
// "*read" calls in this ABI, this either returns a well-defined error given a handle in bad state,
// or returns ZX_ERR_SHOULD_WAIT in the event that there are no signals available for the handle.
// The user will then need to refer to the handle notifier fd (see ffx_connect_handle_notifier) to
// determine when the object is ready with a signal.
extern fc_status_t ffx_object_signal_poll(ffx_lib_context_t* ctx, zx_handle_t hdl, uint32_t signals,
                                          uint32_t* signals_out);

/// This is a testing function for SIGINT handling.
extern fc_status_t _block_forever(ffx_lib_context_t* ctx);

// Opens a file descriptor that delivers zircon handle numbers that are ready to be read.
// There can only be one file descriptor for the lifetime of a library module, so all calls to this
// function will return the same file descriptor number.
extern int32_t ffx_connect_handle_notifier(ffx_lib_context_t* ctx);
extern void ffx_get_last_error(ffx_lib_context_t* ctx, char** out_buf, uint64_t* out_len,
                               fc_err_type_t* out_type);
extern void ffx_free_error_buffer(char* ptr, uint64_t len);

#ifdef __cplusplus

}  // extern "C"

#endif  // __cplusplus

// LINT.ThenChange(../../src/lib.rs, //src/developer/ffx/lib/fuchsia-controller/src/compat.rs)
#endif  // SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FUCHSIA_CONTROLLER_INTERNAL_FUCHSIA_CONTROLLER_H_
