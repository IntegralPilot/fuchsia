// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DDK_INCLUDE_LIB_DDK_DRIVER_H_
#define SRC_LIB_DDK_INCLUDE_LIB_DDK_DRIVER_H_

#include <lib/async/dispatcher.h>
#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

typedef struct zx_device zx_device_t;
typedef struct zx_driver zx_driver_t;
typedef struct zx_protocol_device zx_protocol_device_t;
typedef struct zx_device_str_prop zx_device_str_prop_t;
typedef struct zx_device_str_prop_val zx_device_str_prop_val_t;
typedef struct zx_driver_rec zx_driver_rec_t;

typedef struct zx_driver_binding zx_driver_binding_t;

// echo -n "zx_driver_ops_v0.5" | sha256sum | cut -c1-16
#define DRIVER_OPS_VERSION 0x2b3490fa40d9f452

typedef struct zx_driver_ops {
  uint64_t version;  // DRIVER_OPS_VERSION

  // Opportunity to do on-load work. Called ony once, before any other ops are called. The driver
  // may optionally return a context pointer to be passed to the other driver ops.
  //
  // This hook will only be executed on the devhost's main thread.
  zx_status_t (*init)(void** out_ctx);

  // Requests that the driver bind to the provided device, initialize it, and publish any
  // children.
  //
  // This hook will only be executed on the devhost's main thread.
  zx_status_t (*bind)(void* ctx, zx_device_t* device);

  // Only provided by bus manager drivers, create() is invoked to instantiate a bus device
  // instance in a new device host process
  //
  // This hook will only be executed on the devhost's main thread.
  zx_status_t (*create)(void* ctx, zx_device_t* parent, const char* name, zx_handle_t rpc_channel);

  // Last call before driver is unloaded.
  //
  // This hook will only be executed on the devhost's main thread.
  void (*release)(void* ctx);

  // Allows the driver to run its hardware unit tests. If tests are enabled for the driver, and
  // run_unit_tests() is implemented, then it will be called after init(). If run_unit_tests()
  // returns true, indicating that the tests passed, then driver operation continues as normal
  // and the driver should be prepared to accept calls to bind(). The tests may write output to
  // |channel| in the form of fuchsia.driver.test.Logger messages. The driver-unit-test library
  // may be used to assist with the implementation of the tests, including output via |channel|.
  //
  // This hook will only be executed on the devhost's main thread.
  bool (*run_unit_tests)(void* ctx, zx_device_t* parent, zx_handle_t channel);
} zx_driver_ops_t;

// echo -n "device_add_args_v0.5" | sha256sum | cut -c1-16
#define DEVICE_ADD_ARGS_VERSION 0x96a64134d56e88e3

enum {
  // Do not attempt to bind drivers to this device automatically
  DEVICE_ADD_NON_BINDABLE = (1 << 0),

  // Children of this device will be loaded in their own devhost process,
  // behind a proxy of this device
  DEVICE_ADD_MUST_ISOLATE = (1 << 1),

  // This device is allowed to be bindable in multiple composite devices
  DEVICE_ADD_ALLOW_MULTI_COMPOSITE = (1 << 2),
};

// Device Manager API

// One of DEV_POWER_STATE_*
typedef uint8_t device_power_state_t;

typedef struct device_power_state_info {
  device_power_state_t state_id;
  // Restore time for coming out of this state to working D0 state.
  zx_duration_t restore_latency;
  // Is this device wakeup_capable?
  bool wakeup_capable;
  // Deepest system system sleep state that the device can wake the system from.
  int32_t system_wake_state;
} device_power_state_info_t;

// One of DEV_PERFORMANCE_STATE_*
typedef uint32_t device_performance_state_t;

typedef struct device_performance_state_info {
  device_performance_state_t state_id;
  // Restore time for coming out of this state to fully performant state.
  zx_duration_t restore_latency;
  // TODO(ravoorir): Figure out how best can a device have metadata that is
  // specific to a performant state of a specific device. For ex: The power
  // manager wants to know what a cpu device's operating point is for a
  // particular performant state.
} device_performance_state_info_t;

//
typedef struct device_metadata {
  uint32_t type;
  const void* data;
  size_t length;
} device_metadata_t;

