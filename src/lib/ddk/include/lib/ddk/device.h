// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DDK_INCLUDE_LIB_DDK_DEVICE_H_
#define SRC_LIB_DDK_INCLUDE_LIB_DDK_DEVICE_H_

#include <lib/fdf/types.h>
#include <zircon/compiler.h>
#include <zircon/fidl.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

__BEGIN_CDECLS

typedef struct zx_device zx_device_t;
typedef struct zx_driver zx_driver_t;

typedef struct zx_protocol_device zx_protocol_device_t;

// Max device name length, not including a null-terminator
#define ZX_DEVICE_NAME_MAX 31

// echo -n "zx_device_ops_v0.52" | sha256sum | cut -c1-16
#define DEVICE_OPS_VERSION_0_52 0xb834fdab33623bb4

// Current Version
#define DEVICE_OPS_VERSION DEVICE_OPS_VERSION_0_52

// TODO: temporary flags used by devcoord to communicate
// with the system bus device.
#define DEVICE_SUSPEND_FLAG_REBOOT 0xdcdc0100
#define DEVICE_SUSPEND_FLAG_POWEROFF 0xdcdc0200
#define DEVICE_SUSPEND_FLAG_MEXEC 0xdcdc0300
#define DEVICE_SUSPEND_FLAG_SUSPEND_RAM 0xdcdc0400
#define DEVICE_SUSPEND_REASON_MASK 0xffffff00

// These values should be same as the enum fuchsia.device.DevicePowerState
// generated from FIDL. The system wide power manager will be using the
// power states from FIDL generated file.
#define DEV_POWER_STATE_D0 UINT8_C(0)
#define DEV_POWER_STATE_D1 UINT8_C(1)
#define DEV_POWER_STATE_D2 UINT8_C(2)
#define DEV_POWER_STATE_D3HOT UINT8_C(3)
#define DEV_POWER_STATE_D3COLD UINT8_C(4)

// Performance state
#define DEV_PERFORMANCE_STATE_P0 UINT32_C(0)

// reboot modifiers
#define DEVICE_SUSPEND_FLAG_REBOOT_BOOTLOADER (DEVICE_SUSPEND_FLAG_REBOOT | 0x01)
#define DEVICE_SUSPEND_FLAG_REBOOT_RECOVERY (DEVICE_SUSPEND_FLAG_REBOOT | 0x02)
#define DEVICE_SUSPEND_FLAG_REBOOT_KERNEL_INITIATED (DEVICE_SUSPEND_FLAG_REBOOT | 0x03)

#define DEVICE_SUSPEND_REASON_POWEROFF UINT8_C(0x10)
#define DEVICE_SUSPEND_REASON_SUSPEND_RAM UINT8_C(0x20)
#define DEVICE_SUSPEND_REASON_MEXEC UINT8_C(0x30)
#define DEVICE_SUSPEND_REASON_REBOOT UINT8_C(0x40)
#define DEVICE_SUSPEND_REASON_REBOOT_RECOVERY (UINT8_C(DEVICE_SUSPEND_REASON_REBOOT | 0x01))
#define DEVICE_SUSPEND_REASON_REBOOT_BOOTLOADER (UINT8_C(DEVICE_SUSPEND_REASON_REBOOT | 0x02))
#define DEVICE_SUSPEND_REASON_REBOOT_KERNEL_INITIATED (UINT8_C(DEVICE_SUSPEND_REASON_REBOOT | 0x03))
#define DEVICE_SUSPEND_REASON_SELECTIVE_SUSPEND UINT8_C(0x50)
#define DEVICE_MASK_SUSPEND_REASON UINT8_C(0xf0)

//@doc(docs/ddk/device-ops.md)

typedef struct device_fidl_txn device_fidl_txn_t;
// An outstanding FIDL transaction used when the driver host is managing
// a FIDL channel.
struct device_fidl_txn {
  // Internal value used for driver host bookkeeping.  Must not be mutated.
  uintptr_t driver_host_context;
};

//@ # The Device Protocol
//
// Device drivers implement a set of hooks (methods) to support the
// operations that may be done on the devices that they publish.
//
// These are described below, including the action that is taken
// by the default implementation that is used for each hook if the
// driver does not provide its own implementation.

