// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package orchestrate

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"
	ffx "go.fuchsia.dev/fuchsia/tools/orchestrate/ffx"
	utils "go.fuchsia.dev/fuchsia/tools/orchestrate/utils"
)

// mockFFXClient is a mock implementation of the FFXClient interface.
type mockFFXClient struct {
	t *testing.T
	// Expected calls and their results.
	calls   []mockCall
	callIdx int

	// Store arguments for later inspection if needed
	defaultTarget *string
	// The environment that was applied by ApplyEnv.
	recordedEnv []string

	// Path to a fake executable for Cmd() calls
	fakeExecPath string

	// For specific command outputs if needed
	repoServerListOutput string
	configGetOutput      string
}

type mockCall struct {
	method string
	args   []string // Store args as string slice for simpler comparison
	retErr error
	retVal any // For methods that return non-error values
}

func (m *mockFFXClient) Close() error {
	call := m.recordCall("Close")
	return call.retErr
}

func (m *mockFFXClient) ApplyEnv(env []string) ([]string, error) {
	// ApplyEnv takes []string, but we record args as flattened strings.
	// We can skip checking env content for now or just check it was called.
	call := m.recordCall("ApplyEnv")
	if call.retErr != nil {
		return nil, call.retErr
	}
	if val := utils.GetOutputsDir(); val != "" {
		env = append(env, fmt.Sprintf("FFX_ISOLATE_DIR=%s", val))
	}
	if m.defaultTarget != nil {
		env = append(env, fmt.Sprintf("FUCHSIA_NODENAME=%s", *m.defaultTarget))
	}
	m.recordedEnv = env
	return env, nil
}

func (m *mockFFXClient) SetDefaultTarget(target *string) {
	val := "<nil>"
	if target != nil {
		val = *target
	}
	m.recordCall("SetDefaultTarget", val)
	m.defaultTarget = target
}

func (m *mockFFXClient) SetupFfx(ctx context.Context, repoName string) error {
	call := m.recordCall("SetupFfx", repoName)
	return call.retErr
}

func (m *mockFFXClient) EmuStop(ctx context.Context) error {
	call := m.recordCall("EmuStop")
	return call.retErr
}

type mockTargetLogCloser struct{}

func (c *mockTargetLogCloser) Close() error { return nil }

func (m *mockFFXClient) TargetLogStart(ctx context.Context, output io.Writer) (io.Closer, error) {
	call := m.recordCall("TargetLogStart")
	if call.retErr != nil {
		return nil, call.retErr
	}
	if call.retVal != nil {
		if closer, ok := call.retVal.(io.Closer); ok {
			return closer, nil
		}
		m.t.Fatalf("mockFFXClient: TargetLogStart expected io.Closer retVal, got %T", call.retVal)
	}
	return &mockTargetLogCloser{}, nil
}

func (m *mockFFXClient) Flash(ctx context.Context, fastbootSerial, productDir, pubKeyPath string) error {
	call := m.recordCall("Flash", fastbootSerial, productDir, pubKeyPath)
	return call.retErr
}

func (m *mockFFXClient) IsPackageServerRunning(ctx context.Context, repoName string) (bool, error) {
	call := m.recordCall("IsPackageServerRunning", repoName)
	if call.retErr != nil {
		return false, call.retErr
	}
	if call.retVal != nil {
		if b, ok := call.retVal.(bool); ok {
			return b, nil
		}
		m.t.Fatalf("mockFFXClient: IsPackageServerRunning expected bool retVal, got %T", call.retVal)
	}
	return true, nil
}

func (m *mockFFXClient) ProductDownload(ctx context.Context, transferURL, outDir, authPath string) error {
	call := m.recordCall("ProductDownload", transferURL, outDir, authPath)
	return call.retErr
}

func (m *mockFFXClient) EmuStart(ctx context.Context, productDir, name, engine, device string) error {
	call := m.recordCall("EmuStart", productDir, name, engine, device)
	return call.retErr
}

func (m *mockFFXClient) RepositoryCreate(ctx context.Context, repoDir string) error {
	call := m.recordCall("RepositoryCreate", repoDir)
	return call.retErr
}

func (m *mockFFXClient) RepositoryPublish(ctx context.Context, repoDir, productDir string, packageArchives []string) error {
	args := append([]string{repoDir, productDir}, packageArchives...)
	call := m.recordCall("RepositoryPublish", args...)
	return call.retErr
}