typedef struct device_add_args {
  // DEVICE_ADD_ARGS_VERSION
  uint64_t version;

  // Driver name is copied to internal structure
  // max length is ZX_DEVICE_NAME_MAX
  const char* name;

  // Context pointer for use by the driver
  // and passed to driver in all zx_protocol_device_t callbacks
  void* ctx;

  // Pointer to device's device protocol operations
  const zx_protocol_device_t* ops;

  // Optional list of device string properties.
  const zx_device_str_prop_t* str_props;

  // Number of device string properties
  uint32_t str_prop_count;

  // Metadata to pass to new device
  const device_metadata_t* metadata_list;

  // Number of metadata blobs in the list
  size_t metadata_count;

  // List of power_states that the device supports.
  // List cannot be more than MAX_DEVICE_POWER_STATES size.
  const device_power_state_info_t* power_states;

  // Number of power states in the list
  uint8_t power_state_count;

  // Optional custom protocol for this device
  uint32_t proto_id;

  // Optional custom protocol operations for this device
  const void* proto_ops;

  // Optional list of fidl services to offer to child driver.
  // These service will automatically be added as bind properties which may be used in
  // bind rules.
  // Only the service instance named "default" will be used.
  // If provided, the `DEVICE_ADD_MUST_ISOLATE` flag must also be specified, and a proxy will not be
  // spawned.
  const char** fidl_service_offers;

  // The number of elements in the above list.
  size_t fidl_service_offer_count;

  // Optional list of runtime services to offer to child driver.
  // Only the service instance named "default" will be used.
  // If provided, the `DEVICE_ADD_MUST_ISOLATE` flag must not be specified.
  const char** runtime_service_offers;

  // The number of elements in the above list.
  size_t runtime_service_offer_count;

  // Zero or more of DEVICE_ADD_*
  uint32_t flags;

  // Optional VMO representing that will get used in devfs inspect tree.
  zx_handle_t inspect_vmo;

  // Optional client channel end for a fuchsia.io.Directory hosting fidl services specified in
  // either |fidl_service_offers| or |runtime_service_offers|.
  zx_handle_t outgoing_dir_channel;

  // Pointer to a fuchsia_driver_framework::BusInfo struct.
  // The exactly type is not encoded to keep this file C friendly.
  const void* bus_info;
} device_add_args_t;

typedef struct device_init_reply_args {
  // List of power_states that the device supports.
  // List cannot be more than MAX_DEVICE_POWER_STATES size.
  const device_power_state_info_t* power_states;

  // Number of power states in the list
  uint8_t power_state_count;
} device_init_reply_args_t;

typedef struct {
  // Optional list of device string properties.
  // Memory owned by the caller.
  zx_device_str_prop_t* str_props;

  // Size of the device string properties array.
  // To be filled by the caller.
  const uint32_t str_prop_count;

  // Count of string properties filled with valid data.
  // Will be filled by the callee.
  uint32_t actual_str_prop_count;

} device_props_args_t;

struct zx_driver_rec {
  const zx_driver_ops_t* ops;
  zx_driver_t* driver;
};

// This global symbol is initialized by the driver loader in devhost
extern zx_driver_rec_t __zircon_driver_rec__;

zx_status_t device_get_properties(zx_device_t* parent, device_props_args_t* out_args);

zx_status_t device_add_from_driver(zx_driver_t* drv, zx_device_t* parent, device_add_args_t* args,
                                   zx_device_t** out);

// Creates a device and adds it to the devmgr.
// device_add_args_t contains all "in" arguments.
// All device_add_args_t values are copied, so device_add_args_t can be stack allocated.
// The device_add_args_t.name string value is copied.
// All other pointer fields are copied as pointers.
// The newly added device will be active before this call returns, so be sure to have
// the "out" pointer point to your device-local structure so callbacks can access
// it immediately.
//
// If this call is successful, but the device needs to be torn down, device_async_remove() should
// be called.  If |args->ctx| is backed by memory, it is the programmer's responsibility to not
// free that memory until the device's |release| hook is called.
static inline zx_status_t device_add(zx_device_t* parent, device_add_args_t* args,
                                     zx_device_t** out) {
  return device_add_from_driver(__zircon_driver_rec__.driver, parent, args, out);
}

// This is used to signal completion of the device's |init| hook.
// This will make the device visible and able to be unbound.
// This can be called from any thread - it does not necessarily need to be called before
// the |init| hook returns.
// If |status| is ZX_OK, the driver may provide optional power state information via |args|.
// If |status| is not ZX_OK, the device will be scheduled to be removed.
void device_init_reply(zx_device_t* device, zx_status_t status,
                       const device_init_reply_args_t* args);

