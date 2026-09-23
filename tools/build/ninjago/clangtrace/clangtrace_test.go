// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package clangtrace

import (
	"path/filepath"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"
	"go.fuchsia.dev/fuchsia/tools/build/ninjago/chrometrace"
	"go.fuchsia.dev/fuchsia/tools/lib/jsonutil"
)

func TestToInterleave(t *testing.T) {
	for _, tc := range []struct {
		desc        string
		traces      []chrometrace.Trace
		granularity time.Duration
		clangTraces map[string]clangTrace
		want        []chrometrace.Trace
		wantErr     bool
	}{
		{
			desc: "empty",
		},
		{
			desc: "successfully interleave",
			traces: []chrometrace.Trace{
				{TimestampMicros: 42, DurationMicros: 1000, Name: "output.cc.o", ProcessID: 123, ThreadID: 321},
			},
			granularity: 100 * time.Microsecond,
			clangTraces: map[string]clangTrace{
				"output.cc.json": {
					TraceEvents: []chrometrace.Trace{
						{TimestampMicros: 0, DurationMicros: 1000, Name: "ExecuteCompiler", EventType: chrometrace.CompleteEvent, ProcessID: 789, ThreadID: 1},
						{TimestampMicros: 0, DurationMicros: 100, Name: "Source", EventType: chrometrace.CompleteEvent, ProcessID: 789, ThreadID: 2},
						{TimestampMicros: 100, DurationMicros: 120, Name: "Source", EventType: chrometrace.CompleteEvent, ProcessID: 789, ThreadID: 2},
						{TimestampMicros: 300, DurationMicros: 420, Name: "Frontend", EventType: chrometrace.CompleteEvent, ProcessID: 789, ThreadID: 3},
						// Events below should be filtered.
						{TimestampMicros: 0, DurationMicros: 800, Name: "NotComplete", ProcessID: 789, ThreadID: 4},
						{TimestampMicros: 0, DurationMicros: 10, Name: "TooShort", EventType: chrometrace.CompleteEvent, ProcessID: 789, ThreadID: 5},
					},
				},
			},
			want: []chrometrace.Trace{
				{TimestampMicros: 42, DurationMicros: 1000, Name: "ExecuteCompiler", EventType: chrometrace.CompleteEvent, ProcessID: 123, ThreadID: 321},
				{TimestampMicros: 42, DurationMicros: 100, Name: "Source", EventType: chrometrace.CompleteEvent, ProcessID: 123, ThreadID: 321},
				{TimestampMicros: 142, DurationMicros: 120, Name: "Source", EventType: chrometrace.CompleteEvent, ProcessID: 123, ThreadID: 321},
				{TimestampMicros: 342, DurationMicros: 420, Name: "Frontend", EventType: chrometrace.CompleteEvent, ProcessID: 123, ThreadID: 321},
			},
		},
		{
			desc: "outputs missing clang traces are skipped",
			traces: []chrometrace.Trace{
				{TimestampMicros: 42, DurationMicros: 1000, Name: "output.cc.o", ProcessID: 123, ThreadID: 321},
				{TimestampMicros: 100, DurationMicros: 2000, Name: "missing_trace.cc.o", ProcessID: 234, ThreadID: 432},
				{TimestampMicros: 200, DurationMicros: 900, Name: "another_output.cc.o", ProcessID: 345, ThreadID: 543},
			},
			granularity: 100 * time.Microsecond,
			clangTraces: map[string]clangTrace{
				"output.cc.json": {
					TraceEvents: []chrometrace.Trace{
						{TimestampMicros: 0, DurationMicros: 1000, Name: "ExecuteCompiler", EventType: chrometrace.CompleteEvent, ProcessID: 789, ThreadID: 1},
					},
				},
				"another_output.cc.json": {
					TraceEvents: []chrometrace.Trace{
						{TimestampMicros: 100, DurationMicros: 200, Name: "ExecuteCompiler", EventType: chrometrace.CompleteEvent, ProcessID: 789, ThreadID: 1},
					},
				},
			},
			want: []chrometrace.Trace{
				{TimestampMicros: 42, DurationMicros: 1000, Name: "ExecuteCompiler", EventType: chrometrace.CompleteEvent, ProcessID: 123, ThreadID: 321},
				{TimestampMicros: 300, DurationMicros: 200, Name: "ExecuteCompiler", EventType: chrometrace.CompleteEvent, ProcessID: 345, ThreadID: 543},
			},
		},
		{
			desc: "sub event longer than main step",
			traces: []chrometrace.Trace{
				{TimestampMicros: 42, DurationMicros: 1000, Name: "output.cc.o", ProcessID: 123, ThreadID: 321},
			},
			granularity: 100 * time.Microsecond,
			clangTraces: map[string]clangTrace{
				"output.cc.json": {
					TraceEvents: []chrometrace.Trace{
						{TimestampMicros: 0, DurationMicros: 999999, Name: "ExecuteCompiler", EventType: chrometrace.CompleteEvent, ProcessID: 789, ThreadID: 1},
					},
				},
			},
			wantErr: true,
		},
	} {
		t.Run(tc.desc, func(t *testing.T) {
			tmpDir := t.TempDir()
			for filename, trace := range tc.clangTraces {
				if err := jsonutil.WriteToFile(filepath.Join(tmpDir, filename), trace); err != nil {
					t.Fatal(err)
				}
			}
			got, err := ToInterleave(tc.traces, tmpDir, tc.granularity)
			if (err != nil) != tc.wantErr {
				t.Errorf("ToInterleave got error: %v, want error: %t", err, tc.wantErr)
			}
			if diff := cmp.Diff(tc.want, got); diff != "" {
				t.Errorf("ToInterleave got: %#v, want: %#v, diff (-want, +got):\n%s", got, tc.want, diff)
			}
		})
	}
}
