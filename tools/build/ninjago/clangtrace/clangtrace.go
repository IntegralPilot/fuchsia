// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// package clangtrace contains utilities for working with Clang traces.
package clangtrace

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"go.fuchsia.dev/fuchsia/tools/build/ninjago/chrometrace"
)

// clangTrace matches the JSON output format from clang when time-trace is
// enabled.
type clangTrace struct {
	// TraceEvents contains all events in this trace.
	TraceEvents []chrometrace.Trace
	// BeginningOfTimeMicros identifies the time when this clang command started,
	// using microseconds since epoch.
	BeginningOfTimeMicros int `json:"beginningOfTime"`
}

// ToInterleave returns all clang traces that can be interleaved
// directly into their corresponding build steps from `mainTraces`.
//
// `buildRoot` should point to the directory where the Ninja build of
// `mainTrace` is executed, where this function will look for clang traces next
// to object files built by clang. Object files with no clang traces next to
// them are skipped.
//
// Clang traces include events with very short durations, so `granularity` is
// provided to filter them and reduce the size of returned slice.
func ToInterleave(mainTraces []chrometrace.Trace, buildRoot string, granularity time.Duration) ([]chrometrace.Trace, error) {
	var interleaved []chrometrace.Trace

	for _, mainTrace := range mainTraces {
		if !strings.HasSuffix(mainTrace.Name, ".o") {
			continue
		}

		// Clang writes a .json file next to the compiled object file when
		// time-trace is enabled.
		//
		// https://releases.llvm.org/9.0.0/tools/clang/docs/ReleaseNotes.html#new-compiler-flags
		clangTracePath := filepath.Join(buildRoot, strings.TrimSuffix(mainTrace.Name, ".o")+".json")
		if _, err := os.Stat(clangTracePath); err != nil {
			if os.IsNotExist(err) {
				continue
			}
			return nil, err
		}
		traceFile, err := os.Open(clangTracePath)
		if err != nil {
			return nil, err
		}
		var cTrace clangTrace
		if err := json.NewDecoder(traceFile).Decode(&cTrace); err != nil {
			return nil, fmt.Errorf("failed to decode clang trace %s: %w", clangTracePath, err)
		}

		for _, t := range cTrace.TraceEvents {
			// Event names starting with "Total" are sums of event durations of a
			// type, for example: "Total Frontend". They are used to form a bar chart
			// in clang traces, so they always stat from time 0 on separate threads.
			// We exclude them so they don't cause interleaved events to misalign.
			if strings.HasPrefix(t.Name, "Total ") || t.EventType != chrometrace.CompleteEvent || t.DurationMicros < int(granularity/time.Microsecond) {
				continue
			}

			if t.DurationMicros > mainTrace.DurationMicros {
				return nil, fmt.Errorf("clang trace for %q has an event %q with duration %dµs, which is longer than the duration of the this clang build %dµs, please make sure they are from the same build", mainTrace.Name, t.Name, t.DurationMicros, mainTrace.DurationMicros)
			}

			// Left align this event with the corresponding step in the main trace,
			// and put them in the same thread, so in the trace viewer it will be
			// displayed right below that step.
			t.ProcessID = mainTrace.ProcessID
			t.ThreadID = mainTrace.ThreadID
			t.TimestampMicros += mainTrace.TimestampMicros

			interleaved = append(interleaved, t)
		}
	}
	return interleaved, nil
}