// Schedules the removal of the given device and all its descendents. When a device is
// being removed, its |unbind| hook will be invoked.
// It is safe to call this as long as the device has not completed its |release| hook.
// Multiple requests to remove the same device will have no effect.
void device_async_remove(zx_device_t* device);
// This is used to signal completion of the device's |unbind| hook.
// This does not necessarily need to be called from within the |unbind| hook.
void device_unbind_reply(zx_device_t* device);

// This is used to signal completion of the device's |suspend| hook.
// Need not necessarily need to be called from within the |suspend| hook.
// |status| is the status of the suspend.
// If |status| is success, the |out_state| is same as the requested_state that is
// sent to the suspend hook. If |status| is failure, the |out_state| is the
// state that the device can go into.
void device_suspend_reply(zx_device_t* device, zx_status_t status, uint8_t out_state);

// Requests that the given thread be assigned a profile with parameters appropriate for the given
// role. The available roles and the specific parameters assigned are device-dependent and may also
// vary across builds. Requests are not guaranteed to be honored for all roles and requestors, and
// may either be rejected or silently ignored, based on system-specific criteria.
//
// |role| is name of the requested role. This must not be nullptr, even if |role_size| is zero.
//
// |thread| is a borrowed handle that must have DUPLICATE, TRANSFER, and MANAGE_THREAD rights. It is
// duplicated internally before transfer to the ProfileProvider service to complete the profile
// assignment.
//
// This API should be used in preference to hard coding parameters in drivers.
zx_status_t device_set_profile_by_role(zx_device_t* device, zx_handle_t thread, const char* role,
                                       size_t role_size);

typedef enum {
  ZX_DEVICE_PROPERTY_VALUE_UNDEFINED = 0,
  ZX_DEVICE_PROPERTY_VALUE_INT = 1,
  ZX_DEVICE_PROPERTY_VALUE_STRING = 2,
  ZX_DEVICE_PROPERTY_VALUE_BOOL = 3,
  ZX_DEVICE_PROPERTY_VALUE_ENUM = 4
} device_bind_prop_value_type;

// The value type in zx_device_str_prop_val must match what's
// in the union. To ensure that it is set properly, the struct
// should only be constructed with the supporting macros.
typedef struct zx_device_str_prop_val {
  uint8_t data_type;
  union {
    uint32_t int_val;
    const char* str_val;
    bool bool_val;
    const char* enum_val;
  } data;
} zx_device_str_prop_val_t;

// Supporting macros to construct zx_device_str_prop_val_t.
#define str_prop_int_val(val)                                              \
  zx_device_str_prop_val {                                                 \
    .data_type = ZX_DEVICE_PROPERTY_VALUE_INT, .data = {.int_val = (val) } \
  }
#define str_prop_str_val(val)                                                 \
  zx_device_str_prop_val {                                                    \
    .data_type = ZX_DEVICE_PROPERTY_VALUE_STRING, .data = {.str_val = (val) } \
  }
#define str_prop_bool_val(val)                                               \
  zx_device_str_prop_val {                                                   \
    .data_type = ZX_DEVICE_PROPERTY_VALUE_BOOL, .data = {.bool_val = (val) } \
  }
#define str_prop_enum_val(val)                                               \
  zx_device_str_prop_val {                                                   \
    .data_type = ZX_DEVICE_PROPERTY_VALUE_ENUM, .data = {.enum_val = (val) } \
  }

typedef struct zx_device_str_prop {
  const char* key;
  zx_device_str_prop_val_t property_value;
} zx_device_str_prop_t;

zx_handle_t get_mmio_resource(zx_device_t* dev);

zx_handle_t get_msi_resource(zx_device_t* dev);

zx_handle_t get_power_resource(zx_device_t* dev);

zx_handle_t get_ioport_resource(zx_device_t* dev);

zx_handle_t get_iommu_resource(zx_device_t* dev);

zx_handle_t get_irq_resource(zx_device_t* dev);

zx_handle_t get_smc_resource(zx_device_t* dev);

zx_handle_t get_info_resource(zx_device_t* dev);

zx_status_t load_firmware_from_driver(zx_driver_t* drv, zx_device_t* device, const char* path,
                                      zx_handle_t* fw, size_t* size);

// Drivers may need to load firmware for a device, typically during the call to
// bind the device. The devmgr will look for the firmware at the given path
// relative to system-defined locations for device firmware. The load will be done synchronously,
// blocking the current thread until the firmware is loaded. Care should be taken when using this
// call, as it may cause deadlocks if storage is backed by a driver in the same driver host.
static inline zx_status_t load_firmware(zx_device_t* device, const char* path, zx_handle_t* fw,
                                        size_t* size) {
  return load_firmware_from_driver(__zircon_driver_rec__.driver, device, path, fw, size);
}