func (m *mockFFXClient) SymbolIndexAdd(ctx context.Context, buildID string) error {
	call := m.recordCall("SymbolIndexAdd", buildID)
	return call.retErr
}

func (m *mockFFXClient) RepositoryServerStart(ctx context.Context, repoName, repoDir, address string) error {
	call := m.recordCall("RepositoryServerStart", repoName, repoDir, address)
	return call.retErr
}

func (m *mockFFXClient) RepositoryServerStop(ctx context.Context, repoName string) error {
	call := m.recordCall("RepositoryServerStop", repoName)
	return call.retErr
}

func (m *mockFFXClient) RepositoryServerList(ctx context.Context) (string, error) {
	call := m.recordCall("RepositoryServerList")
	if call.retErr != nil {
		return "", call.retErr
	}
	if call.retVal != nil {
		if str, ok := call.retVal.(string); ok {
			return str, nil
		}
		m.t.Fatalf("mockFFXClient: RepositoryServerList expected string retVal, got %T", call.retVal)
	}
	return "", nil
}

func (m *mockFFXClient) TargetAdd(ctx context.Context, addr string) error {
	call := m.recordCall("TargetAdd", addr)
	return call.retErr
}

func (m *mockFFXClient) TargetList(ctx context.Context) (string, error) {
	call := m.recordCall("TargetList")
	if call.retErr != nil {
		return "", call.retErr
	}
	if call.retVal != nil {
		if str, ok := call.retVal.(string); ok {
			return str, nil
		}
		m.t.Fatalf("mockFFXClient: TargetList expected string retVal, got %T", call.retVal)
	}
	return "", nil
}

func (m *mockFFXClient) TargetWait(ctx context.Context) error {
	call := m.recordCall("TargetWait")
	return call.retErr
}

func (m *mockFFXClient) TargetShow(ctx context.Context) (string, error) {
	call := m.recordCall("TargetShow")
	if call.retErr != nil {
		return "", call.retErr
	}
	if call.retVal != nil {
		if str, ok := call.retVal.(string); ok {
			return str, nil
		}
		m.t.Fatalf("mockFFXClient: TargetShow expected string retVal, got %T", call.retVal)
	}
	return "", nil
}

func (m *mockFFXClient) TargetRepositoryRegister(ctx context.Context, repoName string, aliases []string) error {
	args := append([]string{repoName}, aliases...)
	call := m.recordCall("TargetRepositoryRegister", args...)
	return call.retErr
}

func (m *mockFFXClient) TargetSnapshot(ctx context.Context, dir string) error {
	call := m.recordCall("TargetSnapshot", dir)
	return call.retErr
}

func (m *mockFFXClient) Symbolize(ctx context.Context, input io.Reader, output io.Writer) error {
	call := m.recordCall("Symbolize")
	return call.retErr
}

// recordCall records a call and returns a predefined error if one exists.
func (m *mockFFXClient) recordCall(method string, args ...string) *mockCall {
	m.t.Helper()
	if m.callIdx >= len(m.calls) {
		m.t.Fatalf("unexpected call %s (args: %v)", method, args)
		return nil
	}

	expected := &m.calls[m.callIdx]
	if expected.method != method {
		m.t.Fatalf("expected call [%d] to %s, got %s", m.callIdx, expected.method, method)
	}

	if len(expected.args) > 0 || len(args) > 0 {
		if diff := cmp.Diff(expected.args, args); diff != "" {
			m.t.Fatalf("args mismatch for %s at call [%d] (-want +got):\n%s", method, m.callIdx, diff)
		}
	}

	m.callIdx++
	return expected
}

// expectCall adds an expected call to the mock.
func (m *mockFFXClient) expectCall(method string, args ...string) *mockFFXClient {
	m.calls = append(m.calls, mockCall{
		method: method,
		args:   args,
	})
	return m
}

// Returns sets the return value and error for the last expected call.
func (m *mockFFXClient) Returns(val any, err error) *mockFFXClient {
	m.t.Helper()
	if len(m.calls) == 0 {
		m.t.Fatal("Returns called without a preceding expectCall")
	}
	lastIdx := len(m.calls) - 1
	m.calls[lastIdx].retVal = val
	m.calls[lastIdx].retErr = err
	return m
}

// NewMockFFXClient creates a new mock for FFXClient.
func NewMockFFXClient(t *testing.T) *mockFFXClient {
	return &mockFFXClient{t: t}
}

