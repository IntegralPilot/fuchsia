// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Package ffx provides wrappers and convenience functions for using the ffx binaries.
package orchestrate

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"

	utils "go.fuchsia.dev/fuchsia/tools/orchestrate/utils"
)

type pathType struct {
	File string `json:"file"`
	URL  string `json:"url"`
}

type repoServerList struct {
	Result struct {
		Data []struct {
			Name                          string   `json:"name"`
			Address                       string   `json:"address"`
			RepoPath                      pathType `json:"repo_path"`
			RegistrationAliases           []string `json:"registration_aliases"`
			RegistrationStorageType       string   `json:"registration_storage_type"`
			RegistrationAliasConflictMode string   `json:"registration_alias_conflict_mode"`
			ServerMode                    string   `json:"server_mode"`
			PID                           int      `json:"pid"`
		} `json:"data"`
	} `json:"ok"`
}

// XDG_ENV_VARS are leaky environment variables to override. See ApplyEnv.
var XDG_ENV_VARS = [...]string{
	"HOME",
	"XDG_CACHE_HOME",
	"XDG_CONFIG_HOME",
	"XDG_DATA_HOME",
	"XDG_HOME",
	"XDG_STATE_HOME",
}

// Ffx defines settings for ffx commands.
type Ffx struct {
	// Dir is the working directory for ffx, and where it writes files.
	Dir             string
	bin             string
	sslCertPath     string
	defaultTarget   *string
	configOverrides map[string]string
}

// Option for creating Ffx.
type Option struct {
	// IsolateDir is the directory where all ffx state is stored. This can be
	// used to allow multiple instances of ffx to share the same configuration.
	// If the directory contents are empty, a default config will be generated.
	// If not set, a temporary directory will be created for this instance of
	// Ffx to use.
	IsolateDir string

	// ExePath is the path to the ffx cli tool.
	ExePath string

	// SSLCertPath is the path to the SSL certificates that ffx should use. If
	// not set, the default certificates will be used from the runfiles.
	// If the runfiles are not available, then the system paths will be searched
	// for appropriate certificates.
	SSLCertPath string

	// LogDir is the directory where ffx logs are stored.
	// If not set, it defaults to a sub-directory of the IsolateDir.
	//
	// Note: This option is only used to write the default ffx config when
	// initializing an empty isolate directory. Otherwise, the existing config in
	// IsolateDir is used.
	LogDir string

	// PrivateSSH is the list of the pregenerated private ssh key files that ffx
	// should use to connect to targets.
	//
	// Note: This option is only used to write the default ffx config when
	// initializing an empty isolate directory. Otherwise, the existing config in
	// IsolateDir is used.
	PrivateSSH []string

	// PublicSSH is the list of the pregenerated public ssh key files and
	// authorized_keys files that ffx should install when flashing targets or
	// starting up emulator instances.
	//
	// Note: This option is only used to write the default ffx config when
	// initializing an empty isolate directory. Otherwise, the existing config in
	// IsolateDir is used.
	PublicSSH []string

	// EnableCSO enables circuit-switched-overnet.
	// If set to false, the ffx default value is used.
	//
	// Note: This option is only used to write the default ffx config when
	// initializing an empty isolate directory. Otherwise, the existing config in
	// IsolateDir is used.
	EnableCSO bool

	// FfxConfig is a map of configuration key-values to pass via "--config key=value".
	FfxConfig map[string]string
}

// New sets up a config and filepaths for local or Forge use.
func New(ctx context.Context, opt *Option) (*Ffx, error) {
	f := &Ffx{
		bin:             opt.ExePath,
		sslCertPath:     opt.SSLCertPath,
		Dir:             opt.IsolateDir,
		configOverrides: opt.FfxConfig,
	}
	var err error
	if f.Dir == "" {
		if f.Dir, err = os.MkdirTemp("", "ffx"); err != nil {
			return nil, fmt.Errorf("create directory for ffx: %w", err)
		}
	}

	if f.bin == "" {
		return nil, errors.New("must provide path to ffx executable")
	}

	// Setup the default config if it has not been initialized yet.
	configPath := filepath.Join(f.Dir, ".ffx_user_config.json")
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		if err := f.setupDefaultConfig(ctx, configPath, *opt); err != nil {
			return nil, err
		}
	} else if err != nil {
		return nil, fmt.Errorf("unable to stat config file: %w", err)
	}
	return f, nil
}

