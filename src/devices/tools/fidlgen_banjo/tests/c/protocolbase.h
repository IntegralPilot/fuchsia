// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// WARNING: THIS FILE IS MACHINE GENERATED. DO NOT EDIT.
// Generated from the banjo.examples.protocolbase banjo file

#pragma once


#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// Forward declarations
typedef void (*async_base_status_callback)(void* ctx, zx_status_t status, zx_status_t status_2);
typedef void (*async_base_time_callback)(void* ctx, zx_time_t time, zx_time_t time_2);
typedef void (*async_base_duration_callback)(void* ctx, zx_duration_t duration, zx_duration_t duration_2);
typedef void (*async_base_koid_callback)(void* ctx, zx_koid_t koid, zx_koid_t koid_2);
typedef void (*async_base_off_callback)(void* ctx, zx_off_t off, zx_off_t off_2);
typedef struct async_base_protocol async_base_protocol_t;
typedef struct async_base_protocol_ops async_base_protocol_ops_t;
typedef struct driver_transport_protocol driver_transport_protocol_t;
typedef struct driver_transport_protocol_ops driver_transport_protocol_ops_t;
typedef struct synchronous_base_protocol synchronous_base_protocol_t;
typedef struct synchronous_base_protocol_ops synchronous_base_protocol_ops_t;


// Declarations
struct async_base_protocol_ops {
    void (*status)(void* ctx, zx_status_t status, async_base_status_callback callback, void* cookie);
    void (*time)(void* ctx, zx_time_t time, async_base_time_callback callback, void* cookie);
    void (*duration)(void* ctx, zx_duration_t duration, async_base_duration_callback callback, void* cookie);
    void (*koid)(void* ctx, zx_koid_t koid, async_base_koid_callback callback, void* cookie);
    void (*off)(void* ctx, zx_off_t off, async_base_off_callback callback, void* cookie);
};


struct async_base_protocol {
    const async_base_protocol_ops_t* ops;
    void* ctx;
};

struct driver_transport_protocol_ops {
    zx_status_t (*status)(void* ctx, zx_status_t status);
};


struct driver_transport_protocol {
    const driver_transport_protocol_ops_t* ops;
    void* ctx;
};

struct synchronous_base_protocol_ops {
    zx_status_t (*status)(void* ctx, zx_status_t status, zx_status_t* out_status_2);
    zx_time_t (*time)(void* ctx, zx_time_t time, zx_time_t* out_time_2);
    zx_duration_t (*duration)(void* ctx, zx_duration_t duration, zx_duration_t* out_duration_2);
    zx_koid_t (*koid)(void* ctx, zx_koid_t koid, zx_koid_t* out_koid_2);
    zx_off_t (*off)(void* ctx, zx_off_t off, zx_off_t* out_off_2);
};


struct synchronous_base_protocol {
    const synchronous_base_protocol_ops_t* ops;
    void* ctx;
};



// Helpers
static inline void async_base_status(const async_base_protocol_t* proto, zx_status_t status, async_base_status_callback callback, void* cookie) {
    proto->ops->status(proto->ctx, status, callback, cookie);
}

static inline void async_base_time(const async_base_protocol_t* proto, zx_time_t time, async_base_time_callback callback, void* cookie) {
    proto->ops->time(proto->ctx, time, callback, cookie);
}

static inline void async_base_duration(const async_base_protocol_t* proto, zx_duration_t duration, async_base_duration_callback callback, void* cookie) {
    proto->ops->duration(proto->ctx, duration, callback, cookie);
}

static inline void async_base_koid(const async_base_protocol_t* proto, zx_koid_t koid, async_base_koid_callback callback, void* cookie) {
    proto->ops->koid(proto->ctx, koid, callback, cookie);
}

static inline void async_base_off(const async_base_protocol_t* proto, zx_off_t off, async_base_off_callback callback, void* cookie) {
    proto->ops->off(proto->ctx, off, callback, cookie);
}

static inline zx_status_t driver_transport_status(const driver_transport_protocol_t* proto, zx_status_t status) {
    return proto->ops->status(proto->ctx, status);
}

static inline zx_status_t synchronous_base_status(const synchronous_base_protocol_t* proto, zx_status_t status, zx_status_t* out_status_2) {
    return proto->ops->status(proto->ctx, status, out_status_2);
}

static inline zx_time_t synchronous_base_time(const synchronous_base_protocol_t* proto, zx_time_t time, zx_time_t* out_time_2) {
    return proto->ops->time(proto->ctx, time, out_time_2);
}

static inline zx_duration_t synchronous_base_duration(const synchronous_base_protocol_t* proto, zx_duration_t duration, zx_duration_t* out_duration_2) {
    return proto->ops->duration(proto->ctx, duration, out_duration_2);
}

static inline zx_koid_t synchronous_base_koid(const synchronous_base_protocol_t* proto, zx_koid_t koid, zx_koid_t* out_koid_2) {
    return proto->ops->koid(proto->ctx, koid, out_koid_2);
}

static inline zx_off_t synchronous_base_off(const synchronous_base_protocol_t* proto, zx_off_t off, zx_off_t* out_off_2) {
    return proto->ops->off(proto->ctx, off, out_off_2);
}



__END_CDECLS