// runOrchestratorScenario is a helper function to run a common orchestrator test scenario.
func runOrchestratorScenario(t *testing.T, isEmulator bool, runInput *RunInput, deviceConfig *DeviceConfig) {
	// Setup temporary directories for artifact paths
	tmpDir := t.TempDir()
	t.Setenv("TEST_UNDECLARED_OUTPUTS_DIR", tmpDir)
	// Also ensure FUCHSIA_PACKAGE_SERVER_PORT is set to something stable if used
	t.Setenv("FUCHSIA_PACKAGE_SERVER_PORT", "0")

	// Prepare a mock FFXClient
	mockFfx := NewMockFFXClient(t)

	// Create a fake executable for Cmd calls
	fakeFfx := filepath.Join(tmpDir, "fake_ffx")
	if err := os.WriteFile(fakeFfx, []byte("#!/bin/bash\nexit 0"), 0755); err != nil {
		t.Fatalf("failed to create fake ffx: %v", err)
	}
	mockFfx.fakeExecPath = fakeFfx

	// --- Dynamic values for mock expectations ---
	repoName := fmt.Sprintf("repo-%d", os.Getpid())
	var emuName string
	if isEmulator {
		emuName = fmt.Sprintf("fuchsia-emulator-%d", os.Getpid())
	}

	// --- Set RunInput's FfxPath to the fake executable ---
	if isEmulator {
		runInput.Emulator.FfxPath = mockFfx.fakeExecPath
	} else {
		runInput.Hardware.FfxPath = mockFfx.fakeExecPath
	}

	// --- Build Expected Calls for Mock FFX Client ---

	// Common FFX setup expectations
	mockFfx.expectCall("SetupFfx", repoName)

	wd, _ := os.Getwd()
	productBundleDir := ""
	targetRunInput := runInput.Target()
	if targetRunInput.TransferURL != "" {
		productBundleDir = filepath.Join(wd, "ffx-product-bundle")
		mockFfx.expectCall("ProductDownload", targetRunInput.TransferURL, productBundleDir, "")
	} else if targetRunInput.LocalPB != "" {
		productBundleDir = targetRunInput.LocalPB
		if !filepath.IsAbs(productBundleDir) {
			productBundleDir = filepath.Join(wd, productBundleDir)
		}
	}

	if isEmulator {
		// Emulator-specific expectations
		mockFfx.expectCall("EmuStart", productBundleDir, emuName, runInput.Emulator.Engine, runInput.Emulator.Device)
		mockFfx.expectCall("SetDefaultTarget", emuName) // Pass emuName as actual string, mock will check pointer value.
	} else {
		// Hardware-specific expectations
		mockFfx.expectCall("Flash", deviceConfig.FastbootSerial, productBundleDir, "")
	}

	// Package serving expectations (common)
	repoDir := filepath.Join(wd, "repo")
	mockFfx.expectCall("RepositoryCreate", repoDir)
	publishArgs := append([]string{repoDir, productBundleDir}, targetRunInput.PackageArchives...)
	mockFfx.expectCall("RepositoryPublish", publishArgs...)
	for _, buildID := range targetRunInput.BuildIds {
		mockFfx.expectCall("SymbolIndexAdd", buildID)
	}

	mockFfx.expectCall("RepositoryServerStart", repoName, repoDir, "[::]:0")
	mockFfx.expectCall("IsPackageServerRunning", repoName)
	mockFfx.expectCall("RepositoryServerList").Returns(`{"ok":{"data":[{"name":"mock-repo","address":"[::]:8080"}]}}`, nil)

	// Reach device expectations (conditional on deviceConfig presence and not emulator)
	if deviceConfig != nil && !isEmulator {
		mockFfx.expectCall("TargetAdd", deviceConfig.Network.IPv4)
	}
	if deviceConfig != nil {
		mockFfx.expectCall("SetDefaultTarget", deviceConfig.Network.IPv4)
	}
	mockFfx.expectCall("TargetList")
	mockFfx.expectCall("TargetWait")
	mockFfx.expectCall("TargetShow")
	mockFfx.expectCall("TargetLogStart")
	mockFfx.expectCall("TargetRepositoryRegister", repoName, "fuchsia.com", "chromium.org")

	// Test execution environment setup (ApplyEnv)
	mockFfx.expectCall("ApplyEnv")

	// Snapshot call
	mockFfx.expectCall("TargetSnapshot", tmpDir)

	// Cleanup calls (LIFO order due to defers)
	mockFfx.expectCall("Symbolize") // From stopFfxLog

	mockFfx.expectCall("RepositoryServerStop", repoName) // From stopPackageServer

	if isEmulator {
		mockFfx.expectCall("EmuStop") // From stopEmulator
	}

	mockFfx.expectCall("Close") // From ffx.Close

	// Create the orchestrator and inject mock
	orchestrator := NewTestOrchestrator(deviceConfig)
	orchestrator.ffx = mockFfx
	orchestrator.repoName = repoName // Ensure orchestrator uses the fixed repoName for mock consistency

	// Create a fake test command
	testCmdPath := filepath.Join(tmpDir, "test_cmd.sh")
	if err := os.WriteFile(testCmdPath, []byte("#!/bin/bash\necho 'mock test'"), 0755); err != nil {
		t.Fatalf("write test cmd: %v", err)
	}

	// Run the orchestrator
	err := orchestrator.Run(context.Background(), runInput, []string{testCmdPath})
	if err != nil {
		t.Errorf("orchestrator.Run failed: %v", err)
	}

	// Assert critical environment variables after ApplyEnv call occurred
	if mockFfx.recordedEnv == nil {
		t.Fatalf("mockFfx.recordedEnv is nil after ApplyEnv")
	}

	ffxIsolateDirFound := false
	for _, e := range mockFfx.recordedEnv {
		if strings.HasPrefix(e, "FFX_ISOLATE_DIR=") && strings.Contains(e, tmpDir) {
			ffxIsolateDirFound = true
		}
	}
	if !ffxIsolateDirFound {
		t.Errorf("FFX_ISOLATE_DIR not found or incorrect in mockFfx.recordedEnv: %v", mockFfx.recordedEnv)
	}

	// Check FUCHSIA_NODENAME for emulator scenario
	if isEmulator {
		nodenameFound := false
		expectedNodenameEnv := fmt.Sprintf("FUCHSIA_NODENAME=%s", emuName)
		for _, e := range mockFfx.recordedEnv {
			if e == expectedNodenameEnv {
				nodenameFound = true
				break
			}
		}
		if !nodenameFound {
			t.Errorf("FUCHSIA_NODENAME not found in env. Expected %q, got %v", expectedNodenameEnv, mockFfx.recordedEnv)
		}
	}

	// Verify all expected calls were made
	if mockFfx.callIdx != len(mockFfx.calls) {
		t.Errorf("Mock not fully exercised. Stopped at call %d. Next expected: %s", mockFfx.callIdx, mockFfx.calls[mockFfx.callIdx].method)
		// Print remaining expected calls for debugging
		for i := mockFfx.callIdx; i < len(mockFfx.calls); i++ {
			t.Logf("Remaining expected call [%d]: Method: %s, Args: %v", i, mockFfx.calls[i].method, mockFfx.calls[i].args)
		}
	}
}