typedef struct zx_protocol_device {
  //@ ## version
  // This field must be set to `DEVICE_OPS_VERSION`
  uint64_t version;

  //@ ## get_protocol
  // The get_protocol hook is called when a driver invokes
  // **device_get_protocol()** on a device object.  The implementation must
  // populate *protocol* with a protocol structure determined by *proto_id*.
  // If the requested *proto_id* is not supported, the implementation must
  // return ZX_ERR_NOT_SUPPORTED.
  //
  // The default get_protocol hook returns with *protocol*=*proto_ops* if *proto_id*
  // matches the one given when **device_add()** created the device, and returns
  // ZX_ERR_NOT_SUPPORTED otherwise.
  //
  // See the **device_get_protocol()** docs for a description of the layout of
  // *protocol*.
  //
  // This hook is never called by the devhost runtime other than when
  // **device_get_protocol()** is invoked by some driver.  It is executed
  // synchronously in the same thread as the caller.
  zx_status_t (*get_protocol)(void* ctx, uint32_t proto_id, void* protocol);

  //@ ## init
  // The init hook is called when a device is initially added.
  //
  // If implemented, the device is guaranteed to be invisible and not able to be unbound until the
  // driver calls **device_init_reply()** on itself. **device_init_reply()** can be called from
  // any thread - it does not necessarily need to be called before the |init| hook returns.
  //
  // This allows drivers to safely complete initialization without explicit synchronization with
  // the unbind hook, such as adding device metadata or completing blocking operations in a
  // worker thread. Once the initialization is completed, **device_init_reply()** should be
  // called to make the device visible and able to be unbound.
  //
  // The hook is always called from the devhost's main thread.
  void (*init)(void* ctx);

  //@ ## unbind
  // The unbind hook is called to begin removal of a device (due to hot unplug, fatal error, etc).
  //
  // The driver should avoid further method calls to its parent device or any
  // protocols obtained from that device, and expect that any further such calls
  // will return with an error.
  //
  // The driver should adjust its state to encourage its client connections to close
  // (cause IO to error out, etc), and call **device_unbind_reply()** on itself when ready.
  // See the docs for **device_unbind_reply()** for important semantics.
  //
  // The driver must continue to handle all device hooks except for message, open, read, and write
  // until the **release** hook is invoked.
  //
  // Prior to unbind being called, the DDK will suspend processing of all FIDL messages and new
  // connections will be disallowed at this point. A device driver is responsible for ensuring that
  // any pending FIDL transactions are replied to or closed prior to replying to unbind. A device
  // which handles asynchronous FIDL messages *must* implement this hook.
  //
  // **Note:** This hook will not be called for a **device instance**.
  //
  // This is an optional hook (except for drivers that implement message). The default
  // implementation will be a hook that replies immediately with **device_unbind_reply()**.
  //
  // This hook will be called from the devhost's main thread. It will be executed sometime
  // after any of the following events occuring: **device_async_remove()** is invoked on the
  // device, the device's parent has completed its unbind hook via **device_unbind_reply**,
  // or a fuchsia.device.Controller/ScheduleUnbind request is received.
  void (*unbind)(void* ctx);

  //@ ## release
  // The release hook is called after this device has finished unbinding, all open client
  // connections of the device have been closed, and all child devices have been unbound and
  // released.
  //
  // At the point release is invoked, the driver will not receive any further calls
  // and absolutely must not use the underlying **zx_device_t** or any protocols obtained
  // from that device once this method returns.
  //
  // The driver must free all memory and release all resources related to this device
  // before returning.
  //
  // This hook may be called from any thread including the devhost's main
  // thread.
  void (*release)(void* ctx);

  //@ ## suspend
  // The suspend hook is used for suspending a device from a working to
  // non-working low power state(sleep state), or from a non-working sleep state
  // to a deeper sleep state.
  //
  // requested_state is always a non-working sleep state.
  // enable_wake is whether to configure the device for wakeup from the requested non
  // working sleep state. If enable_wake is true and the device does not support
  // wake up, the hook fails without suspending the device.
  // suspend_reason provides information for the driver why the suspend hook is called.
  // Bus drivers and platform drivers like acpi will find this information useful to
  // issue any system calls or save the reboot reason.
  //
  // The driver should put the device into the requested_state and call **device_suspend_reply()**
  // on itself. device_suspend_reply() will take in two parameters: status of the suspend operation
  // and an out_state. If status is success, the out_state is same as requested_state.
  // If status is failure, out_state is the low power state the device is currently in.
  //
  // This hook assumes that the drivers are aware of their current state. This hook will only
  // be executed on the devhost's main thread.
  void (*suspend)(void* ctx, uint8_t requested_state, bool enable_wake, uint8_t suspend_reason);

  //@ ## message
  // Process a FIDL rpc message. This is used to handle class or
  // device specific messaging.
  //
  // The entire message becomes the responsibility of the driver,
  // including the handles.
  //
  // The txn provided to respond to the message is only valid for
  // the duration of the message() call. It must not be retained
  // and used later.
  //
  // If this method wishes to respond asynchronously, the txn
  // should be copied.
  //
  // This hook will only be executed on the devhost's main thread.
  void (*message)(void* ctx, fidl_incoming_msg_t msg, device_fidl_txn_t txn);

  //@ ## child_pre_release
  // The child_pre_release hook is used to signal that a child device
  // will soon be released. This is after the child and all its descendents
  // have been unbound and removed from the device filesystem, and all client
  // connections to the child have been closed.
  //
  // The device may want to drop any references to the child context or child
  // **zx_device_t**.
  //
  // This hook may be called from any thread including the devhost's main
  // thread.
  void (*child_pre_release)(void* ctx, void* child_ctx);

  //@ ## made_visible
  // The made_visible hook is used to signal that the device has been made
  // visible in devfs. It can be used as a synchornization point to inform
  // clients that they may try and open the device.
  //
  // This hook will only be executed on the devhost's main thread.
  void (*made_visible)(void* ctx);

} zx_protocol_device_t;

