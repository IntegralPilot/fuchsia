// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"path/filepath"

	"go.fuchsia.dev/fuchsia/tools/lib/flagmisc"
	"go.fuchsia.dev/fuchsia/tools/lib/osmisc"
	"go.fuchsia.dev/fuchsia/tools/testing/tefmocheck"
)

func usage() {
	fmt.Printf(`tefmocheck [flags]

Reads inputs from [flags] and writes a JSON formatted summary to stdout.
The summary contains a synthetic test for each supported failure mode.
`)
}

func main() {
	help := flag.Bool("help", false, "Whether to show Usage and exit.")
	flag.Usage = usage
	swarmingSummaryPath := flag.String("swarming-summary-json", "", "Path to the Swarming task summary file. Required.")
	swarmingHost := flag.String("swarming-host", "", "Swarming server host. Optional.")
	inputSummaryPath := flag.String("test-summary-json", "", "Path to test summary file. Optional.")
	swarmingOutputPath := flag.String("swarming-output", "", "Path to a file containing the stdout and stderr of the Swarming task. Optional.")
	var syslogPaths flagmisc.StringsValue
	flag.Var(&syslogPaths, "syslog", "Repeated flag, path to a file containing the syslog. Optional.")
	var serialLogPaths flagmisc.StringsValue
	flag.Var(&serialLogPaths, "serial-log", "Repeated flag, path to a file containing the serial log. Optional.")
	outputsDir := flag.String("outputs-dir", "", "If set, will produce text output files for the produced tests in this dir. Optional.")
	jsonOutput := flag.String("json-output", "", "Output summary.json to this path.")
	var checksConfigPaths flagmisc.StringsValue
	flag.Var(&checksConfigPaths, "checks-config", "Repeated flag, path to a file containing FailureModeChecks to load. Optional.")
	flag.Parse()

	if *help || flag.NArg() > 0 || *swarmingSummaryPath == "" {
		flag.Usage()
		flag.PrintDefaults()
		if *help {
			return
		}
		os.Exit(64)
	}

	swarmingSummary, err := tefmocheck.LoadSwarmingTaskSummary(*swarmingSummaryPath)
	if err != nil {
		log.Fatal(err)
	}
	swarmingSummary.Host = *swarmingHost

	inputSummary, err := tefmocheck.LoadTestSummary(*inputSummaryPath)
	if err != nil {
		log.Fatal(err)
	}

	var serialLogs [][]byte
	for _, serialLogPath := range serialLogPaths {
		serialLog, err := os.ReadFile(serialLogPath)
		if err != nil {
			log.Fatalf("failed to read serial log from %s: %v", serialLogPath, err)
		}
		serialLogs = append(serialLogs, serialLog)
	}

	var swarmingOutput []byte
	var swarmingOutputPerTest []tefmocheck.TestLog
	if *swarmingOutputPath != "" {
		swarmingOutput, err = os.ReadFile(*swarmingOutputPath)
		if err != nil {
			log.Fatalf("failed to read swarming output from %s: %v", *swarmingOutputPath, err)
		}
		swarmingOutputPerTest, err = tefmocheck.SplitTestLogs(swarmingOutput, filepath.Base(*swarmingOutputPath), *outputsDir, inputSummary.Tests)
		if err != nil {
			log.Fatalf("failed to split swarming output into per-test logs: %s", err)
		}
	}
	// inputSummary is empty if -test-summary-json is not specified. This happens
	// when the recipe detects no summary.json exists.
	if *outputsDir != "" && *inputSummaryPath != "" {
		for i := range swarmingOutputPerTest {
			test := &inputSummary.Tests[i]
			testLog := &swarmingOutputPerTest[i]
			// TODO(olivernewman): This check fails when a unit test for
			// testrunner fails and the failure output contains the TAP output
			// referencing fake tests used for test data. Make this check less
			// strict by having `SplitTestLogs` only create a split log for
			// tests that actually appear in summary.json.
			if test.Name != testLog.TestName {
				log.Fatalf("swarmingOutputPerTest[%d].TestName != inputSummary.Tests[%d] (%q vs %q)", i, i, testLog.TestName, test.Name)
			}
			relPath, err := filepath.Rel(filepath.Join(*outputsDir, test.OutputDir), testLog.FilePath)
			if err != nil {
				log.Fatal(err)
			}
			test.OutputFiles = append(test.OutputFiles, relPath)
		}
	}

	var syslogs [][]byte
	for _, syslogPath := range syslogPaths {
		syslog, err := os.ReadFile(syslogPath)
		if err != nil {
			log.Fatalf("failed to read syslog from %s: %v", syslogPath, err)
		}
		syslogs = append(syslogs, syslog)
	}

	testingOutputs := tefmocheck.TestingOutputs{
		TestSummary:           inputSummary,
		SwarmingSummary:       swarmingSummary,
		SerialLogs:            serialLogs,
		SwarmingOutput:        swarmingOutput,
		SwarmingOutputPerTest: swarmingOutputPerTest,
		Syslogs:               syslogs,
	}

	// These should be ordered from most specific to least specific. If an earlier
	// check finds a failure mode, then we skip running later checks because we assume
	// they'll add no useful information.
	checks := []tefmocheck.FailureModeCheck{}

	// Parse FailureModeChecks from external sources if provided.
	for _, configPath := range checksConfigPaths {
		checksFromFile, err := tefmocheck.LoadChecksFromFile(configPath)
		if err != nil {
			// If a configuration file is explicitly requested, failing to load it
			// must be a fatal error.
			log.Fatalf("failed to load checks from %s: %v", configPath, err)
		}
		checks = append(checks, checksFromFile...)
	}

	checks = append(checks, &tefmocheck.CdcEthernetStateCheck{
		RequiredTags: map[string]string{
			"device_type": "Sorrel",
			"product":     "vendor/google/products/fuchsia_internal.gni",
		},
	})
	checks = append(checks, tefmocheck.NearbyStringsChecks()...)
	checks = append(checks, tefmocheck.FailureReasonChecks()...)
	checks = append(checks, tefmocheck.MassTestFailureCheck{MaxFailed: 5})
	// TaskStateChecks should go toward the end, since they're not very specific.
	checks = append(checks, tefmocheck.TaskStateChecks...)
	// No tests being run is only an issue if the task didn't fail for another
	// reason, since conditions handled by many other checks can result in a
	// missing summary.json. So run this check last.
	checks = append(checks, tefmocheck.NoTestsRanCheck{})

	checkTests, err := tefmocheck.RunChecks(checks, &testingOutputs, *outputsDir)
	if err != nil {
		log.Fatalf("failed to run checks: %v", err)
	}

	inputSummary.Tests = append(inputSummary.Tests, checkTests...)
	outFile := os.Stdout
	if *jsonOutput != "" {
		outFile, err = osmisc.CreateFile(*jsonOutput)
		if err != nil {
			log.Fatalf("failed to create output file: %s", err)
		}
	}
	summaryBytes, err := json.MarshalIndent(inputSummary, "", "  ")
	if err != nil {
		log.Fatalf("failed to marshal output test summary: %s", err)
	}
	summaryBytes = append(summaryBytes, []byte("\n")...) // Terminate output with new line.
	if _, err := outFile.Write(summaryBytes); err != nil {
		log.Fatalf("failed to write summary: %s", err)
	}
}
