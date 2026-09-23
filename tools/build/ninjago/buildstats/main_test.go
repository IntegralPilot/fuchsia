// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.package main

package main

import (
	"bytes"
	"compress/gzip"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"
	"github.com/google/go-cmp/cmp/cmpopts"
	"go.fuchsia.dev/fuchsia/tools/build/ninjago/chrometrace"
)

var testDataDir = flag.String("test_data_dir", "../test_data", "Path to ../test_data/; only used in GN build")

func readAndUnzip(t *testing.T, path string) *gzip.Reader {
	f, err := os.Open(path)
	if err != nil {
		t.Fatalf("Failed to read %q: %v", path, err)
	}
	t.Cleanup(func() { f.Close() })

	unzipped, err := gzip.NewReader(f)
	if err != nil {
		t.Fatalf("Failed to unzip %q: %v", path, err)
	}
	t.Cleanup(func() { unzipped.Close() })
	return unzipped
}

func getInputTraces(traces *[]chrometrace.Trace) []*chrometrace.Trace {
	result := []*chrometrace.Trace{}
	for i := 0; i < len(*traces); i++ {
		result = append(result, &((*traces)[i]))
	}
	return result
}

func TestSlowestTraces(t *testing.T) {
	for _, v := range []struct {
		name           string
		traces         []chrometrace.Trace
		maxCount       int
		wantTraceNames []string
	}{
		{
			name: "three traces",
			traces: []chrometrace.Trace{
				{
					Name:           "1",
					DurationMicros: 100,
				},
				{
					Name:           "2",
					DurationMicros: 1000,
				},
				{
					Name:           "3",
					DurationMicros: 2000,
				},
			},
			maxCount: 30,
			wantTraceNames: []string{
				"3", "2", "1",
			},
		},
	} {
		t.Run(v.name, func(t *testing.T) {
			traces := getInputTraces(&v.traces)
			slowTraces := slowestTraces(traces, v.maxCount)
			var gotTraceNames []string
			for _, slowTrace := range slowTraces {
				gotTraceNames = append(gotTraceNames, slowTrace.Name)
			}
			if diff := cmp.Diff(v.wantTraceNames, gotTraceNames, cmpopts.EquateEmpty()); diff != "" {
				t.Errorf("slowestTraces(%v, %v) got diff (-want +got):\n%s", v.traces, v.maxCount, diff)
			}
		})
	}
}

func TestExtractAndSerializeBuildStatsFromTrace(t *testing.T) {
	tracePath := filepath.Join(*testDataDir, "ninja_trace.json.gz")
	traces, err := readChromeTrace(tracePath)
	if err != nil {
		t.Fatalf("Failed to load traces from %q: %v", tracePath, err)
	}
	stats := extractBuildStatsFromTrace(traces, 0)
	if len(stats.CriticalPath) == 0 {
		t.Errorf("Critical path in stats is emtpy, expect non-empty")
	}
	if len(stats.Slowests) == 0 {
		t.Errorf("Slowest builds in stats is empty, expect non-empty")
	}
	if len(stats.CatBuildTimes) == 0 {
		t.Errorf("Build times by category in stats is empty, expect non-empty")
	}
	if len(stats.All) == 0 {
		t.Errorf("All in stats is empty, expect non-empty")
	}

	buffer := new(bytes.Buffer)
	if err := serializeBuildStats(stats, buffer); err != nil {
		t.Fatalf("Failed to serialize build stats: %v", err)
	}
	var gotStats buildStats
	if err := json.NewDecoder(buffer).Decode(&gotStats); err != nil {
		t.Fatalf("Failed to deserialize build stats: %v", err)
	}
	if diff := cmp.Diff(stats, gotStats); diff != "" {
		t.Errorf("build stats diff after deserialization (-want, +got):\n%s", diff)
	}
}

