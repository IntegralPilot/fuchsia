// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package ffx

import (
	"cmp"
	"context"
	"encoding/json"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"time"

	"go.fuchsia.dev/fuchsia/tools/lib/ffxutil"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

var _ FFXToolImpl = (*ffxStrict)(nil)

type ffxStrict struct {
	ffxToolPath         string
	runDir              RunDir
	supportsPackageBlob *bool
	ffxInstance         *ffxutil.FFXInstance
	hasPlaceholderKey   bool
	logLevel            string
}

func newFfxStrict(ctx context.Context, ffxToolPath string, runDir RunDir, subtoolsSearchPath string) (*ffxStrict, error) {
	if _, err := os.Stat(ffxToolPath); err != nil {
		return nil, fmt.Errorf("error accessing %v: %w", ffxToolPath, err)
	}

	outputDir := filepath.Join(runDir.path, "strict-output")
	if err := os.MkdirAll(outputDir, 0755); err != nil {
		return nil, fmt.Errorf("failed to create strict output dir: %w", err)
	}

	var sshInfo *ffxutil.SSHInfo
	privKey := runDir.PrivKey()
	if privKey == "" {
		privKey = os.Getenv("FUCHSIA_SSH_KEY")
	}

	hasPlaceholderKey := false
	if privKey != "" {
		sshInfo = &ffxutil.SSHInfo{
			SshPriv: privKey,
			SshPub:  privKey + ".pub",
		}
	} else {
		placeholderPrivPath := filepath.Join(outputDir, "placeholder-priv")
		err := os.WriteFile(placeholderPrivPath, []byte("placeholder"), 0600)
		if err != nil {
			os.RemoveAll(outputDir)
			return nil, err
		}
		sshInfo = &ffxutil.SSHInfo{
			SshPriv: placeholderPrivPath,
		}
		hasPlaceholderKey = true
	}

	resolvedSubtoolsSearchPath := resolveSubtoolsSearchPath(ffxToolPath, subtoolsSearchPath)

	extraConfigSettings := []ffxutil.ConfigSettings{
		{
			Level: "user",
			Settings: map[string]any{
				"ffx.subtool-search-paths": resolvedSubtoolsSearchPath,
			},
		},
	}

	ffxInst, err := ffxutil.NewFFXInstance(ctx, ffxToolPath, "", []string{}, "", sshInfo, outputDir, ffxutil.UseFFXStrict, extraConfigSettings...)
	if err != nil {
		os.RemoveAll(outputDir)
		return nil, err
	}

	return &ffxStrict{
		ffxToolPath:         ffxToolPath,
		runDir:              runDir,
		supportsPackageBlob: nil,
		ffxInstance:         ffxInst,
		hasPlaceholderKey:   hasPlaceholderKey,
		logLevel:            "trace",
	}, nil
}

func (f *ffxStrict) SetTarget(target string) {
	f.ffxInstance.SetTarget(target)
}

func (f *ffxStrict) SetLogLevel(level string) {
	f.logLevel = level
}

func (f *ffxStrict) TargetWait(ctx context.Context, target string) error {
	resolvedTarget, err := f.resolveTargetIfNeeded(ctx, target)
	if err != nil {
		return err
	}
	f.ffxInstance.SetTarget(resolvedTarget)
	return f.ffxInstance.TargetWait(ctx)
}

func (f *ffxStrict) ConfigSet(ctx context.Context, key, value string) error {
	return f.ffxInstance.ConfigSet(ctx, key, value)
}

func (f *ffxStrict) RunDir() RunDir {
	return f.runDir
}

func (f *ffxStrict) ClearRunDir() {
	os.RemoveAll(f.RunDir().path)
}

// resolveTargetIfNeeded resolves a target name to an IP address if it is not already a valid
// strict mode target specifier (e.g. IP address, serial, usb). This ensures that strict mode
// commands do not fail immediately when passed a node name.
func (f *ffxStrict) resolveTargetIfNeeded(ctx context.Context, target string) (string, error) {
	if target == "" {
		return "", nil
	}

	// Attempt to split host and port if present.
	host := target
	if h, _, err := net.SplitHostPort(target); err == nil {
		host = h
	}

	// Strip brackets and scope ID to validate if the target is an IP address.
	ipStr := strings.Trim(host, "[]")
	if idx := strings.Index(ipStr, "%"); idx != -1 {
		ipStr = ipStr[:idx]
	}

	// Check if target is a valid IP address, or a valid prefix.
	if net.ParseIP(ipStr) != nil || strings.HasPrefix(target, "usb:") || strings.HasPrefix(target, "vsock:") || strings.HasPrefix(target, "id:") {
		return target, nil
	}

	// Target looks like a node name. Resolve it using TargetList.
	entries, err := f.TargetList(ctx, target, 0)
	if err != nil {
		return "", fmt.Errorf("failed to resolve node name %q to an IP address: %w", target, err)
	}

	if len(entries) == 0 {
		return "", fmt.Errorf("no target found for node name %q", target)
	}

	for _, addr := range entries[0].Addresses {
		if addr.Type == "Ip" {
			return addr.IP, nil
		}
	}

	return "", fmt.Errorf("no IP address found for node name %q", target)
}

func (f *ffxStrict) TargetList(ctx context.Context, target string, timeout time.Duration) ([]TargetEntry, error) {
	args := []string{
		"--machine",
		"json",
		"target",
		"list",
	}

	if timeout > 0 {
		args = append([]string{"-c", fmt.Sprintf("discovery.timeout=%d", timeout.Milliseconds())}, args...)
	}

	if target != "" {
		args = append(args, target)
	}

	stdout, err := f.runFFXCmd(ctx, args...)
	if err != nil {
		return []TargetEntry{}, fmt.Errorf("ffx target list failed: %w", err)
	}

	if len(stdout) == 0 {
		return []TargetEntry{}, nil
	}

	var entries []TargetEntry
	if err := json.Unmarshal(stdout, &entries); err != nil {
		return []TargetEntry{}, err
	}

	return entries, nil
}

func (f *ffxStrict) GetDisambiguatedTarget(ctx context.Context) (TargetEntry, error) {
	targets, err := f.TargetList(ctx, "", 0)
	if err != nil {
		return TargetEntry{}, err
	}

	if len(targets) == 1 {
		return targets[0], nil
	}

	for _, v := range targets {
		if v.IsDefault {
			return v, nil
		}
	}

	if len(targets) == 0 {
		return TargetEntry{}, fmt.Errorf("no targets found")
	}

	return slices.MinFunc(targets, func(a, b TargetEntry) int {
		return cmp.Compare(a.NodeName, b.NodeName)
	}), nil
}

func (f *ffxStrict) TargetListForNode(ctx context.Context, nodeName string) ([]TargetEntry, error) {
	entries, err := f.TargetList(ctx, nodeName, 0)
	if err != nil {
		return []TargetEntry{}, err
	}

	var matchingTargets []TargetEntry

	for _, target := range entries {
		if target.NodeName == nodeName {
			matchingTargets = append(matchingTargets, target)
		}
	}

	return matchingTargets, nil
}

func (f *ffxStrict) WaitForTarget(ctx context.Context, address string) (TargetEntry, error) {
	for attempt := 0; attempt < 10; attempt++ {
		entries, err := f.TargetList(ctx, "", 0)
		if err != nil {
			return TargetEntry{}, fmt.Errorf("failed to get target list: %w", err)
		}

		for _, target := range entries {
			for _, addr := range target.Addresses {
				if addr.Type == "Ip" && addr.IP == address {
					return target, nil
				}
			}
		}
		time.Sleep(5 * time.Second)
	}

	return TargetEntry{}, fmt.Errorf("no target found for address %v", address)
}

func (f *ffxStrict) SupportsZedbootDiscovery(ctx context.Context) (bool, error) {
	args := []string{
		"config",
		"get",
		"discovery.zedboot.enabled",
	}

	stdout, err := f.runFFXCmd(ctx, args...)
	if err != nil {
		if exiterr, ok := err.(*exec.ExitError); ok {
			if exiterr.ExitCode() == 2 {
				return false, nil
			}
		}

		return false, fmt.Errorf("ffx config get failed: %w", err)
	}

	if string(stdout) == "true\n" {
		return true, nil
	}

	return false, nil
}

func (f *ffxStrict) TargetGetSshTime(ctx context.Context, target string) (time.Duration, error) {
	resolvedTarget, err := f.resolveTargetIfNeeded(ctx, target)
	if err != nil {
		return 0, err
	}

	args := []string{
		"--target",
		resolvedTarget,
		"target",
		"get-time",
	}

	t0 := time.Now()
	stdout, err := f.runFFXCmd(ctx, args...)
	t1 := time.Now()

	if err != nil {
		return 0, fmt.Errorf("ffx target get-time failed: %w", err)
	}

	t, err := strconv.Atoi(strings.TrimSpace(string(stdout)))
	if err != nil {
		return 0, fmt.Errorf("failed to parse ffx target-get-time output: %w", err)
	}

	latency := t1.Sub(t0) / 2
	monotonicTime := (time.Duration(t) * time.Nanosecond) - latency

	return monotonicTime, nil
}

func (f *ffxStrict) TargetUpdateChannelSet(ctx context.Context, target string, channel string) error {
	resolvedTarget, err := f.resolveTargetIfNeeded(ctx, target)
	if err != nil {
		return err
	}

	args := []string{
		"--machine",
		"raw",
		"--target",
		resolvedTarget,
		"target",
		"update",
		"channel",
		"set",
		channel,
	}

	_, err = f.runFFXCmd(ctx, args...)
	return err
}

func (f *ffxStrict) TargetUpdateCheckNowMonitor(ctx context.Context, target string) ([]byte, error) {
	resolvedTarget, err := f.resolveTargetIfNeeded(ctx, target)
	if err != nil {
		return nil, err
	}

	args := []string{
		"--target",
		resolvedTarget,
		"target",
		"update",
		"check-now",
		"--monitor",
	}

	stdout, err := f.ffxInstance.RunAndGetOutputRaw(ctx, args...)
	return []byte(stdout), err
}

func (f *ffxStrict) TargetUpdateForceInstallNoReboot(ctx context.Context, target string, url string) error {
	resolvedTarget, err := f.resolveTargetIfNeeded(ctx, target)
	if err != nil {
		return err
	}

	args := []string{
		"--target",
		resolvedTarget,
		"target",
		"update",
		"force-install",
		url,
		"--reboot",
		"false",
	}

	_, err = f.runFFXCmd(ctx, args...)
	return err
}

func (f *ffxStrict) RebootToBootloader(ctx context.Context, target string) error {
	resolvedTarget, err := f.resolveTargetIfNeeded(ctx, target)
	if err != nil {
		return err
	}
	_, err = f.runFFXCmd(ctx, "--target", resolvedTarget, "target", "reboot", "-b")
	return err
}

func (f *ffxStrict) Flasher() *Flasher {
	return newFlasher(f)
}

func containsSequence(args []string, seq []string) bool {
	for i := 0; i <= len(args)-len(seq); i++ {
		match := true
		for j := 0; j < len(seq); j++ {
			if args[i+j] != seq[j] {
				match = false
				break
			}
		}
		if match {
			return true
		}
	}
	return false
}

func isTargetAgnostic(args []string) bool {
	if containsSequence(args, []string{"config"}) {
		return true
	}
	if containsSequence(args, []string{"package", "blob", "decompress"}) {
		return true
	}
	if containsSequence(args, []string{"repository", "create"}) {
		return true
	}
	if containsSequence(args, []string{"repository", "publish"}) {
		return true
	}
	if containsSequence(args, []string{"target", "list"}) {
		return true
	}
	if containsSequence(args, []string{"target", "discover"}) {
		return true
	}
	if containsSequence(args, []string{"target", "flash"}) {
		return true
	}
	if containsSequence(args, []string{"target", "fastboot"}) {
		return true
	}
	if containsSequence(args, []string{"emulator"}) {
		return true
	}
	if containsSequence(args, []string{"monitor"}) {
		return true
	}

	return false
}

func (f *ffxStrict) runFFXCmd(ctx context.Context, args ...string) ([]byte, error) {
	path, err := exec.LookPath(f.ffxToolPath)
	if err != nil {
		return []byte{}, err
	}

	if f.hasPlaceholderKey && !isTargetAgnostic(args) {
		return nil, fmt.Errorf("cannot run target-interacting command without a valid SSH key (args: %v)", args)
	}

	// Add default flags to match daemon implementation
	args = append([]string{"--log-level", f.logLevel}, args...)
	logger.Infof(ctx, "running with strict ffx: %s %v", path, args)
	stdoutStr, err := f.ffxInstance.RunAndGetOutput(ctx, args...)
	stdout := []byte(stdoutStr)

	if err == nil {
		logger.Infof(ctx, "finished running with strict ffx %s %q", path, args)
	} else {
		logger.Infof(ctx, "running with strict ffx %s %q failed with: %v", path, args, err)
	}
	return stdout, err
}

func (f *ffxStrict) RunAndGetOutput(ctx context.Context, args ...string) (string, error) {
	stdout, err := f.runFFXCmd(ctx, args...)
	return string(stdout), err
}

func (f *ffxStrict) Run(ctx context.Context, args ...string) error {
	_, err := f.RunAndGetOutput(ctx, args...)
	return err
}

func (f *ffxStrict) RepositoryCreate(ctx context.Context, repoDir, keysDir string) error {
	args := []string{
		"--config", "ffx_repository=true",
		"repository",
		"create",
		"--keys", keysDir,
		repoDir,
	}

	_, err := f.runFFXCmd(ctx, args...)
	return err
}

func (f *ffxStrict) RepositoryPublish(ctx context.Context, repoDir string, packageManifests []string, additionalArgs ...string) error {
	args := []string{
		"repository",
		"publish",
	}

	for _, manifest := range packageManifests {
		args = append(args, "--package", manifest)
	}

	args = append(args, additionalArgs...)
	args = append(args, repoDir)

	_, err := f.runFFXCmd(ctx, args...)
	return err
}

func (f *ffxStrict) SupportsPackageBlob(ctx context.Context) bool {
	if f.supportsPackageBlob == nil {
		_, err := f.runFFXCmd(ctx, "package", "blob", "--help")
		supportsPackageBlob := err == nil
		f.supportsPackageBlob = &supportsPackageBlob
	}
	return *f.supportsPackageBlob
}

func (f *ffxStrict) DecompressBlobs(ctx context.Context, delivery_blobs []string, out_dir string) error {
	args := []string{
		"package",
		"blob",
		"decompress",
		"--output", out_dir,
	}

	args = append(args, delivery_blobs...)

	_, err := f.runFFXCmd(ctx, args...)
	return err
}

func (f *ffxStrict) RegisterPackageRepository(ctx context.Context, target string, repo_url string) error {
	resolvedTarget, err := f.resolveTargetIfNeeded(ctx, target)
	if err != nil {
		return err
	}

	args := []string{
		"target",
		"repository",
		"register",
		"--json-uri",
		repo_url,
	}

	if resolvedTarget != "" {
		args = append([]string{"--target", resolvedTarget}, args...)
	}

	_, err = f.runFFXCmd(ctx, args...)
	return err
}

func (f *ffxStrict) TargetGetLastRebootReason(ctx context.Context, target string) (string, error) {
	resolvedTarget, err := f.resolveTargetIfNeeded(ctx, target)
	if err != nil {
		return "", err
	}

	logger.Infof(ctx, "getting last reboot reason for target %s", resolvedTarget)
	f.ffxInstance.SetTarget(resolvedTarget)
	return f.ffxInstance.GetLastRebootReason(ctx)
}

func (f *ffxStrict) GetTarget() string {
	return f.ffxInstance.GetTarget()
}

func (f *ffxStrict) Close(ctx context.Context) error {
	return nil
}

// EnsureOutputDirsExist ensures that the output directory for strict mode logs exists.
// This is needed if the run directory was cleared by ClearRunDir, as strict mode
// requires the log directory to exist to start.
func (f *ffxStrict) EnsureOutputDirsExist(ctx context.Context) error {
	outputDir := filepath.Join(f.runDir.path, "strict-output")
	if err := os.MkdirAll(outputDir, 0755); err != nil {
		return fmt.Errorf("failed to create strict output dir: %w", err)
	}
	return nil
}