func (f *Ffx) setupDefaultConfig(ctx context.Context, configPath string, opt Option) error {
	if opt.LogDir == "" {
		opt.LogDir = filepath.Join(f.Dir, "log")
	}
	if err := os.MkdirAll(opt.LogDir, 0755); err != nil {
		return err
	}

	socketPath := filepath.Join(f.Dir, "ascendd")
	err := writeConfigFile(configPath, opt, socketPath)
	if err != nil {
		return err
	}

	// Write the config to the isolation dir so that we don't need to pass it with every command.
	if out, err := f.RunCmdSync(ctx, "config", "env", "set", configPath); err != nil {
		return fmt.Errorf("saving ffx config path: %s, %w", out, err)
	}
	return nil
}

// CmdContext returns a generic exec.Cmd configured to execute ffx with context.
func (f *Ffx) CmdContext(ctx context.Context, args ...string) (*exec.Cmd, error) {
	var cmdArgs []string
	if len(f.configOverrides) > 0 {
		keys := make([]string, 0, len(f.configOverrides))
		for k := range f.configOverrides {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for _, k := range keys {
			cmdArgs = append(cmdArgs, "--config", fmt.Sprintf("%s=%s", k, f.configOverrides[k]))
		}
	}
	cmdArgs = append(cmdArgs, args...)
	cmd := exec.CommandContext(ctx, f.bin, cmdArgs...)
	env, err := f.ApplyEnv(cmd.Environ())
	if err != nil {
		return nil, fmt.Errorf("Applying ffx env: %v", err)
	}
	cmd.Env = env
	return cmd, nil
}

// ApplyEnv adds the environment variables needed for safe execution of ffx.
func (f *Ffx) ApplyEnv(env []string) ([]string, error) {
	wd, err := os.Getwd()
	if err != nil {
		return nil, fmt.Errorf("os.Getwd: %v", err)
	}
	env = append(env, "FFX_ISOLATE_DIR="+f.Dir, "FUCHSIA_ANALYTICS_DISABLED=1")

	// Unset FUCHSIA_DEVICE_ADDR and maybe override FUCHSIA_NODENAME.
	for idx, val := range env {
		if strings.HasPrefix(val, "FUCHSIA_DEVICE_ADDR=") {
			for ; idx < len(env)-1; idx++ {
				env[idx] = env[idx+1]
			}
			env = env[:len(env)-1]
			break
		}
	}
	if f.defaultTarget != nil {
		env = append(env, "FUCHSIA_NODENAME="+*f.defaultTarget)
	}

	if f.sslCertPath != "" {
		env = append(env, "SSL_CERT_FILE="+f.sslCertPath)
	}
	// Override HOME and other HOME-related environment variables, since ffx and
	// tests shouldn't assume anything about those.
	// This prevents ffx from creating and using default ssh keys from the real
	// home directory.
	for _, xdg_env_var := range XDG_ENV_VARS {
		env = append(env, xdg_env_var+"="+f.Dir)
	}
	sshDir := filepath.Join(wd, "openssh-portable", "bin")
	// The path to the ssh binary is required.
	env = utils.AppendPath(env, sshDir)
	return env, nil
}

// RunCmdSync starts a command and waits for the command to complete.
func (f *Ffx) RunCmdSync(ctx context.Context, args ...string) (string, error) {
	cmd, err := f.CmdContext(ctx, args...)
	if err != nil {
		return "", fmt.Errorf("Creating ffx command from %+v: %v", args, err)
	}
	log.Printf("Running command and streaming output: %+v", cmd.Args)

	// Pipe stderr to stdout, and then tee to a string builder.
	var output strings.Builder
	outputWriter := io.MultiWriter(&output, os.Stdout)
	cmd.Stdout = outputWriter
	cmd.Stderr = outputWriter

	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("cmd.Run: %w", err)
	}

	return output.String(), nil
}

