// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_MEMORY_ATTRIBUTION_MONITOR_TESTS_TRACES_LIB_H_
#define SRC_PERFORMANCE_MEMORY_ATTRIBUTION_MONITOR_TESTS_TRACES_LIB_H_

extern "C" {

void rs_init_logs(void);
void rs_test_trace_two_records(void);
void rs_test_trace_no_record(void);
}

#endif  // SRC_PERFORMANCE_MEMORY_ATTRIBUTION_MONITOR_TESTS_TRACES_LIB_H_
