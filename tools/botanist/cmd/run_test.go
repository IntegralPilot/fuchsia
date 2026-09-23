// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"encoding/json"
	"io"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"go.fuchsia.dev/fuchsia/tools/botanist"
	"go.fuchsia.dev/fuchsia/tools/botanist/targets"
	"go.fuchsia.dev/fuchsia/tools/lib/ffxutil"
	"go.fuchsia.dev/fuchsia/tools/net/sshutil"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
	testrunnerconstants "go.fuchsia.dev/fuchsia/tools/testing/testrunner/constants"
	"golang.org/x/sync/errgroup"
)

type MockTarget struct {
	ffx *targets.FFXInstance
}

func (m *MockTarget) TestConfig(expectsSSH bool) (any, error) { return nil, nil }
func (m *MockTarget) AddFFXPackageRepository(ctx context.Context, pkgRepoName string, pkgSrvPort int, useForward bool) (func(), error) {
	return func() {}, nil
}
func (m *MockTarget) AddPackageRepository(client *sshutil.Client, repoURL, blobURL string) error {
	return nil
}
func (m *MockTarget) CaptureSerialLog(filename string) error { return nil }
func (m *MockTarget) CaptureSyslog(client *sshutil.Client, filename string, pkgSrv *botanist.PackageServer) error {
	return nil
}
func (m *MockTarget) StopSyslog() {}
func (m *MockTarget) IPv6() (*net.IPAddr, error) {
	// Sleep to allow StartFFXMonitor to run before runAgainstTarget fails/returns
	time.Sleep(500 * time.Millisecond)
	addr, _ := net.ResolveIPAddr("ip6", "::1")
	return addr, nil
}
func (m *MockTarget) IPv4() (net.IP, error) {
	return net.ParseIP("127.0.0.1"), nil
}
func (m *MockTarget) Nodename() string                    { return "mock-target" }
func (m *MockTarget) Serial() io.ReadWriteCloser          { return nil }
func (m *MockTarget) SerialSocketPath() string            { return "" }
func (m *MockTarget) SetConnectionTimeout(time.Duration)  {}
func (m *MockTarget) ResolveIP() error                    { return nil }
func (m *MockTarget) SSHClient() (*sshutil.Client, error) { return nil, nil }
func (m *MockTarget) SSHControlMasterPath() string        { return "" }
func (m *MockTarget) SetupSSHControlMaster(ctx context.Context, sshKey, addr string) (func(), error) {
	return func() {}, nil
}
func (m *MockTarget) SSHControlMasterRunning() bool { return false }
func (m *MockTarget) SSHKey() string                { return "ssh-key" }
func (m *MockTarget) Start(ctx context.Context, args []string, pbPath string, isBootTest bool) error {
	return nil
}
func (m *MockTarget) StartSerialServer() error   { return nil }
func (m *MockTarget) Stop() error                { return nil }
func (m *MockTarget) Wait(context.Context) error { return nil }
func (m *MockTarget) SetFFX(ffx *targets.FFXInstance, env []string) {
	m.ffx = ffx
}
func (m *MockTarget) GetFFX() *targets.FFXInstance { return m.ffx }
func (m *MockTarget) UseFFXExperiment(exp botanist.Experiment) bool {
	return m.ffx.Experiments.Contains(exp)
}
func (m *MockTarget) UseProductBundles() bool { return false }
func (m *MockTarget) FFXEnv() []string        { return nil }
func (m *MockTarget) GetSharedData() string   { return "" }

