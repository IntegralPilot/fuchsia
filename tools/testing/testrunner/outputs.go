// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package testrunner

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"io/fs"
	"net/url"
	"path/filepath"
	"strconv"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/lib/osmisc"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
	"go.fuchsia.dev/fuchsia/tools/testing/tap"
)

// TestOutputs manages the test runner's output drivers. Upon completion, if tar output is
// initialized, a TAR archive containing all other outputs is produced.
type TestOutputs struct {
	OutDir  string
	Summary runtests.TestSummary
	tap     *tap.Producer
}

func CreateTestOutputs(producer *tap.Producer, outdir string) (*TestOutputs, error) {
	if outdir == "" {
		return nil, fmt.Errorf("outdir must be set")
	}
	return &TestOutputs{
		OutDir: outdir,
		tap:    producer,
	}, nil
}

// rebaseOutputFiles takes the list of outputFiles and rebases them against the
// newOutputDir. If an `output file` refers to a directory, all the files in that
// directory will be returned in the list of outputs.
func (o *TestOutputs) rebaseOutputFiles(outputFiles []string, oldOutputDir, newOutputDir string) ([]string, error) {
	var allOutputs []string
	for _, outputFilePath := range outputFiles {
		outputFilePath = filepath.Join(oldOutputDir, outputFilePath)
		if err := filepath.Walk(outputFilePath, func(path string, info fs.FileInfo, err error) error {
			if err != nil {
				return err
			}
			if !info.IsDir() {
				pathRel, err := filepath.Rel(newOutputDir, path)
				if err != nil {
					return fmt.Errorf("failed to get relative path of %s to %s: %w", path, newOutputDir, err)
				}
				allOutputs = append(allOutputs, pathRel)
			}
			return nil
		}); err != nil {
			return nil, err
		}
	}
	return allOutputs, nil
}

// Record writes the test result to initialized outputs.
func (o *TestOutputs) Record(ctx context.Context, result runtests.TestDetails) error {
	// Sponge doesn't seem to like the path if we just put Name in there.
	nameForPath := url.PathEscape(strings.ReplaceAll(result.Name, ":", ""))
	outputRelPath := filepath.Join(nameForPath, strconv.Itoa(result.RunIndex))
	// Strip any leading //.
	outputRelPath = strings.TrimLeft(outputRelPath, "//")

	stdioPath := filepath.Join(outputRelPath, runtests.TestOutputFilename)

	duration := result.Duration()
	if duration < 0 {
		return fmt.Errorf("test %q must have non-negative duration: (start, end) = (%s, %s)", result.Name, result.StartTime, result.EndTime)
	}

	// Rebase outputs from test against the global outdir.
	suiteOutputFiles, err := o.rebaseOutputFiles(result.OutputFiles, result.OutputDir, filepath.Join(o.OutDir, outputRelPath))
	if err != nil {
		return fmt.Errorf("error rebasing output files: %w", err)
	}
	containsStdio := false
	for _, outputFile := range suiteOutputFiles {
		if filepath.Base(outputFile) == runtests.TestOutputFilename {
			containsStdio = true
			break
		}
	}

	// If the stdout/stderr file didn't already exist in the test result's OutputFiles,
	// create it using the bytes from the test Stdio.
	//
	// We'll write a file even if the stdio is empty for consistency, and to
	// make it clear that a file's stdio was empty versus infra silently failing
	// to create the file.
	if !containsStdio {
		pathWriter, err := osmisc.CreateFile(filepath.Join(o.OutDir, stdioPath))
		if err != nil {
			return fmt.Errorf("failed to create stdio file for test %q: %w", result.Name, err)
		}
		defer pathWriter.Close()
		if _, err := pathWriter.Write(result.Stdio); err != nil {
			return fmt.Errorf("failed to write stdio file for test %q: %w", result.Name, err)
		}

		suiteOutputFiles = append(suiteOutputFiles, runtests.TestOutputFilename)
	}

	var cases []runtests.TestCaseResult
	for _, testCase := range result.Cases {
		caseOutputFiles, err := o.rebaseOutputFiles(testCase.OutputFiles, testCase.OutputDir, filepath.Join(o.OutDir, outputRelPath))
		if err != nil {
			return fmt.Errorf("error rebasing output files: %w", err)
		}
		newCase := testCase
		newCase.OutputFiles = caseOutputFiles
		newCase.OutputDir = ""
		cases = append(cases, newCase)
	}

	// Only append the test summary after writing all output files to disk. This
	// ensures that even if writing the output files fails, the summary won't
	// reference nonexistent files.
	o.Summary.Tests = append(o.Summary.Tests, runtests.TestDetails{
		Name:        result.Name,
		GNLabel:     result.GNLabel,
		SourceLabel: result.SourceLabel,
		Status:      result.Status,
		TestResult: runtests.TestResult{
			OutputFiles:       suiteOutputFiles,
			OutputDir:         outputRelPath,
			Cases:             cases,
			FailureReason:     result.FailureReason,
			SetupSucceeded:    result.SetupSucceeded,
			TeardownSucceeded: result.TeardownSucceeded,
			ExitCode:          result.ExitCode,
		},
		StartTime:      result.StartTime,
		DurationMillis: duration.Milliseconds(),
		DataSinks:      result.DataSinks,
		Affected:       result.Affected,
		Tags:           result.Tags,
		Metadata:       result.Metadata,
	})

	desc := fmt.Sprintf("%s (%s)", result.Name, duration)
	if o.tap != nil {
		o.tap.Ok(result.Passed(), desc)
	}

	return nil
}

// UpdateDataSinks updates the DataSinks field of the tests in the summary with
// the provided `newSinks`. If the sinks were copied to a subdirectory within
// o.outDir, that path should be provided as the `insertPrefixPath` which will
// get prepended to the sink file paths so that they point to the correct paths
// relative to o.outDir.
func (o *TestOutputs) updateDataSinks(newSinks map[string]runtests.DataSinkMap, insertPrefixPath string) {
	for i, test := range o.Summary.Tests {
		if sinkRef, ok := newSinks[test.Name]; ok {
			if test.DataSinks == nil {
				test.DataSinks = runtests.DataSinkMap{}
			}
			for name, sinks := range sinkRef {
				for _, sink := range sinks {
					sink.File = filepath.Join(insertPrefixPath, sink.File)
					test.DataSinks[name] = append(test.DataSinks[name], sink)
				}
			}
			o.Summary.Tests[i] = test
		}
	}
}

// Close stops the recording of test outputs; it must be called to finalize them.
func (o *TestOutputs) Close() error {
	if o.OutDir == "" {
		return nil
	}
	summaryBytes, err := json.Marshal(o.Summary)
	if err != nil {
		return err
	}
	summaryPath := filepath.Join(o.OutDir, runtests.TestSummaryFilename)
	s, err := osmisc.CreateFile(summaryPath)
	if err != nil {
		return fmt.Errorf("failed to create file: %w", err)
	}
	defer s.Close()
	_, err = io.Copy(s, bytes.NewBuffer(summaryBytes))
	return err
}