// RunCmdAsync starts a command but does NOT wait for the command to complete.
func (f *Ffx) RunCmdAsync(ctx context.Context, args ...string) (*exec.Cmd, error) {
	cmd, err := f.CmdContext(ctx, args...)
	if err != nil {
		return nil, fmt.Errorf("Creating ffx command from %+v: %v", args, err)
	}
	log.Printf("Running background command: %+v", cmd.Args)
	if err := cmd.Start(); err != nil {
		return cmd, fmt.Errorf("start command %s: %w", args, err)
	}
	return cmd, nil
}

// ConfigGet reads from the ffx config and writes to result as structured data.
func (f *Ffx) ConfigGet(ctx context.Context, field string, result any) error {
	out, err := f.RunCmdSync(ctx, "config", "get", field)
	if err != nil {
		return fmt.Errorf("ffx config failed for %q: %w", field, err)
	}
	if err := json.Unmarshal([]byte(out), result); err != nil {
		return fmt.Errorf("unable to unmarshal config output: %w", err)
	}
	return nil
}

// Close removes all files from the ffx directory.
func (f *Ffx) Close() error {
	return os.RemoveAll(f.Dir)
}

// SetDefaultTarget sets the default ffx target.
// If nil, it's inherited from the $FUCHSIA_NODENAME environment variable.
func (f *Ffx) SetDefaultTarget(target *string) {
	if target == nil {
		log.Printf("Default target unset")
	} else {
		log.Printf("Default target set: %s", *target)
	}
	f.defaultTarget = target
}

// GetDefaultTarget gets the effective default ffx target.
func (f *Ffx) GetDefaultTarget(ctx context.Context) (string, error) {
	defaultName, err := f.RunCmdSync(ctx, "target", "default", "get")
	if err != nil {
		return "", fmt.Errorf("run \"target default get\" command. %s. %w", defaultName, err)
	}
	// An extra '\n' is added at the end of defaultName.
	return strings.TrimSpace(defaultName), nil
}

// Flash uses "ffx target flash" to flash a product bundle into a device.
// pubKeyPath is optional and ignored if empty.
func (f *Ffx) Flash(ctx context.Context, fastbootSerial, productDir, pubKeyPath string) error {
	ffxArgs := []string{
		"--target", fastbootSerial,
		"target", "flash",
		"--product-bundle", productDir}
	if pubKeyPath != "" {
		ffxArgs = append(ffxArgs, "--authorized-keys", pubKeyPath)
	}
	_, err := f.RunCmdSync(ctx, ffxArgs...)
	return err
}

func writeConfigFile(configPath string, opt Option, socketPath string) error {
	overnet := map[string]string{"socket": socketPath}
	if opt.EnableCSO {
		overnet["cso"] = "enabled"
	}
	ssh := map[string][]string{}
	if len(opt.PrivateSSH) > 0 {
		ssh["priv"] = opt.PrivateSSH
	}
	if len(opt.PublicSSH) > 0 {
		ssh["pub"] = opt.PublicSSH
	}
	data := map[string]any{
		"connectivity": map[string]bool{
			"direct": true,
		},
		"overnet": overnet,
		"proxy": map[string]int{
			"timeout_secs": 60,
		},
		"ssh": ssh,
		"log": map[string]any{
			"dir":     []string{opt.LogDir},
			"enabled": []bool{true},
			"level":   "Debug",
		},
		"test": map[string]any{
			"suite_start_timeout_seconds": 600,
		},
	}
	j, err := json.Marshal(data)
	if err != nil {
		return fmt.Errorf("marshal ffx config: %w", err)
	}
	if err := os.WriteFile(configPath, j, 0o600); err != nil {
		return fmt.Errorf("writing ffx config to file: %w", err)
	}
	return nil
}

