// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package ffx

import (
	"context"
	"encoding/json"
	"os/exec"
	"path/filepath"
	"time"

	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

// FfxVersionPolicy specifies whether to use the latest ffx or infer the version from the target's API level.
type FfxVersionPolicy string

const (
	FfxVersionPolicyLatest       FfxVersionPolicy = "latest"
	FfxVersionPolicyFromApiLevel FfxVersionPolicy = "fromApiLevel"

	// strictModeApiLevelThreshold is the API level above which ffx strict mode is supported.
	strictModeApiLevelThreshold = 31
)

// RunDir represents the execution directory for ffx.
type RunDir struct {
	path    string
	privKey string
}

func NewRunDir(path string) RunDir {
	return RunDir{path: path}
}

func NewRunDirWithPrivKey(path string, privKey string) RunDir {
	return RunDir{path: path, privKey: privKey}
}

func (d RunDir) PrivKey() string {
	return d.privKey
}

// TargetAddress represents a single address for a target.
// It corresponds to ffx's enum JsonTargetAddress.
type TargetAddress struct {
	Type    string `json:"type"`
	IP      string `json:"ip,omitempty"`       // for Type == "Ip"
	SSHPort uint16 `json:"ssh_port,omitempty"` // for Type == "Ip"
	CID     uint32 `json:"cid,omitempty"`      // for Type == "VSock" or "Usb"
}

// TargetEntry represents a single Fuchsia device/emulator.
// It corresponds to the Rust struct JsonTarget.
type TargetEntry struct {
	NodeName    string          `json:"nodename"`
	RCSState    string          `json:"rcs_state"` // "Y" or "N"
	Serial      string          `json:"serial"`
	TargetType  string          `json:"target_type"`  // board/product, like "core.x64" or "Unknown"
	TargetState string          `json:"target_state"` // e.g., "Product", "Fastboot", "Zedboot"
	Addresses   []TargetAddress `json:"addresses"`
	IsDefault   bool            `json:"is_default"`
	IsManual    bool            `json:"is_manual"`
}

// FFXToolImpl is the interface that abstracts the operations performed by the ffx tool implementations.
type FFXToolImpl interface {
	runFFXCmd(ctx context.Context, args ...string) ([]byte, error)
	TargetList(ctx context.Context, target string, timeout time.Duration) ([]TargetEntry, error)
	GetDisambiguatedTarget(ctx context.Context) (TargetEntry, error)
	TargetListForNode(ctx context.Context, nodeName string) ([]TargetEntry, error)
	WaitForTarget(ctx context.Context, address string) (TargetEntry, error)

	SupportsZedbootDiscovery(ctx context.Context) (bool, error)

	TargetGetSshTime(ctx context.Context, target string) (time.Duration, error)
	TargetUpdateChannelSet(ctx context.Context, target string, channel string) error
	TargetUpdateCheckNowMonitor(ctx context.Context, target string) ([]byte, error)
	TargetUpdateForceInstallNoReboot(ctx context.Context, target string, url string) error
	Flasher() *Flasher
	RepositoryCreate(ctx context.Context, repoDir, keysDir string) error
	RepositoryPublish(ctx context.Context, repoDir string, packageManifests []string, additionalArgs ...string) error
	SupportsPackageBlob(ctx context.Context) bool
	DecompressBlobs(ctx context.Context, deliveryBlobs []string, outDir string) error
	RegisterPackageRepository(ctx context.Context, target string, repoURL string) error
	TargetGetLastRebootReason(ctx context.Context, target string) (string, error)
	Close(ctx context.Context) error
	RunDir() RunDir
	Run(ctx context.Context, args ...string) error
	RunAndGetOutput(ctx context.Context, args ...string) (string, error)
	ClearRunDir()
	SetTarget(target string)
	GetTarget() string
	TargetWait(ctx context.Context, target string) error
	RebootToBootloader(ctx context.Context, target string) error
	EnsureOutputDirsExist(ctx context.Context) error
	SetLogLevel(level string)
}

var _ FFXToolImpl = (*FFXTool)(nil)

// FFXTool is a concrete object that contains the implementation.
type FFXTool struct {
	version      FfxVersionPolicy
	buildVersion string
	impl         FFXToolImpl
}

func NewFFXToolForVersion(ctx context.Context, ffxPath string, runDir RunDir, versionPolicy FfxVersionPolicy, subtoolsSearchPath string) (*FFXTool, error) {
	logger.Infof(ctx, "NewFFXToolForVersion called with version policy: %q", versionPolicy)

	// Query the build version and API level of the ffx binary.
	// Fallback to "unknown" if it fails (robustness).
	buildVersion := "unknown (likely old)"
	apiLevel := 0

	cmd := exec.CommandContext(ctx, ffxPath, "--machine", "json", "version", "-v")
	output, errRun := cmd.Output()
	if errRun != nil {
		logger.Infof(ctx, "Failed to query ffx version (expected on old binaries): %v", errRun)
	} else {
		var versionInfo struct {
			ToolVersion struct {
				ApiLevel     int    `json:"api_level"`
				BuildVersion string `json:"build_version"`
			} `json:"tool_version"`
		}
		if errJSON := json.Unmarshal(output, &versionInfo); errJSON != nil {
			logger.Infof(ctx, "Failed to parse ffx version JSON: %v", errJSON)
		} else {
			apiLevel = versionInfo.ToolVersion.ApiLevel
			buildVersion = versionInfo.ToolVersion.BuildVersion
			logger.Infof(ctx, "Detected ffx API level: %d, build version: %q", apiLevel, buildVersion)
		}
	}

	var impl FFXToolImpl
	var err error
	if versionPolicy == FfxVersionPolicyLatest {
		logger.Infof(ctx, "Using strict mode directly for version policy: latest")
		impl, err = newFfxStrict(ctx, ffxPath, runDir, subtoolsSearchPath)
	} else if versionPolicy == FfxVersionPolicyFromApiLevel {
		if apiLevel > strictModeApiLevelThreshold {
			logger.Infof(ctx, "API Level > %d, using strict mode", strictModeApiLevelThreshold)
			impl, err = newFfxStrict(ctx, ffxPath, runDir, subtoolsSearchPath)
		} else {
			logger.Infof(ctx, "API Level <= %d, using daemon mode", strictModeApiLevelThreshold)
			impl, err = newFfxDaemon(ctx, ffxPath, runDir, subtoolsSearchPath)
		}
	} else {
		logger.Infof(ctx, "Falling back to daemon mode for version policy: %q", versionPolicy)
		impl, err = newFfxDaemon(ctx, ffxPath, runDir, subtoolsSearchPath)
	}
	if err != nil {
		return nil, err
	}

	return &FFXTool{
		version:      versionPolicy,
		buildVersion: buildVersion,
		impl:         impl,
	}, nil
}

func NewFFXTool(ffxPath string, runDir RunDir) (*FFXTool, error) {
	return NewFFXToolForVersion(context.Background(), ffxPath, runDir, FfxVersionPolicyLatest, "")
}

func (t *FFXTool) runFFXCmd(ctx context.Context, args ...string) ([]byte, error) {
	return t.impl.runFFXCmd(ctx, args...)
}

func (t *FFXTool) TargetList(ctx context.Context, target string, timeout time.Duration) ([]TargetEntry, error) {
	return t.impl.TargetList(ctx, target, timeout)
}

// GetDisambiguatedTarget is like TargetList, but returns exactly one target, enforcing the
// following rules:
// 1. Return the target if only one is found.
// 2. Return the default target if it is set.
// 3. Return the first target in the list if multiple targets are found, sorted by target name.
func (t *FFXTool) GetDisambiguatedTarget(ctx context.Context) (TargetEntry, error) {
	return t.impl.GetDisambiguatedTarget(ctx)
}

func (t *FFXTool) TargetListForNode(ctx context.Context, nodeName string) ([]TargetEntry, error) {
	return t.impl.TargetListForNode(ctx, nodeName)
}

func (t *FFXTool) WaitForTarget(ctx context.Context, address string) (TargetEntry, error) {
	return t.impl.WaitForTarget(ctx, address)
}

func (t *FFXTool) SupportsZedbootDiscovery(ctx context.Context) (bool, error) {
	return t.impl.SupportsZedbootDiscovery(ctx)
}

func (t *FFXTool) TargetGetSshTime(ctx context.Context, target string) (time.Duration, error) {
	return t.impl.TargetGetSshTime(ctx, target)
}

func (t *FFXTool) TargetUpdateChannelSet(ctx context.Context, target string, channel string) error {
	return t.impl.TargetUpdateChannelSet(ctx, target, channel)
}

func (t *FFXTool) TargetUpdateCheckNowMonitor(ctx context.Context, target string) ([]byte, error) {
	return t.impl.TargetUpdateCheckNowMonitor(ctx, target)
}

func (t *FFXTool) TargetUpdateForceInstallNoReboot(ctx context.Context, target string, url string) error {
	return t.impl.TargetUpdateForceInstallNoReboot(ctx, target, url)
}

func (t *FFXTool) Flasher() *Flasher {
	return t.impl.Flasher()
}

func (t *FFXTool) RepositoryCreate(ctx context.Context, repoDir, keysDir string) error {
	return t.impl.RepositoryCreate(ctx, repoDir, keysDir)
}

func (t *FFXTool) RepositoryPublish(ctx context.Context, repoDir string, packageManifests []string, additionalArgs ...string) error {
	return t.impl.RepositoryPublish(ctx, repoDir, packageManifests, additionalArgs...)
}

func (t *FFXTool) SupportsPackageBlob(ctx context.Context) bool {
	return t.impl.SupportsPackageBlob(ctx)
}

func (t *FFXTool) DecompressBlobs(ctx context.Context, delivery_blobs []string, out_dir string) error {
	return t.impl.DecompressBlobs(ctx, delivery_blobs, out_dir)
}

func (t *FFXTool) RegisterPackageRepository(ctx context.Context, target string, repo_url string) error {
	return t.impl.RegisterPackageRepository(ctx, target, repo_url)
}

func (t *FFXTool) TargetGetLastRebootReason(ctx context.Context, target string) (string, error) {
	return t.impl.TargetGetLastRebootReason(ctx, target)
}

func (t *FFXTool) Close(ctx context.Context) error {
	return t.impl.Close(ctx)
}

func (t *FFXTool) EnsureOutputDirsExist(ctx context.Context) error {
	return t.impl.EnsureOutputDirsExist(ctx)
}

func (t *FFXTool) RunDir() RunDir {
	return t.impl.RunDir()
}

func (t *FFXTool) Run(ctx context.Context, args ...string) error {
	return t.impl.Run(ctx, args...)
}

func (t *FFXTool) RunAndGetOutput(ctx context.Context, args ...string) (string, error) {
	return t.impl.RunAndGetOutput(ctx, args...)
}

func (t *FFXTool) ClearRunDir() {
	t.impl.ClearRunDir()
}

func (t *FFXTool) SetTarget(target string) {
	t.impl.SetTarget(target)
}

func (t *FFXTool) GetTarget() string {
	return t.impl.GetTarget()
}

func (t *FFXTool) TargetWait(ctx context.Context, target string) error {
	return t.impl.TargetWait(ctx, target)
}

func (t *FFXTool) RebootToBootloader(ctx context.Context, target string) error {
	return t.impl.RebootToBootloader(ctx, target)
}

func (t *FFXTool) SetLogLevel(level string) {
	t.impl.SetLogLevel(level)
}

// resolveSubtoolsSearchPath returns the provided subtoolsSearchPath if it is not empty.
// Otherwise, it resolves the absolute directory of the ffxToolPath and returns it,
// defaulting the subtool search path to the directory containing the ffx binary.
func resolveSubtoolsSearchPath(ffxToolPath string, subtoolsSearchPath string) string {
	if subtoolsSearchPath != "" {
		return subtoolsSearchPath
	}
	path := ffxToolPath
	if p, err := exec.LookPath(ffxToolPath); err == nil {
		path = p
	}
	if absPath, err := filepath.Abs(path); err == nil {
		path = absPath
	}
	return filepath.Dir(path)
}
