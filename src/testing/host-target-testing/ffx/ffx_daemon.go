// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package ffx

import (
	"bytes"
	"cmp"
	"context"
	"encoding/json"
	"fmt"
	"net"
	"os"
	"os/exec"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

// TODO(b/540443180): Clean up ffxDaemon naming and ffx_daemon.go file in a follow-up CL now that daemon is removed.
var _ FFXToolImpl = (*ffxDaemon)(nil)

type ffxDaemon struct {
	ffxToolPath         string
	runDir              RunDir
	supportsPackageBlob *bool
	supportsDirect      bool
	target              string
	subtoolsSearchPath  string
	logLevel            string
}

var directSupportCache sync.Map

func newFfxDaemon(ctx context.Context, ffxToolPath string, runDir RunDir, subtoolsSearchPath string) (*ffxDaemon, error) {
	if _, err := os.Stat(ffxToolPath); err != nil {
		return nil, fmt.Errorf("error accessing %v: %w", ffxToolPath, err)
	}

	// Check if the ffx binary supports the --direct flag, which allows bypassing
	// the daemon for certain commands to avoid sync issues in tests that frequently
	// reboot the target.
	var supportsDirect bool
	if val, ok := directSupportCache.Load(ffxToolPath); ok {
		supportsDirect = val.(bool)
	} else {
		supportsDirect = false
		cmd := exec.CommandContext(ctx, ffxToolPath, "--help")
		output, err := cmd.Output()
		if err == nil {
			if strings.Contains(string(output), "--direct") {
				supportsDirect = true
			}
		} else {
			logger.Warningf(ctx, "failed to run ffx --help to check for --direct: %v", err)
		}
		directSupportCache.Store(ffxToolPath, supportsDirect)
	}

	resolvedSubtoolsSearchPath := resolveSubtoolsSearchPath(ffxToolPath, subtoolsSearchPath)

	return &ffxDaemon{
		ffxToolPath:         ffxToolPath,
		runDir:              runDir,
		supportsPackageBlob: nil,
		supportsDirect:      supportsDirect,
		subtoolsSearchPath:  resolvedSubtoolsSearchPath,
	}, nil
}

func (f *ffxDaemon) RunDir() RunDir {
	return f.runDir
}

func (f *ffxDaemon) ClearRunDir() {
	os.RemoveAll(f.RunDir().path)
}

func (f *ffxDaemon) appendDirectFlag(args []string) []string {
	if f.supportsDirect {
		return append(args, "--direct")
	}
	return args
}

func (f *ffxDaemon) SetTarget(target string) {
	f.target = target
}

func (f *ffxDaemon) SetLogLevel(level string) {
	f.logLevel = level
}

func (f *ffxDaemon) GetTarget() string {
	return f.target
}

func matchTarget(entry TargetEntry, target string) bool {
	if target == "" {
		return true
	}
	if entry.NodeName == target || entry.NodeName == "" || entry.NodeName == "<unnamed>" || entry.Serial == target {
		return true
	}
	for _, addr := range entry.Addresses {
		if addr.IP == target {
			return true
		}
		if addr.SSHPort != 0 {
			if net.JoinHostPort(addr.IP, strconv.Itoa(int(addr.SSHPort))) == target {
				return true
			}
		}
	}
	return false
}

func (f *ffxDaemon) TargetWait(ctx context.Context, target string) error {
	if target == "" {
		target = f.target
	}
	for i := 0; i < 10; i++ {
		entries, err := f.TargetList(ctx, target, 0)
		if err != nil {
			return fmt.Errorf("failed to list devices: %w", err)
		}

		if len(entries) > 0 {
			if target == "" {
				return nil
			}
			for _, entry := range entries {
				if matchTarget(entry, target) {
					return nil
				}
			}
		}
		time.Sleep(5 * time.Second)
	}
	return fmt.Errorf("timed out waiting for target %q", target)
}

func (f *ffxDaemon) RebootToBootloader(ctx context.Context, target string) error {
	_, err := f.runFFXCmd(ctx, "--target", target, "target", "reboot", "-b")
	return err
}

func (f *ffxDaemon) Close(ctx context.Context) error {
	// TODO(https://fxbug.dev/415899721): We put a time
	// limit because the command fails when run inside an nsjail.
	// Remove when bug is fixed.
	args := []string{"daemon", "stop", "-t", "4000"}
	_, err := f.runFFXCmd(ctx, args...)
	return err
}

// EnsureOutputDirsExist ensures that the isolate directory exists.
func (f *ffxDaemon) EnsureOutputDirsExist(ctx context.Context) error {
	if err := os.MkdirAll(f.runDir.path, 0755); err != nil {
		return fmt.Errorf("failed to create isolate dir: %w", err)
	}
	return nil
}

func (f *ffxDaemon) TargetList(ctx context.Context, target string, timeout time.Duration) ([]TargetEntry, error) {
	args := f.appendDirectFlag([]string{})
	args = append(args, "--machine", "json", "target", "list")

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

func (f *ffxDaemon) GetDisambiguatedTarget(ctx context.Context) (TargetEntry, error) {
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

	return slices.MinFunc(targets, func(a, b TargetEntry) int {
		return cmp.Compare(a.NodeName, b.NodeName)
	}), nil
}

func (f *ffxDaemon) TargetListForNode(ctx context.Context, nodeName string) ([]TargetEntry, error) {
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

func (f *ffxDaemon) WaitForTarget(ctx context.Context, address string) (TargetEntry, error) {
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

func (f *ffxDaemon) TargetGetSshAddress(ctx context.Context, target string) (string, error) {
	args := f.appendDirectFlag([]string{})
	args = append(args, "--target", target, "target", "list", "--format", "addresses", "--no-probe", "--no-usb")

	stdout, err := f.runFFXCmd(ctx, args...)
	if err != nil {
		return "", fmt.Errorf("ffx target list --format addresses failed: %w", err)
	}

	return strings.TrimSpace(string(stdout)), nil
}

func (f *ffxDaemon) SupportsZedbootDiscovery(ctx context.Context) (bool, error) {
	// Check if ffx is configured to resolve devices in zedboot.
	args := []string{
		"config",
		"get",
		"discovery.zedboot.enabled",
	}

	stdout, err := f.runFFXCmd(ctx, args...)
	if err != nil {
		// `ffx config get` exits with 2 if variable is undefined.
		if exiterr, ok := err.(*exec.ExitError); ok {
			if exiterr.ExitCode() == 2 {
				return false, nil
			}
		}

		return false, fmt.Errorf("ffx config get failed: %w", err)
	}

	// FIXME(https://fxbug.dev/42060660): Unfortunately we need to parse the raw string to see if it's true.
	if string(stdout) == "true\n" {
		return true, nil
	}

	return false, nil
}

func (f *ffxDaemon) TargetGetSshTime(ctx context.Context, target string) (time.Duration, error) {
	args := []string{
		"--target",
		target,
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

	// Estimate the latency as half the time to execute the command.
	latency := t1.Sub(t0) / 2

	// The output is in nanoseconds.
	monotonicTime := (time.Duration(t) * time.Nanosecond) - latency

	return monotonicTime, nil
}

func (f *ffxDaemon) TargetUpdateChannelSet(ctx context.Context, target string, channel string) error {
	args := []string{
		"--target",
		target,
		"target",
		"update",
		"channel",
		"set",
		channel,
	}

	_, err := f.runFFXCmd(ctx, args...)
	return err
}

func (f *ffxDaemon) TargetUpdateCheckNowMonitor(ctx context.Context, target string) ([]byte, error) {
	args := []string{
		"--target",
		target,
		"target",
		"update",
		"check-now",
		"--monitor",
	}

	return f.runFFXCmd(ctx, args...)
}

func (f *ffxDaemon) TargetUpdateForceInstallNoReboot(ctx context.Context, target string, url string) error {
	args := f.appendDirectFlag([]string{})
	args = append(args, "--target", target, "target", "update", "force-install", url, "--reboot", "false")

	_, err := f.runFFXCmd(ctx, args...)
	return err
}

func (f *ffxDaemon) Flasher() *Flasher {
	return newFlasher(f)
}

func (f *ffxDaemon) runFFXCmd(ctx context.Context, args ...string) ([]byte, error) {
	path, err := exec.LookPath(f.ffxToolPath)
	if err != nil {
		return []byte{}, err
	}

	// prepend a config flag for finding subtools that are compiled separately
	// in the same directory as ffx itself.
	var defaultFlags []string
	if f.logLevel != "" {
		defaultFlags = append(defaultFlags, "--log-level", f.logLevel)
	}
	defaultFlags = append(
		defaultFlags,
		"--isolate-dir", f.runDir.path,
		"--config", fmt.Sprintf("ffx.subtool-search-paths=%s", f.subtoolsSearchPath),
	)
	args = append(defaultFlags, args...)

	logger.Infof(ctx, "running: %s %q", path, args)
	cmd := exec.CommandContext(ctx, path, args...)
	var stdoutBuf bytes.Buffer
	cmd.Stdout = &stdoutBuf
	cmd.Stderr = os.Stderr

	cmdRet := cmd.Run()

	stdout := stdoutBuf.Bytes()
	if len(stdout) != 0 {
		logger.Infof(ctx, "%s", string(stdout))
	}

	if cmdRet == nil {
		logger.Infof(ctx, "finished running %s %q", path, args)
	} else {
		logger.Infof(ctx, "running %s %q failed with: %v", path, args, cmdRet)
	}
	return stdout, cmdRet
}

func (f *ffxDaemon) RunAndGetOutput(ctx context.Context, args ...string) (string, error) {
	stdout, err := f.runFFXCmd(ctx, args...)
	return string(stdout), err
}

func (f *ffxDaemon) Run(ctx context.Context, args ...string) error {
	_, err := f.RunAndGetOutput(ctx, args...)
	return err
}

func (f *ffxDaemon) RepositoryCreate(ctx context.Context, repoDir, keysDir string) error {
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

func (f *ffxDaemon) RepositoryPublish(ctx context.Context, repoDir string, packageManifests []string, additionalArgs ...string) error {
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

func (f *ffxDaemon) SupportsPackageBlob(ctx context.Context) bool {
	if f.supportsPackageBlob == nil {
		_, err := f.runFFXCmd(ctx, "package", "blob", "--help")
		supportsPackageBlob := err == nil
		f.supportsPackageBlob = &supportsPackageBlob
	}
	return *f.supportsPackageBlob
}

func (f *ffxDaemon) DecompressBlobs(ctx context.Context, deliveryBlobs []string, outDir string) error {
	args := []string{
		"package",
		"blob",
		"decompress",
		"--output", outDir,
	}

	args = append(args, deliveryBlobs...)

	_, err := f.runFFXCmd(ctx, args...)
	return err
}

func (f *ffxDaemon) RegisterPackageRepository(ctx context.Context, target string, repo_url string) error {
	args := []string{"target", "repository", "register", "--json-uri", repo_url}
	if target != "" {
		args = append([]string{"--target", target}, args...)
	}
	_, err := f.runFFXCmd(ctx, args...)
	return err
}

func (f *ffxDaemon) TargetGetLastRebootReason(ctx context.Context, target string) (string, error) {
	args := f.appendDirectFlag([]string{})
	args = append(args, "--target", target, "--machine", "json", "target", "show")

	stdout, err := f.runFFXCmd(ctx, args...)
	if err != nil {
		return "", fmt.Errorf("ffx target show failed: %w", err)
	}

	var showInfo struct {
		Target struct {
			LastRebootReason *string `json:"last_reboot_reason"`
		} `json:"target"`
	}

	if err := json.Unmarshal(stdout, &showInfo); err != nil {
		return "", fmt.Errorf("failed to unmarshal ffx target show output: %w", err)
	}

	if showInfo.Target.LastRebootReason != nil {
		return *showInfo.Target.LastRebootReason, nil
	}

	return "", fmt.Errorf("no last reboot reason found in ffx target show output")
}
