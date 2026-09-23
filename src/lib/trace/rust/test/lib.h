// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_TRACE_RUST_TEST_LIB_H_
#define SRC_LIB_TRACE_RUST_TEST_LIB_H_

#include <cstdint>
extern "C" {
bool rs_test_trace_enabled(void);

bool rs_test_category_disabled_cstr(void);
bool rs_test_category_disabled_str(void);

bool rs_test_category_enabled_cstr(void);
bool rs_test_category_enabled_str(void);

void rs_test_counter_macro_cstr(void);
void rs_test_counter_macro_str(void);
void rs_test_counter_macro_str_and_string(void);

void rs_test_instant_macro_cstr(void);
void rs_test_instant_macro_str(void);
void rs_test_instant_macro_str_and_string(void);

void rs_test_duration_macro_cstr(void);
void rs_test_duration_macro_str(void);
void rs_test_duration_macro_str_and_string(void);

void rs_test_duration_macro_with_scope(void);

void rs_test_duration_begin_end_macros_cstr(void);
void rs_test_duration_begin_end_macros_str(void);
void rs_test_duration_begin_end_macros_str_and_string(void);

void rs_test_vthread_duration_begin_end_macros(void);
void rs_test_custom_track_and_async_events(void);
void rs_test_custom_track_concurrent_usage(void);

void rs_test_blob_macro_cstr(void);
void rs_test_blob_macro_str(void);
void rs_test_blob_macro_str_and_string(void);

void rs_test_flow_begin_step_end_macros_cstr(void);
void rs_test_flow_begin_step_end_macros_str(void);
void rs_test_flow_begin_step_end_macros_str_and_string(void);

void rs_test_arglimit(void);

void rs_test_async_event_with_scope(void);

void rs_test_alert_cstr();
void rs_test_alert_str();
void rs_test_alert_str_and_string();

void rs_test_trace_future_enabled_cstr();
void rs_test_trace_future_enabled_str();
void rs_test_trace_future_enabled_str_and_string();

void rs_test_trace_future_enabled_with_arg();
void rs_test_trace_future_disabled();
void rs_test_trace_future_disabled_with_arg();
uint8_t rs_check_trace_state();
void rs_wait_trace_state_is(uint32_t expected);
void rs_setup_trace_observer();
}

#endif  // SRC_LIB_TRACE_RUST_TEST_LIB_H_