// isRunning returns true if the package server is currently running and responds to HTTP requests.
func (f *Ffx) IsPackageServerRunning(ctx context.Context, repoName string) (bool, error) {
	args := []string{"--machine", "json", "repository", "server", "list"}
	out, err := f.RunCmdSync(ctx, args...)
	if err != nil {
		return false, fmt.Errorf("ffx repository server list: output: %s, error: %w", out, err)
	}
	var repoList repoServerList
	if err := json.Unmarshal([]byte(out), &repoList); err != nil {
		return false, err
	}
	repoNamePrefix := fmt.Sprintf("%s.", repoName)
	for _, status := range repoList.Result.Data {
		// product bundle based repo servers use the repoName as a prefix.
		if status.Name == repoName || strings.HasPrefix(status.Name, repoNamePrefix) {
			return true, nil
		}
	}
	// We don't need to differentiate between a stopped package server, no server found, etc.
	return false, nil
}

// ProductDownload downloads a product bundle.
func (f *Ffx) ProductDownload(ctx context.Context, transferURL, outDir, authPath string) error {
	args := []string{
		"product",
		"download",
		transferURL,
		outDir,
	}
	if authPath != "" {
		args = append(args, "--auth", authPath)
	}
	_, err := f.RunCmdSync(ctx, args...)
	return err
}

// EmuStart starts the emulator.
func (f *Ffx) EmuStart(ctx context.Context, productDir, name, engine, device string) error {
	args := []string{
		"emu",
		"start",
		productDir,
		"--net", "user",
		"--headless",
		"--startup-timeout", "300",
		"--name", name,
	}
	if engine != "" {
		args = append(args, "--engine", engine)
	}
	if device != "" {
		args = append(args, "--device", device)
	}
	_, err := f.RunCmdSync(ctx, args...)
	return err
}

// RepositoryCreate creates a repository.
func (f *Ffx) RepositoryCreate(ctx context.Context, repoDir string) error {
	_, err := f.RunCmdSync(ctx, "repository", "create", repoDir)
	return err
}

// RepositoryPublish publishes packages to a repository.
func (f *Ffx) RepositoryPublish(ctx context.Context, repoDir, productDir string, packageArchives []string) error {
	if _, err := f.RunCmdSync(ctx, "repository", "publish", repoDir, "--product-bundle", productDir); err != nil {
		return err
	}
	if len(packageArchives) > 0 {
		publishArgs := []string{"repository", "publish", repoDir}
		for _, far := range packageArchives {
			publishArgs = append(publishArgs, "--package-archive", far)
		}
		if _, err := f.RunCmdSync(ctx, publishArgs...); err != nil {
			return err
		}
	}
	return nil
}

// SymbolIndexAdd adds a build ID to the symbol index.
func (f *Ffx) SymbolIndexAdd(ctx context.Context, buildID string) error {
	_, err := f.RunCmdSync(ctx, "debug", "symbol-index", "add", buildID)
	return err
}

// RepositoryServerStart starts the repository server.
func (f *Ffx) RepositoryServerStart(ctx context.Context, repoName, repoDir, address string) error {
	args := []string{
		"repository", "server", "start",
		"--background", "--no-device",
		"--address", address,
		"--repo-path", repoDir,
		"--repository", repoName,
		"--refresh-metadata",
	}
	_, err := f.RunCmdSync(ctx, args...)
	return err
}

// RepositoryServerStop stops the repository server.
func (f *Ffx) RepositoryServerStop(ctx context.Context, repoName string) error {
	_, err := f.RunCmdSync(ctx, "repository", "server", "stop", repoName)
	return err
}

// RepositoryServerList lists the repository servers.
func (f *Ffx) RepositoryServerList(ctx context.Context) (string, error) {
	return f.RunCmdSync(ctx, "repository", "server", "list")
}

// TargetAdd adds a target.
func (f *Ffx) TargetAdd(ctx context.Context, addr string) error {
	_, err := f.RunCmdSync(ctx, "target", "add", addr, "--nowait")
	return err
}

// TargetList lists the targets.
func (f *Ffx) TargetList(ctx context.Context) (string, error) {
	return f.RunCmdSync(ctx, "--machine", "json-pretty", "target", "list")
}

// TargetWait waits for the target to be reachable.
func (f *Ffx) TargetWait(ctx context.Context) error {
	_, err := f.RunCmdSync(ctx, "target", "wait")
	return err
}

// TargetShow shows target details.
func (f *Ffx) TargetShow(ctx context.Context) (string, error) {
	return f.RunCmdSync(ctx, "--machine", "json-pretty", "target", "show")
}