func TestOrchestrator_Run_Unit(t *testing.T) {
	deviceConfig := &DeviceConfig{
		FastbootSerial: "serial123",
		Network:        DeviceNetworkConfig{IPv4: "192.168.1.10"},
	}
	runInput := &RunInput{
		Hardware: TargetRunInput{
			TransferURL:     "gs://bucket/product.json",
			PackageArchives: []string{"/tmp/pkg1.far"},
			BuildIds:        []string{"abc1234"},
		},
	}
	runOrchestratorScenario(t, false, runInput, deviceConfig)
}

func TestOrchestrator_Run_EmulatorUnit(t *testing.T) {
	runInput := &RunInput{
		Emulator: EmulatorRunInput{
			TargetRunInput: TargetRunInput{
				TransferURL: "gs://bucket/product.json",
				BuildIds:    []string{"abc1234"},
			},
		},
	}
	runOrchestratorScenario(t, true, runInput, nil)
}

func TestOrchestrator_Run_EmulatorLocalPB(t *testing.T) {
	runInput := &RunInput{
		Emulator: EmulatorRunInput{
			TargetRunInput: TargetRunInput{
				LocalPB:  "relative/path/to/pb",
				BuildIds: []string{"abc1234"},
			},
		},
	}
	runOrchestratorScenario(t, true, runInput, nil)
}