func TestExtractStatsFromTrace(t *testing.T) {
	for _, v := range []struct {
		name               string
		minActionBuildTime time.Duration
		traces             []chrometrace.Trace
		want               buildStats
	}{
		{
			name: "empty steps",
		},
		{
			name:               "successfully extract stats",
			minActionBuildTime: 0,
			traces: []chrometrace.Trace{
				{
					Name:            "a.o",
					Category:        "clang++,critical_path",
					EventType:       chrometrace.CompleteEvent,
					TimestampMicros: 0,
					DurationMicros:  3000000,
					Args: map[string]any{
						"command":     "clang++ a.cc",
						"outputs":     []any{"a.o", "aa.o", "aaa.o"},
						"drag":        "123s",
						"total float": "0s",
					},
				},
				{
					Name:            "b.o",
					Category:        "rustc,critical_path",
					EventType:       chrometrace.CompleteEvent,
					TimestampMicros: 3000000,
					DurationMicros:  2000000,
					Args: map[string]any{
						"command":     "rustc b.rs",
						"outputs":     []any{"b.o"},
						"drag":        "321s",
						"total float": "0s",
					},
				},
				{
					Name:            "c.o",
					Category:        "clang++",
					EventType:       chrometrace.CompleteEvent,
					TimestampMicros: 9000000,
					DurationMicros:  1000000,
					Args: map[string]any{
						"command":     "clang++ c.cc",
						"outputs":     []any{"c.o"},
						"total float": "789s",
					},
				},
			},
			want: buildStats{
				CriticalPath: []action{
					{
						Command:  "clang++ a.cc",
						Outputs:  []string{"a.o", "aa.o", "aaa.o"},
						End:      3 * time.Second,
						Category: "clang++",
					},
					{
						Command:  "rustc b.rs",
						Outputs:  []string{"b.o"},
						Start:    3 * time.Second,
						End:      5 * time.Second,
						Category: "rustc",
					},
				},
				Slowests: []action{
					{
						Command:  "clang++ a.cc",
						Outputs:  []string{"a.o", "aa.o", "aaa.o"},
						End:      3 * time.Second,
						Category: "clang++",
					},
					{
						Command:  "rustc b.rs",
						Outputs:  []string{"b.o"},
						Start:    3 * time.Second,
						End:      5 * time.Second,
						Category: "rustc",
					},
					{
						Command:  "clang++ c.cc",
						Outputs:  []string{"c.o"},
						Start:    9 * time.Second,
						End:      10 * time.Second,
						Category: "clang++",
					},
				},
				CatBuildTimes: []catBuildTime{
					{
						Category:     "clang++",
						Count:        2,
						BuildTime:    4 * time.Second,
						MinBuildTime: time.Second,
						MaxBuildTime: 3 * time.Second,
					},
					{
						Category:     "rustc",
						Count:        1,
						BuildTime:    2 * time.Second,
						MinBuildTime: 2 * time.Second,
						MaxBuildTime: 2 * time.Second,
					},
				},
				All: []action{
					{
						Command:  "clang++ a.cc",
						Outputs:  []string{"a.o", "aa.o", "aaa.o"},
						End:      3 * time.Second,
						Category: "clang++",
					},
					{
						Command:  "rustc b.rs",
						Outputs:  []string{"b.o"},
						Start:    3 * time.Second,
						End:      5 * time.Second,
						Category: "rustc",
					},
					{
						Command:  "clang++ c.cc",
						Outputs:  []string{"c.o"},
						Start:    9 * time.Second,
						End:      10 * time.Second,
						Category: "clang++",
					},
				},
				Actions: []action{
					{
						Command:  "clang++ a.cc",
						Outputs:  []string{"a.o", "aa.o", "aaa.o"},
						End:      3 * time.Second,
						Category: "clang++",
					},
					{
						Command:  "rustc b.rs",
						Outputs:  []string{"b.o"},
						Start:    3 * time.Second,
						End:      5 * time.Second,
						Category: "rustc",
					},
					{
						Command:  "clang++ c.cc",
						Outputs:  []string{"c.o"},
						Start:    9 * time.Second,
						End:      10 * time.Second,
						Category: "clang++",
					},
				},
				TotalBuildTime: 6 * time.Second,
				BuildDuration:  10 * time.Second,
			},
		},
		{
			name:               "filter short actions",
			minActionBuildTime: time.Minute,
			traces: []chrometrace.Trace{
				{
					Name:            "1",
					Category:        "unknown",
					TimestampMicros: 0,
					DurationMicros:  1000000,
					Args: map[string]any{
						"outputs": []any{"1"},
					},
				},
				{
					Name:            "2",
					Category:        "unknown",
					TimestampMicros: 0,
					DurationMicros:  60 * 1000000,
					Args: map[string]any{
						"outputs": []any{"2"},
					},
				},
				{
					Name:            "3",
					Category:        "unknown",
					TimestampMicros: 0,
					DurationMicros:  2 * 60 * 1000000,
					Args: map[string]any{
						"outputs": []any{"3"},
					},
				},
			},
			want: buildStats{
				Slowests: []action{
					{Outputs: []string{"3"}, Category: "unknown", End: 2 * time.Minute},
					{Outputs: []string{"2"}, Category: "unknown", End: time.Minute},
					{Outputs: []string{"1"}, Category: "unknown", End: time.Second},
				},
				CatBuildTimes: []catBuildTime{
					{
						Category:     "unknown",
						Count:        3,
						BuildTime:    3*time.Minute + time.Second,
						MinBuildTime: time.Second,
						MaxBuildTime: 2 * time.Minute,
					},
				},
				All: []action{
					{Outputs: []string{"2"}, Category: "unknown", End: time.Minute},
					{Outputs: []string{"3"}, Category: "unknown", End: 2 * time.Minute},
				},
				Actions: []action{
					{Outputs: []string{"2"}, Category: "unknown", End: time.Minute},
					{Outputs: []string{"3"}, Category: "unknown", End: 2 * time.Minute},
				},
				TotalBuildTime: 3*time.Minute + time.Second,
				BuildDuration:  2 * time.Minute,
			},
		},
	} {
		t.Run(v.name, func(t *testing.T) {
			traces := getInputTraces(&v.traces)
			gotStats := extractBuildStatsFromTrace(traces, v.minActionBuildTime)
			if diff := cmp.Diff(v.want, gotStats, cmpopts.EquateEmpty()); diff != "" {
				t.Errorf("extractBuildStats(%#v, %s) got stats diff (-want +got):\n%s", v.traces, v.minActionBuildTime, diff)
			}
		})
	}
}
