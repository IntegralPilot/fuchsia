// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/trace/event.h>
#include <lib/trace/event_args.h>
#include <zircon/syscalls.h>

#include <fbl/string.h>
#include <trace-test-utils/fixture.h>
#include <zxtest/zxtest.h>

#include "fixture_macros.h"
#include "lib.h"

#define TEST_SUITE rust_test

TEST(TEST_SUITE, test_trace_enabled) {
  BEGIN_TRACE_TEST;

  EXPECT_FALSE(rs_test_trace_enabled());

  fixture_initialize_and_start_tracing();

  EXPECT_TRUE(rs_test_trace_enabled());
}

TEST(TEST_SUITE, test_category_enabled) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  EXPECT_FALSE(rs_test_category_disabled_cstr());
  EXPECT_FALSE(rs_test_category_disabled_str());
  EXPECT_TRUE(rs_test_category_enabled_cstr());
  EXPECT_TRUE(rs_test_category_enabled_str());
}

TEST(TEST_SUITE, test_counter_cstr) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_counter_macro_cstr();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"arg\")\n"
      "String(index: 3, \"name\")\n"
      "String(index: 4, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", Counter(id: 42), {arg: "
      "int32(10)})\n");
}

TEST(TEST_SUITE, test_counter_str) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_counter_macro_str();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"arg\")\n"
      "String(index: 3, \"name\")\n"
      "String(index: 4, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", Counter(id: 42), {arg: "
      "int32(10)})\n");
}

TEST(TEST_SUITE, test_counter_str_and_string) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_counter_macro_str_and_string();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"arg\")\n"
      "String(index: 3, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", Counter(id: 42), {arg: "
      "int32(10)})\n");
}

TEST(TEST_SUITE, test_instant_cstr) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_instant_macro_cstr();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"arg\")\n"
      "String(index: 3, \"name\")\n"
      "String(index: 4, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", Instant(scope: process), "
      "{arg: int32(10)})\n");
}

TEST(TEST_SUITE, test_instant_str) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_instant_macro_str();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"arg\")\n"
      "String(index: 3, \"name\")\n"
      "String(index: 4, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", Instant(scope: process), "
      "{arg: int32(10)})\n");
}

TEST(TEST_SUITE, test_instant_str_and_string) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_instant_macro_str_and_string();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"arg\")\n"
      "String(index: 3, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", Instant(scope: process), "
      "{arg: int32(10)})\n");
}

TEST(TEST_SUITE, test_duration_cstr) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_duration_macro_cstr();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"y\")\n"
      "String(index: 4, \"name\")\n"
      "String(index: 5, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationComplete(end_ts: <>), "
      "{x: int32(5), y: int32(10)})\n");
}

TEST(TEST_SUITE, test_duration_str) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_duration_macro_str();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"y\")\n"
      "String(index: 4, \"name\")\n"
      "String(index: 5, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationComplete(end_ts: <>), "
      "{x: int32(5), y: int32(10)})\n");
}

TEST(TEST_SUITE, test_duration_str_and_string) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_duration_macro_str_and_string();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"y\")\n"
      "String(index: 4, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationComplete(end_ts: <>), "
      "{x: int32(5), y: int32(10)})\n");
}

TEST(TEST_SUITE, test_scoped_duration) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_duration_macro_with_scope();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"y\")\n"
      "String(index: 4, \"arg\")\n"
      "String(index: 5, \"name\")\n"
      "String(index: 6, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", Instant(scope: process), "
      "{arg: int32(10)})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationComplete(end_ts: <>), "
      "{x: int32(5), y: int32(10)})\n");
}

TEST(TEST_SUITE, test_duration_granular_cstr) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_duration_begin_end_macros_cstr();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"name\")\n"
      "String(index: 4, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {x: "
      "int32(5)})\n"
      "String(index: 5, \"arg\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", Instant(scope: process), "
      "{arg: int32(10)})\n"
      "String(index: 6, \"y\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, {y: "
      "string(\"foo\")})\n");
}

TEST(TEST_SUITE, test_duration_granular_str) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_duration_begin_end_macros_str();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"name\")\n"
      "String(index: 4, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {x: "
      "int32(5)})\n"
      "String(index: 5, \"arg\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", Instant(scope: process), "
      "{arg: int32(10)})\n"
      "String(index: 6, \"y\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, {y: "
      "string(\"foo\")})\n");
}