func TestStartFFXMonitor(t *testing.T) {
	tmpDir := t.TempDir()
	ffxPath := filepath.Join(tmpDir, "ffx")
	// Create a mock ffx script that echoes arguments
	if err := os.WriteFile(ffxPath, []byte("#!/bin/bash\necho $@"), 0o755); err != nil {
		t.Fatal("failed to write mock ffx tool")
	}

	// Create test SSH keys
	sshPriv := filepath.Join(tmpDir, "ssh_priv")
	sshPub := filepath.Join(tmpDir, "ssh_pub")
	if err := os.WriteFile(sshPriv, []byte("PRIVATE KEY"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(sshPub, []byte("PUBLIC KEY"), 0o600); err != nil {
		t.Fatal(err)
	}

	ctx := context.Background()
	ffxInstance, err := ffxutil.NewFFXInstance(ctx, ffxPath, tmpDir, nil, "target", &ffxutil.SSHInfo{SshPriv: sshPriv, SshPub: sshPub}, tmpDir, ffxutil.UseFFXLegacy)
	if err != nil {
		t.Fatalf("failed to create ffx instance: %s", err)
	}

	// Capture stdout
	rOut, wOut, _ := os.Pipe()
	ffxInstance.SetStdoutStderr(wOut, wOut)

	targetFFX := &targets.FFXInstance{FFXInstance: ffxInstance}
	target := &MockTarget{ffx: targetFFX}

	cmd := &RunCommand{
		expectsSSH:        true,
		productBundles:    "test-bundles",
		productBundleName: "test-bundle-name",
	}
	eg, ctx := errgroup.WithContext(ctx)
	cancel := func() {}

	// Set test out dir env var because StartFFXMonitor uses it
	os.Setenv(testrunnerconstants.TestOutDirEnvKey, tmpDir)
	defer os.Unsetenv(testrunnerconstants.TestOutDirEnvKey)

	experiments := make(botanist.Experiments)
	experiments[string(botanist.UseFFXMonitor)] = struct{}{}

	const (
		monitorName         = "ffx_monitor"
		aggregationFilename = "aggregation.json"
	)

	// Create an initial summary.json
	summaryPath := filepath.Join(tmpDir, cmd.testrunnerOptions.OutDir, runtests.TestSummaryFilename)
	initialSummary := runtests.TestSummary{Tests: []runtests.TestDetails{}}
	summaryBytes, _ := json.Marshal(initialSummary)
	if err := os.WriteFile(summaryPath, summaryBytes, 0o600); err != nil {
		t.Fatalf("failed to write initial summary.json: %v", err)
	}

	// Run dispatchTests
	cmd.dispatchTests(ctx, cancel, eg, nil, []targets.FuchsiaTarget{target}, target, nil, "invalid-tests-path", experiments)

	// Wait for goroutines to complete
	_ = eg.Wait()

	wOut.Close()
	outBytes, _ := io.ReadAll(rOut)
	outStr := string(outBytes)

	// Check if monitor start command was issued with correct args
	if !strings.Contains(outStr, "monitor start") {
		t.Errorf("expected monitor start command, got: %s", outStr)
	}
	if !strings.Contains(outStr, "--log-file") {
		t.Errorf("expected --log-file arg, got: %s", outStr)
	}
	expectedLogFile := filepath.Join(tmpDir, cmd.testrunnerOptions.OutDir, monitorName, "device.status.json")
	if !strings.Contains(outStr, expectedLogFile) {
		t.Errorf("expected log file %s, got output: %s", expectedLogFile, outStr)
	}
	if !strings.Contains(outStr, "--aggregations-file") {
		t.Errorf("expected --aggregations-file arg, got: %s", outStr)
	}
	expectedAggregationsFile := filepath.Join(tmpDir, cmd.testrunnerOptions.OutDir, monitorName, aggregationFilename)
	if !strings.Contains(outStr, expectedAggregationsFile) {
		t.Errorf("expected aggregations file %s, got output: %s", expectedAggregationsFile, outStr)
	}

	// Verify summary.json was updated
	data, err := os.ReadFile(summaryPath)
	if err != nil {
		t.Fatalf("failed to read summary.json: %v", err)
	}
	var summary runtests.TestSummary
	if err := json.Unmarshal(data, &summary); err != nil {
		t.Fatalf("failed to unmarshal summary.json: %v", err)
	}
	found := false
	for _, test := range summary.Tests {
		if test.Name == monitorName {
			found = true
			if test.OutputDir != monitorName {
				t.Errorf("expected OutputDir %s, got %s", monitorName, test.OutputDir)
			}
			if len(test.OutputFiles) != 1 || test.OutputFiles[0] != aggregationFilename {
				t.Errorf("expected OutputFiles [%s], got %v", aggregationFilename, test.OutputFiles)
			}
			break
		}
	}
	if !found {
		t.Error("ffx_monitor entry not found in summary.json")
	}
}
