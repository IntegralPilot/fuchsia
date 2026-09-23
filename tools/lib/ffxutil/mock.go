// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package ffxutil

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"

	"go.fuchsia.dev/fuchsia/tools/build"
)

type MockFFXInstance struct {
	CmdsCalled  []string
	TestOutcome string
	Output      string
	stdout      io.Writer
	stderr      io.Writer
}

func (f *MockFFXInstance) SetTarget(target string) {
}

func (f *MockFFXInstance) Stdout() io.Writer {
	if f.stdout == nil {
		return os.Stdout
	}
	return f.stdout
}

func (f *MockFFXInstance) Stderr() io.Writer {
	if f.stderr == nil {
		return os.Stderr
	}
	return f.stderr
}

func (f *MockFFXInstance) SetStdoutStderr(stdout, stderr io.Writer) {
	f.stdout = stdout
	f.stderr = stderr
}

func (f *MockFFXInstance) run(cmd string, args ...string) error {
	f.CmdsCalled = append(f.CmdsCalled, fmt.Sprintf("%s:%s", cmd, strings.Join(args, " ")))
	return nil
}

func (f *MockFFXInstance) Run(_ context.Context, args ...string) error {
	return nil
}

func (f *MockFFXInstance) RunWithTarget(_ context.Context, args ...string) error {
	return nil
}

func (f *MockFFXInstance) RunWithTargetAndTimeout(_ context.Context, _ time.Duration, args ...string) error {
	return nil
}

func (f *MockFFXInstance) TestEarlyBootProfile(_ context.Context, outDir string) error {
	return nil
}

func (f *MockFFXInstance) TestRun(_ context.Context, testList build.TestList, outDir string, args ...string) (*TestRunResult, error) {
	if testList.SchemaID != build.TestListSchemaIDExperimental {
		return nil, fmt.Errorf(`schema_id must be %q, found %q`, build.TestListSchemaIDExperimental, testList.SchemaID)
	}
	f.run("test", args...)
	if _, err := f.Stdout().Write([]byte(f.Output)); err != nil {
		return nil, err
	}
	return f.WriteRunResult(testList, outDir)
}

func (f *MockFFXInstance) WriteRunResult(testList build.TestList, outDir string) (*TestRunResult, error) {
	outcome := TestPassed
	if f.TestOutcome != "" {
		outcome = f.TestOutcome
	}
	if err := os.MkdirAll(outDir, os.ModePerm); err != nil {
		return nil, err
	}
	var suites []SuiteResult
	for i, test := range testList.Data {
		relTestDir := fmt.Sprintf("test%d", i)
		if err := os.Mkdir(filepath.Join(outDir, relTestDir), os.ModePerm); err != nil {
			return nil, err
		}
		if err := os.WriteFile(filepath.Join(outDir, relTestDir, "report.txt"), []byte("stdio"), os.ModePerm); err != nil {
			return nil, err
		}
		suiteResult := SuiteResult{
			Outcome: outcome,
			Name:    test.Execution.ComponentURL,
			Cases: []CaseResult{
				{
					Outcome:              outcome,
					Name:                 "case1",
					StartTime:            time.Now().UnixMilli(),
					DurationMilliseconds: 1000,
				},
			},
			StartTime:            time.Now().UnixMilli(),
			DurationMilliseconds: 1000,
			Artifacts: map[string]ArtifactMetadata{
				"report.txt": {
					ArtifactType: ReportType,
				}},
			ArtifactDir: relTestDir,
		}
		suites = append(suites, suiteResult)
	}
	runArtifactDir := "artifact-run"
	debugDir := "debug"
	earlyBootProfile := filepath.Join(outDir, runArtifactDir, debugDir, "llvm-profile", "kernel.profraw")
	if err := os.MkdirAll(filepath.Dir(earlyBootProfile), os.ModePerm); err != nil {
		return nil, err
	}
	if err := os.WriteFile(earlyBootProfile, []byte("data"), os.ModePerm); err != nil {
		return nil, err
	}

	runResult := &TestRunResult{
		Artifacts: map[string]ArtifactMetadata{
			debugDir: {
				ArtifactType: DebugType,
			},
		},
		ArtifactDir: runArtifactDir,
		Outcome:     outcome,
		Suites:      suites,
		outputDir:   outDir,
	}
	runResultEnvelope := TestRunResultEnvelope{
		Data:     *runResult,
		SchemaID: runSummarySchemaID,
	}
	runResultBytes, err := json.Marshal(runResultEnvelope)
	if err != nil {
		return nil, err
	}
	if err := os.WriteFile(filepath.Join(outDir, runSummaryFilename), runResultBytes, os.ModePerm); err != nil {
		return nil, err
	}
	return runResult, nil
}

func (f *MockFFXInstance) PackageResolve(_ context.Context, packageURL string) error {
	return f.run("resolve", packageURL)
}

func (f *MockFFXInstance) Snapshot(_ context.Context, _, _ string) error {
	return f.run("snapshot")
}

func (f *MockFFXInstance) Stop() error {
	return f.run("stop")
}

func (f *MockFFXInstance) ContainsCmd(cmd string, args ...string) bool {
	for _, c := range f.CmdsCalled {
		parts := strings.SplitN(c, ":", 2)
		if parts[0] == cmd {
			for _, arg := range args {
				if !strings.Contains(parts[1], arg) {
					return false
				}
			}
			return true
		}
	}
	return false
}

func (f *MockFFXInstance) TargetWait(_ context.Context, args ...string) error {
	return nil
}

func (f *MockFFXInstance) Doctor(_ context.Context) error {
	return f.run("doctor")
}