// protocols look like:
// typedef struct {
//     protocol_xyz_ops_t* ops;
//     void* ctx;
// } protocol_xyz_t;
zx_status_t device_get_protocol(const zx_device_t* dev, uint32_t proto_id, void* protocol);

// Structured configuration VMO
// Returns the configuration VMO. This call can only be made once per device.
zx_status_t device_get_config_vmo(zx_device_t* device, zx_handle_t* config_vmo);

// Direct Device Ops Functions

// Opens a connection to the specified runtime service offered by |device|.
//
// |device| should be a composite device. |fragment_name| picks out the specific
// fragment device to use; it must match the fragment name declared in the
// composite device's bind file.
//
// Arguments are otherwise the same as for device_connect_runtime_protocol.
//
// The ddktl equivalent is DdkConnectFragmentRuntimeProtocol.
//
// Returns |ZX_ERR_UNAVAILABLE| if the parent (or fragment) does not have an outgoing directory.
// Returns |ZX_ERR_NOT_FOUND| if |fragment_name| is not the name of a parent.
// Returns |ZX_ERR_NOT_SUPPORTED| if |fragment_name| specified by the device is not a
// composite node.
// Returns |ZX_ERR_BAD_HANDLE| if |request| is not a valid runtime handle.
zx_status_t device_connect_fragment_runtime_protocol(zx_device_t* device, const char* fragment_name,
                                                     const char* service_name,
                                                     const char* protocol_name,
                                                     fdf_handle_t request);

// Opens a connection to the specified protocol in driver's incoming namespace.
// See |fdio_service_connect_at| for more information.
zx_status_t device_connect_ns_protocol(zx_device_t* device, const char* protocol_name,
                                       zx_handle_t request);
// Device Metadata Support

// retrieves metadata for a specific device
// searches parent devices to find a match
zx_status_t device_get_metadata(zx_device_t* dev, uint32_t type, void* buf, size_t buflen,
                                size_t* actual);

// Returns the specific protocol from the named fragment, identified by the name
// provided when it was created via`device_add_composite`. Returns ZX_ERR_NOT_FOUND if
// no fragment exists.
zx_status_t device_get_fragment_protocol(zx_device_t* dev, const char* name, uint32_t proto_id,
                                         void* protocol);

// retrieves metadata for a specific device
// searches parent devices to find a match
zx_status_t device_get_fragment_metadata(zx_device_t* dev, const char* name, uint32_t type,
                                         void* buf, size_t buflen, size_t* actual);

// Adds a service member protocol to the outgoing directory of |dev|
//
// May be called by the device ONLY AFTER device_add is called.
// |handler| is a function pointer castable to an AnyHandler object.  The handler is called when
// a client connects to the protocol.
// The protocol will be added to /svc/|service_name|/|instance_name|/|member_name|
// This call only supports the zircon channel transport, as it is intended to be used to
// advertise a service to non-drivers.
zx_status_t device_register_service_member(zx_device_t* dev, void* handler,
                                           const char* service_name, const char* instance_name,
                                           const char* member_name);

// Device State Change Functions.  These match up with the signals defined in
// the fuchsia.device.Controller interface.
//
//@ #### Device State Bits
//{
#define DEV_STATE_READABLE ZX_USER_SIGNAL_0
#define DEV_STATE_WRITABLE ZX_USER_SIGNAL_2
#define DEV_STATE_ERROR ZX_USER_SIGNAL_3
#define DEV_STATE_HANGUP ZX_USER_SIGNAL_4
#define DEV_STATE_OOB ZX_USER_SIGNAL_1
//}

__END_CDECLS

#endif  // SRC_LIB_DDK_INCLUDE_LIB_DDK_DEVICE_H_