// Opens a connection to the specified FIDL protocol offered by |device|.
//
// |device| should be a composite device. |fragment_name| picks out the specific
// fragment device to use; it must match the fragment name declared in the
// composite device's bind file.
//
// Arguments are otherwise the same as for device_open_fidl_service. The
// ddktl equivalent is DdkConnectFidlProtocol.
zx_status_t device_connect_fragment_fidl_protocol(zx_device_t* device, const char* fragment_name,
                                                  const char* service_name,
                                                  const char* protocol_name, zx_handle_t request);

typedef enum {
  DEVICE_BIND_PROPERTY_KEY_UNDEFINED = 0,
  DEVICE_BIND_PROPERTY_KEY_INT = 1,
  DEVICE_BIND_PROPERTY_KEY_STRING = 2,
} device_bind_prop_key_type;

// The value type in device_bind_prop_key_t must match what's in the union. To ensure that it is
// set properly, the struct should only be constructed with the supporting macros.
typedef struct device_bind_prop_key {
  uint8_t key_type;
  union {
    uint32_t int_key;
    const char* str_key;
  } data;
} device_bind_prop_key_t;

// Supporting macros to construct device_bind_prop_key_t.
#define device_bind_prop_int_key(val)                                     \
  device_bind_prop_key {                                                  \
    .key_type = DEVICE_BIND_PROPERTY_KEY_INT, .data = {.int_key = (val) } \
  }

#define device_bind_prop_str_key(val)                                        \
  device_bind_prop_key {                                                     \
    .key_type = DEVICE_BIND_PROPERTY_KEY_STRING, .data = {.str_key = (val) } \
  }

typedef struct device_bind_prop_value {
  uint8_t data_type;
  union {
    uint32_t int_value;
    const char* str_value;
    bool bool_value;
    const char* enum_value;
  } data;
} device_bind_prop_value_t;

// Supporting macros to construct device_bind_prop_value_t.
#define device_bind_prop_int_val(val)                                        \
  device_bind_prop_value {                                                   \
    .data_type = ZX_DEVICE_PROPERTY_VALUE_INT, .data = {.int_value = (val) } \
  }
#define device_bind_prop_str_val(val)                                           \
  device_bind_prop_value {                                                      \
    .data_type = ZX_DEVICE_PROPERTY_VALUE_STRING, .data = {.str_value = (val) } \
  }
#define device_bind_prop_bool_val(val)                                         \
  device_bind_prop_value {                                                     \
    .data_type = ZX_DEVICE_PROPERTY_VALUE_BOOL, .data = {.bool_value = (val) } \
  }
#define device_bind_prop_enum_val(val)                                         \
  device_bind_prop_value {                                                     \
    .data_type = ZX_DEVICE_PROPERTY_VALUE_ENUM, .data = {.enum_value = (val) } \
  }

typedef struct device_bind_prop {
  device_bind_prop_key_t key;
  device_bind_prop_value_t value;
} device_bind_prop_t;

// Represents the condition for evaluating the property values in a node group.
// The values are accepted or rejected based on the condition.
typedef enum {
  DEVICE_BIND_RULE_CONDITION_UNDEFINED = 0,
  DEVICE_BIND_RULE_CONDITION_ACCEPT = 1,
  DEVICE_BIND_RULE_CONDITION_REJECT = 2,
} device_bind_rule_condition;

// Represents the a property in a node group node.
typedef struct bind_rule {
  device_bind_prop_key_t key;
  device_bind_rule_condition condition;

  const device_bind_prop_value_t* values;
  size_t values_count;
} bind_rule_t;

typedef struct parent_spec {
  const bind_rule_t* bind_rules;
  size_t bind_rule_count;

  const device_bind_prop_t* properties;
  size_t property_count;

} parent_spec_t;

typedef struct composite_node_spec {
  const parent_spec_t* parents;
  size_t parent_count;
} composite_node_spec_t;

zx_status_t device_add_composite_spec(zx_device_t* dev, const char* name,
                                      const composite_node_spec_t* spec);

// Protocol Identifiers
#define DDK_PROTOCOL_DEF(tag, val, name, flags) ZX_PROTOCOL_##tag = val,
enum {
#include <lib/ddk/protodefs.h>
};

__END_CDECLS

#endif  // SRC_LIB_DDK_INCLUDE_LIB_DDK_DRIVER_H_