TEST(TEST_SUITE, test_duration_granular_str_and_string) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_duration_begin_end_macros_str_and_string();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {x: "
      "int32(5)})\n"
      "String(index: 4, \"arg\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", Instant(scope: process), "
      "{arg: int32(10)})\n"
      "String(index: 5, \"y\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, {y: "
      "string(\"foo\")})\n");
}

TEST(TEST_SUITE, test_vthread_duration_granular) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_vthread_duration_begin_end_macros();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"name\")\n"
      "String(index: 4, \"vthread_name\")\n"
      "String(index: 5, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"vthread_name\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {x: "
      "int32(5)})\n"
      "String(index: 6, \"arg\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 2, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", Instant(scope: process), "
      "{arg: int32(10)})\n"
      "String(index: 7, \"y\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, {y: "
      "string(\"foo\")})\n");
}

TEST(TEST_SUITE, test_blob_cstr) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_blob_macro_cstr();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"name\")\n"
      "String(index: 4, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "LargeRecord(Blob(format: blob_event, category: \"+enabled\", name: \"name\", ts: <>, pt: "
      "<>, {x: int32(5)}, size: 13, preview: <62 6c 6f 62 20 63 6f 6e 74 65 6e 74 73>))\n");
}

TEST(TEST_SUITE, test_blob_str) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_blob_macro_str();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"name\")\n"
      "String(index: 4, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "LargeRecord(Blob(format: blob_event, category: \"+enabled\", name: \"name\", ts: <>, pt: "
      "<>, {x: int32(5)}, size: 13, preview: <62 6c 6f 62 20 63 6f 6e 74 65 6e 74 73>))\n");
}

TEST(TEST_SUITE, test_blob_str_and_string) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_blob_macro_str_and_string();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "LargeRecord(Blob(format: blob_event, category: \"+enabled\", name: \"name\", ts: <>, pt: "
      "<>, {x: int32(5)}, size: 13, preview: <62 6c 6f 62 20 63 6f 6e 74 65 6e 74 73>))\n");
}

TEST(TEST_SUITE, test_flow_cstr) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_flow_begin_step_end_macros_cstr();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"name\")\n"
      "String(index: 4, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowBegin(id: 123), {x: "
      "int32(5)})\n"
      "String(index: 5, \"z\")\n"
      "String(index: 6, \"step\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"step\", FlowStep(id: 123), {z: "
      "int32(42)})\n"
      "String(index: 7, \"y\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowEnd(id: 123), {y: "
      "string(\"foo\")})\n");
}

TEST(TEST_SUITE, test_flow_str) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_flow_begin_step_end_macros_str();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"name\")\n"
      "String(index: 4, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowBegin(id: 123), {x: "
      "int32(5)})\n"
      "String(index: 5, \"z\")\n"
      "String(index: 6, \"step\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"step\", FlowStep(id: 123), {z: "
      "int32(42)})\n"
      "String(index: 7, \"y\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowEnd(id: 123), {y: "
      "string(\"foo\")})\n");
}

TEST(TEST_SUITE, test_flow_str_and_string) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_flow_begin_step_end_macros_str_and_string();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowBegin(id: 123), {x: "
      "int32(5)})\n"
      "String(index: 4, \"z\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"step\", FlowStep(id: 123), {z: "
      "int32(42)})\n"
      "String(index: 5, \"y\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowEnd(id: 123), {y: "
      "string(\"foo\")})\n");
}

TEST(TEST_SUITE, test_arglimit) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_arglimit();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"1\")\n"
      "String(index: 3, \"2\")\n"
      "String(index: 4, \"3\")\n"
      "String(index: 5, \"4\")\n"
      "String(index: 6, \"5\")\n"
      "String(index: 7, \"6\")\n"
      "String(index: 8, \"7\")\n"
      "String(index: 9, \"8\")\n"
      "String(index: 10, \"9\")\n"
      "String(index: 11, \"10\")\n"
      "String(index: 12, \"11\")\n"
      "String(index: 13, \"12\")\n"
      "String(index: 14, \"13\")\n"
      "String(index: 15, \"14\")\n"
      "String(index: 16, \"15\")\n"
      "String(index: 17, \"name\")\n"
      "String(index: 18, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationComplete(end_ts: <>), "
      "{1: int32(1), 2: int32(2), 3: int32(3), 4: int32(4), 5: int32(5), 6: int32(6), 7: int32(7), "
      "8: int32(8), 9: int32(9), 10: int32(10), 11: int32(11), 12: int32(12), 13: int32(13), 14: "
      "int32(14), 15: int32(15)})\n");
}