func TestOrchestrator_Run_HardwareLocalPB(t *testing.T) {
	deviceConfig := &DeviceConfig{
		FastbootSerial: "serial123",
		Network:        DeviceNetworkConfig{IPv4: "192.168.1.10"},
	}
	runInput := &RunInput{
		Hardware: TargetRunInput{
			LocalPB:         "relative/path/to/pb",
			PackageArchives: []string{"/tmp/pkg1.far"},
			BuildIds:        []string{"abc1234"},
		},
	}
	runOrchestratorScenario(t, false, runInput, deviceConfig)
}

// TestInstantiateFfx_PathResolution verifies that instantiateFfx correctly resolves ffx_path.
// Specifically, it ensures that:
// 1. Absolute paths are preserved as-is (not corrupted by joining with CWD).
// 2. Relative paths are correctly resolved to absolute paths relative to CWD.
//
// TODO: This test is only needed for the custom local ffx wrapper.
// Once orchestrate is migrated to use the ffxutil library (which handles absolute
// path resolution internally), this test can be safely deleted.
func TestInstantiateFfx_PathResolution(t *testing.T) {
	tmpDir := t.TempDir()
	t.Setenv("TEST_UNDECLARED_OUTPUTS_DIR", tmpDir)

	// Create a fake ffx executable shell script that exits 0.
	// ffx.New initialization runs 'config env set' command to set default configs,
	// so the ffxPath MUST point to a valid executable that returns 0 when run.
	fakeFfxName := "fake_ffx"
	fakeFfxPath := filepath.Join(tmpDir, fakeFfxName)
	if err := os.WriteFile(fakeFfxPath, []byte("#!/bin/bash\nexit 0"), 0755); err != nil {
		t.Fatalf("failed to create fake ffx: %v", err)
	}

	// Calculate a relative path from the current working directory to our fake ffx binary
	wd, err := os.Getwd()
	if err != nil {
		t.Fatalf("os.Getwd: %v", err)
	}
	relFfxPath, err := filepath.Rel(wd, fakeFfxPath)
	if err != nil {
		t.Fatalf("filepath.Rel: %v", err)
	}

	tests := []struct {
		name         string
		inputFfxPath string
		wantAbs      bool
		wantErr      bool
	}{
		{
			name:         "Absolute path remains absolute",
			inputFfxPath: fakeFfxPath, // This is absolute since tmpDir is absolute
			wantAbs:      true,
			wantErr:      false,
		},
		{
			name:         "Relative path becomes absolute",
			inputFfxPath: relFfxPath, // Use dynamically computed relative path
			wantAbs:      true,
			wantErr:      false,
		},
		{
			name:         "Empty path returns error",
			inputFfxPath: "",
			wantAbs:      false,
			wantErr:      true,
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {

			// Instantiate orchestrator without any pre-injected mock FFX client
			orchestrator := NewTestOrchestrator(nil)
			runInput := &RunInput{
				Emulator: EmulatorRunInput{
					TargetRunInput: TargetRunInput{
						FfxPath: tc.inputFfxPath,
					},
				},
			}

			// This will trigger the actual ffxPath resolution and call ffx.New()
			err = orchestrator.instantiateFfx(context.Background(), runInput)
			if (err != nil) != tc.wantErr {
				t.Fatalf("instantiateFfx returned err: %v, wantErr: %v", err, tc.wantErr)
			}
			if tc.wantErr {
				return
			}

			if orchestrator.ffx == nil {
				t.Fatalf("orchestrator.ffx is nil")
			}

			// Type-assert to the concrete *ffx.Ffx struct to access its Cmd method.
			realFfx, ok := orchestrator.ffx.(*ffx.Ffx)
			if !ok {
				t.Fatalf("expected orchestrator.ffx to be *ffx.Ffx, got %T", orchestrator.ffx)
			}
			cmd, err := realFfx.CmdContext(context.Background(), "version")
			if err != nil {
				t.Fatalf("ffx.Cmd failed: %v", err)
			}
			resolvedPath := cmd.Path
			if tc.wantAbs && !filepath.IsAbs(resolvedPath) {
				t.Errorf("expected absolute path, got: %s", resolvedPath)
			}

			// Verify that the resolved path actually exists on disk
			if _, err := os.Stat(resolvedPath); err != nil {
				t.Errorf("resolved path does not exist: %s, err: %v", resolvedPath, err)
			}
		})
	}
}
