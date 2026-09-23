// Copyright 2014 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// ninjatrace converts .ninja_log into trace-viewer formats.
//
// usage:
//
//	$ go run ninjatrace.go --ninjalog out/debug-x64/.ninja_log
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"runtime"
	"runtime/pprof"
	"time"

	"go.fuchsia.dev/fuchsia/tools/build/ninjago/chrometrace"
	"go.fuchsia.dev/fuchsia/tools/build/ninjago/clangtrace"
	"go.fuchsia.dev/fuchsia/tools/build/ninjago/ninjacommand"
	"go.fuchsia.dev/fuchsia/tools/build/ninjago/rbetrace"
	"go.fuchsia.dev/fuchsia/tools/build/ninjago/readerwriters"
)

var (
	ninjaBuildTracePath = flag.String("ninjabuildtrace", "", "path of ninja_build_trace.json")
	criticalPath        = flag.Bool("critical-path", false, "whether to highlight critical path in this build, --graph must be set for this to work")

	// Flags for interleaving subtraces.
	buildDir      = flag.String("build-dir", "", "path of the directory where ninja is run; when non-empty, ninjatrace will look for subtraces in this directory to interleave in the main trace")
	granularity   = flag.Duration("granularity", 100*time.Millisecond, "time granularity used to filter short events in interleaved sub traces, for example traces from clang and rustc; this flag does NOT affect the main trace")
	rbeRPLPath    = flag.String("rbe-rpl-path", "", "path to the RPL file containing performance metrics from RBE, if set, ninjatrace will interleave RBE traces into the main trace")
	rpl2TracePath = flag.String("rpl2trace-path", "", "path to rpl2trace binary for parsing RBE's RPL files, must be set if --rbe-rpl-path is set; this flag has no effect if --rbe-rpl-path is not set")

	// Flags controlling outputs.
	traceJSON  = flag.String("trace-json", "trace.json", "output path of trace.json")
	cpuprofile = flag.String("cpuprofile", "", "file to write cpu profile")
)

func readNinjaBuildTrace(tracePath string) (traces []chrometrace.Trace, err error) {
	file, err := readerwriters.Open(tracePath)
	if err != nil {
		return nil, fmt.Errorf("opening Ninja build trace file: %v", err)
	}
	defer func() {
		if cerr := file.Close(); cerr != nil && err == nil {
			err = cerr
		}
	}()

	if err := json.NewDecoder(file).Decode(&traces); err != nil {
		return nil, err
	}

	// Augment trace.Category with categories computed from the build
	// command itself. This takes care of ignoring Fuchsia wrapper scripts
	// (that are used for remoting with RBE, tracing or hermeticity checks).

	// Limit number of goroutines to number of CPUs to minimize context switching.
	limit := runtime.GOMAXPROCS(0)
	sems := make(chan struct{}, limit)
	for i := 0; i < len(traces); i++ {
		sems <- struct{}{}
		go func(i int) {
			defer func() { <-sems }()

			if traces[i].Args == nil {
				// Workflow events do not have any arguments.
				return
			}
			command, ok := traces[i].Args["command"].(string)
			if !ok { // this event doesn't have any command'
				return
			}
			categories := ninjacommand.ComputeCommandCategories(command)
			if categories == "" {
				return
			}
			traces[i].Category = traces[i].Category + "," + categories
		}(i)
	}
	// Wait for in-flight Goroutines to finish.
	for i := 0; i < limit; i++ {
		sems <- struct{}{}
	}
	return traces, nil
}

func createAndWriteTrace(path string, traces []chrometrace.Trace) (err error) {
	f, err := readerwriters.Create(*traceJSON)
	if err != nil {
		return fmt.Errorf("creating trace output file %q: %v", path, err)
	}
	defer func() {
		if cerr := f.Close(); cerr != nil && err == nil {
			err = fmt.Errorf("closing trace ouptut file: %q: %v", path, err)
		}
	}()

	if err := json.NewEncoder(f).Encode(traces); err != nil {
		return fmt.Errorf("writing trace: %v", err)
	}
	return nil
}

func main() {
	flag.Parse()
	ctx := context.Background()

	if *ninjaBuildTracePath == "" {
		log.Fatalf("--ninjabuildtrace is required")
	}

	if *cpuprofile != "" {
		f, err := os.Create(*cpuprofile)
		if err != nil {
			log.Fatalf("Failed to create CPU profile: %v", err)
		}
		defer f.Close()
		if err := pprof.StartCPUProfile(f); err != nil {
			log.Fatalf("Failed to start CPU profile: %v", err)
		}
		defer pprof.StopCPUProfile()
	}

	// Read build trace directly.
	traces, err := readNinjaBuildTrace(*ninjaBuildTracePath)
	if err != nil {
		log.Fatalf("Failed to read Ninja build trace: %v", err)
	}

	if *buildDir != "" {
		interleaved, err := clangtrace.ToInterleave(traces, *buildDir, *granularity)
		if err != nil {
			log.Fatalf("Failed to interleave clang trace: %v", err)
		}
		traces = append(traces, interleaved...)
	}

	if *rbeRPLPath != "" {
		if *rpl2TracePath == "" {
			log.Fatal("--rpl2trace-path is empty, must be set when --rbe-rpl-path is set")
		}

		tmpDir, err := os.MkdirTemp(os.TempDir(), "ninjatrace")
		if err != nil {
			log.Fatalf("Failed to make temporary directory for RPL to Chrome trace conversion: %v", err)
		}
		rbeTrace, err := rbetrace.FromRPLFile(ctx, *rpl2TracePath, *rbeRPLPath, tmpDir)
		if err != nil {
			log.Fatalf("Failed to parse RBE's RPL file: %v", err)
		}
		traces, err = rbetrace.Interleave(traces, rbeTrace)
		if err != nil {
			log.Fatalf("Failed to interleave RBE traces: %v", err)
		}
	}

	if *traceJSON != "" {
		if err := createAndWriteTrace(*traceJSON, traces); err != nil {
			log.Fatalf("Failed to create and write trace: %v", err)
		}
	}
}