TEST(TEST_SUITE, test_async_event) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_async_event_with_scope();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"y\")\n"
      "String(index: 4, \"name\")\n"
      "String(index: 5, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", AsyncBegin(id: 1), {x: "
      "int32(5), y: int32(10)})\n"
      "String(index: 6, \"arg\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", Instant(scope: process)"
      ", {arg: int32(10)})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", AsyncEnd(id: 1), {})\n");
}

TEST(TEST_SUITE, test_custom_track_and_async) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_custom_track_and_async_events();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"x\")\n"
      "String(index: 3, \"async_instant_name\")\n"
      "String(index: 4, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"async_instant_name\", AsyncInstant(id: 42), {x: int32(1)})\n"
      "String(index: 5, \"y\")\n"
      "String(index: 6, \"async_duration_name\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"async_duration_name\", AsyncBegin(id: 42), {y: int32(2)})\n"
      "String(index: 7, \"arg\")\n"
      "String(index: 8, \"instant_in_between\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"instant_in_between\", Instant(scope: process), {arg: int32(10)})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"async_duration_name\", AsyncEnd(id: 42), {})\n"
      "String(index: 9, \"a\")\n"
      "String(index: 10, \"track_instant_name\")\n"
      "String(index: 11, \"my_custom_track\")\n"
      "KernelObject(koid: <>, type: thread, name: \"my_custom_track\", {process: koid(<>)})\n"
      "Thread(index: 2, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"track_instant_name\", Instant(scope: thread), {a: int32(10)})\n"
      "String(index: 12, \"b\")\n"
      "String(index: 13, \"track_manual_duration\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"track_manual_duration\", DurationBegin, {b: int32(20)})\n"
      "String(index: 14, \"instant_in_between_track\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"instant_in_between_track\", Instant(scope: process), {arg: int32(20)})\n"
      "String(index: 15, \"c\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"track_manual_duration\", DurationEnd, {c: int32(30)})\n"
      "String(index: 16, \"d\")\n"
      "String(index: 17, \"track_scoped_duration\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"track_scoped_duration\", DurationComplete(end_ts: <>), {d: int32(40)})\n");
}

TEST(TEST_SUITE, test_custom_track_concurrency) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_custom_track_concurrent_usage();
}

TEST(TEST_SUITE, test_alert_cstr) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_alert_str();

  ASSERT_TRUE(fixture_wait_alert_notification());
  ASSERT_TRUE(fixture_compare_last_alert_name("alert_name"));
}

TEST(TEST_SUITE, test_alert_str) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_alert_str();

  ASSERT_TRUE(fixture_wait_alert_notification());
  ASSERT_TRUE(fixture_compare_last_alert_name("alert_name"));
}

TEST(TEST_SUITE, test_alert_str_and_string) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_alert_str_and_string();

  ASSERT_TRUE(fixture_wait_alert_notification());
  ASSERT_TRUE(fixture_compare_last_alert_name("alert_name"));
}