// TargetRepositoryRegister registers a repository with the target.
func (f *Ffx) TargetRepositoryRegister(ctx context.Context, repoName string, aliases []string) error {
	args := []string{
		"target", "repository", "register",
		"--repository", repoName,
	}
	for _, alias := range aliases {
		args = append(args, "--alias", alias)
	}
	_, err := f.RunCmdSync(ctx, args...)
	return err
}

// TargetSnapshot takes a target snapshot.
func (f *Ffx) TargetSnapshot(ctx context.Context, dir string) error {
	_, err := f.RunCmdSync(ctx, "target", "snapshot", "-d", dir)
	return err
}

// Symbolize symbolizes logs using ffx debug symbolize.
func (f *Ffx) Symbolize(ctx context.Context, input io.Reader, output io.Writer) error {
	cmd, err := f.CmdContext(ctx, "debug", "symbolize")
	if err != nil {
		return err
	}
	cmd.Stdin = input
	cmd.Stdout = output
	cmd.Stderr = output
	return cmd.Run()
}

// SetupFfx configures ffx.
func (f *Ffx) SetupFfx(ctx context.Context, repoName string) error {
	cmds := [][]string{
		{"config", "set", "log.level", "Debug"},
		{"config", "set", "test.experimental_json_input", "true"},
		{"config", "set", "fastboot.flash.timeout_rate", "1"},
		{"config", "set", "fastboot.flash.min_timeout_secs", "600"},
		{"config", "set", "discovery.mdns.enabled", "false"},
		{"config", "set", "fastboot.usb.disabled", "true"},
		{"config", "set", "proactive_log.enabled", "false"},
		{"config", "set", "overnet.cso", "only"},
		{"config", "set", "repository.default", repoName},
		{"config", "set", "repository.server.enabled", "false"},
		{"config", "set", "connectivity.direct", "true"},
	}

	for _, cmd := range cmds {
		if _, err := f.RunCmdSync(ctx, cmd...); err != nil {
			return fmt.Errorf("ffx setup %v: %w", cmd, err)
		}
	}

	logDir := utils.GetOutputsDir()
	if logDir != "" {
		cmd := []string{"config", "set", "log.dir", logDir}
		if _, err := f.RunCmdSync(ctx, cmd...); err != nil {
			return fmt.Errorf("ffx setup %v: %w", cmd, err)
		}
	}

	ffxConfigDump := filepath.Join(logDir, "ffx_config.txt")
	if err := f.dumpFfxConfig(ctx, ffxConfigDump); err != nil {
		return fmt.Errorf("dumpFfxConfig: %w", err)
	}

	return nil
}

func (f *Ffx) dumpFfxConfig(ctx context.Context, outputPath string) error {
	logFile, err := os.Create(outputPath)
	if err != nil {
		return fmt.Errorf("os.Create: %w", err)
	}
	defer logFile.Close()
	cmd, err := f.CmdContext(ctx, "config", "get")
	if err != nil {
		return err
	}
	cmd.Stdout = logFile
	cmd.Stderr = logFile
	return cmd.Run()
}

// EmuStop stops all running emulator instances.
func (f *Ffx) EmuStop(ctx context.Context) error {
	_, err := f.RunCmdSync(ctx, "emu", "stop", "--all")
	return err
}

type ffxLogCloser struct {
	cmd *exec.Cmd
}

func (c *ffxLogCloser) Close() error {
	if c.cmd == nil || c.cmd.Process == nil {
		return nil
	}
	return c.cmd.Process.Kill()
}

// TargetLogStart starts streaming target logs in the background.
func (f *Ffx) TargetLogStart(ctx context.Context, output io.Writer) (io.Closer, error) {
	cmd, err := f.CmdContext(ctx, "log", "--symbolize", "off")
	if err != nil {
		return nil, fmt.Errorf("ffx.Cmd: %w", err)
	}
	cmd.Stdout = output
	cmd.Stderr = output
	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("cmd.Start: %w", err)
	}
	go func() {
		if err := cmd.Wait(); err != nil {
			log.Printf("ffx log stream finished: %v", err)
		}
	}()
	return &ffxLogCloser{cmd: cmd}, nil
}