TEST(TEST_SUITE, test_trace_future_enabled_cstr) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_trace_future_enabled_cstr();

  ASSERT_RECORDS(
      // Future immediately ready.
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"name\")\n"
      "String(index: 3, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "String(index: 4, \"ready\")\n"
      "String(index: 5, \"poll-state\")\n"
      "String(index: 6, \"poll-count\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"ready\"), poll-count: uint64(1)})\n"

      // Future that returns pending then ready.
      // Poll 1.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowBegin(id: 4), {})\n"
      "String(index: 7, \"pending\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"pending\"), poll-count: uint64(1)})\n"
      // Poll 2.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowEnd(id: 4), {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"ready\"), poll-count: uint64(2)})\n"

      // Future that returns pending twice then ready.
      // Poll 1.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowBegin(id: 5), {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"pending\"), poll-count: uint64(1)})\n"
      // Poll 2.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowStep(id: 5), {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"pending\"), poll-count: uint64(2)})\n"
      // Poll 3.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowEnd(id: 5), {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"ready\"), poll-count: uint64(3)})\n");
}

TEST(TEST_SUITE, test_trace_future_enabled_str) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_trace_future_enabled_str();

  ASSERT_RECORDS(
      // Future immediately ready.
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"name\")\n"
      "String(index: 3, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "String(index: 4, \"ready\")\n"
      "String(index: 5, \"poll-state\")\n"
      "String(index: 6, \"poll-count\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"ready\"), poll-count: uint64(1)})\n"

      // Future that returns pending then ready.
      // Poll 1.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowBegin(id: 4), {})\n"
      "String(index: 7, \"pending\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"pending\"), poll-count: uint64(1)})\n"
      // Poll 2.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowEnd(id: 4), {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"ready\"), poll-count: uint64(2)})\n"

      // Future that returns pending twice then ready.
      // Poll 1.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowBegin(id: 5), {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"pending\"), poll-count: uint64(1)})\n"
      // Poll 2.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowStep(id: 5), {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"pending\"), poll-count: uint64(2)})\n"
      // Poll 3.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowEnd(id: 5), {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"ready\"), poll-count: uint64(3)})\n");
}

TEST(TEST_SUITE, test_trace_future_enabled_str_and_string) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_trace_future_enabled_str_and_string();

  ASSERT_RECORDS(
      // Future immediately ready.
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "String(index: 3, \"ready\")\n"
      "String(index: 4, \"poll-state\")\n"
      "String(index: 5, \"poll-count\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"ready\"), poll-count: uint64(1)})\n"

      // Future that returns pending then ready.
      // Poll 1.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowBegin(id: 4), {})\n"
      "String(index: 6, \"pending\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"pending\"), poll-count: uint64(1)})\n"
      // Poll 2.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowEnd(id: 4), {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"ready\"), poll-count: uint64(2)})\n"

      // Future that returns pending twice then ready.
      // Poll 1.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowBegin(id: 5), {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"pending\"), poll-count: uint64(1)})\n"
      // Poll 2.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowStep(id: 5), {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"pending\"), poll-count: uint64(2)})\n"
      // Poll 3.
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowEnd(id: 5), {})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"ready\"), poll-count: uint64(3)})\n");
}

TEST(TEST_SUITE, test_trace_future_enabled_with_arg) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_trace_future_enabled_with_arg();

  ASSERT_RECORDS(
      "String(index: 1, \"+enabled\")\n"
      "String(index: 2, \"arg\")\n"
      "String(index: 3, \"name\")\n"
      "String(index: 4, \"process\")\n"
      "KernelObject(koid: <>, type: thread, name: \"initial-thread\", {process: koid(<>)})\n"
      "Thread(index: 1, <>)\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, "
      "{arg: int32(10)})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowBegin(id: 3), {})\n"
      "String(index: 5, \"pending\")\n"
      "String(index: 6, \"poll-state\")\n"
      "String(index: 7, \"poll-count\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"pending\"), poll-count: uint64(1)})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationBegin, "
      "{arg: int32(10)})\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", FlowEnd(id: 3), {})\n"
      "String(index: 8, \"ready\")\n"
      "Event(ts: <>, pt: <>, category: \"+enabled\", name: \"name\", DurationEnd, "
      "{poll-state: string(\"ready\"), poll-count: uint64(2)})\n");
}

TEST(TEST_SUITE, test_trace_future_disabled) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_trace_future_disabled();

  ASSERT_RECORDS("");
}

TEST(TEST_SUITE, test_trace_future_disabled_with_arg) {
  BEGIN_TRACE_TEST;

  fixture_initialize_and_start_tracing();

  rs_test_trace_future_disabled_with_arg();

  ASSERT_RECORDS("");
}

TEST(TEST_SUITE, test_trace_observer) {
  BEGIN_TRACE_TEST;

  EXPECT_EQ(0, rs_check_trace_state());
  rs_setup_trace_observer();
  EXPECT_EQ(0, rs_check_trace_state());
  fixture_initialize_and_start_tracing();
  rs_wait_trace_state_is(TRACE_STARTED);
  EXPECT_EQ(1, rs_check_trace_state());
  ASSERT_RECORDS("");
  rs_wait_trace_state_is(TRACE_STOPPED);
  EXPECT_EQ(0, rs_check_trace_state());
}
