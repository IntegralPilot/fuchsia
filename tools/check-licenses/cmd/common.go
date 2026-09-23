// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/config"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/boundary"
)

// ReconstructCommand scans raw args to find -bug and -desc flags and their values,
// separates them from positional arguments, and formats a correct syntax suggestion string.
// Returns the formatted command string and true if any misplaced flags (starting with -) were found.
func ReconstructCommand(commandPath string, args []string, placeholders []string, activeBug, activeDesc string) (string, bool) {
	misplacedFlags := false
	for _, arg := range args {
		if strings.HasPrefix(arg, "-") {
			misplacedFlags = true
			break
		}
	}

	var bugVal string
	var descVal string
	var positionals []string

	for i := 0; i < len(args); i++ {
		arg := args[i]
		if arg == "-bug" || arg == "--bug" {
			if i+1 < len(args) {
				bugVal = args[i+1]
				i++
			}
		} else if arg == "-desc" || arg == "--desc" {
			if i+1 < len(args) {
				descVal = args[i+1]
				i++
			}
		} else if strings.HasPrefix(arg, "-") {
			// skip unknown flags
		} else {
			positionals = append(positionals, arg)
		}
	}

	if activeBug != "" && bugVal == "" {
		bugVal = activeBug
	}
	if activeDesc != "" && activeDesc != "Auto-generated exception" && activeDesc != "Auto-generated allowlist entry" && descVal == "" {
		descVal = activeDesc
	}

	var cmdBuilder strings.Builder
	cmdBuilder.WriteString(fmt.Sprintf("fx check-licenses %s", commandPath))
	if bugVal != "" {
		cmdBuilder.WriteString(fmt.Sprintf(" -bug %s", bugVal))
	} else {
		cmdBuilder.WriteString(" -bug BUG_ID")
	}
	if descVal != "" && descVal != "Auto-generated exception" && descVal != "Auto-generated allowlist entry" {
		cmdBuilder.WriteString(fmt.Sprintf(" -desc %q", descVal))
	}

	for i, placeholder := range placeholders {
		if i < len(positionals) {
			cmdBuilder.WriteString(fmt.Sprintf(" %s", positionals[i]))
		} else {
			cmdBuilder.WriteString(fmt.Sprintf(" %s", placeholder))
		}
	}
	// Append remaining positionals if any
	if len(positionals) > len(placeholders) {
		for _, extra := range positionals[len(placeholders):] {
			cmdBuilder.WriteString(fmt.Sprintf(" %s", extra))
		}
	}

	return cmdBuilder.String(), misplacedFlags
}

func findProjectBasename(fuchsiaDir, targetPath string, cfg *config.MasterConfig) string {
	if cfg != nil {
		return cfg.FindProjectBasename(targetPath)
	}
	return config.NewMasterConfig(fuchsiaDir).FindProjectBasename(targetPath)
}

// ResolveAndValidatePath normalizes the fuchsia root and ensures the given input path
// resides safely within that root. Returns the absolute fuchsia path, the relative target path,
// or an error if the path escapes the root workspace.
func ResolveAndValidatePath(fuchsiaDir, inputPath string) (string, string, error) {
	if fuchsiaDir == "" {
		fuchsiaDir = os.Getenv("FUCHSIA_DIR")
		if fuchsiaDir == "" {
			fuchsiaDir = "."
		}
	}
	absFuchsiaDir, err := filepath.Abs(fuchsiaDir)
	if err != nil {
		return "", "", fmt.Errorf("failed to get absolute path for fuchsia_dir %s: %w", fuchsiaDir, err)
	}

	var absInputPath string
	if strings.HasPrefix(inputPath, "//") {
		absInputPath = filepath.Join(absFuchsiaDir, strings.TrimPrefix(inputPath, "//"))
	} else if filepath.IsAbs(inputPath) {
		absInputPath = filepath.Clean(inputPath)
	} else {
		workingDir := os.Getenv("BUILD_WORKING_DIRECTORY")
		if workingDir == "" {
			workingDir, _ = os.Getwd()
		}
		if workingDir != "" && (workingDir == absFuchsiaDir || strings.HasPrefix(workingDir, absFuchsiaDir+string(filepath.Separator))) {
			absInputPath = filepath.Join(workingDir, inputPath)
		} else {
			absInputPath = filepath.Join(absFuchsiaDir, inputPath)
		}
	}

	rel, err := filepath.Rel(absFuchsiaDir, absInputPath)
	if err != nil || strings.HasPrefix(rel, "..") {
		return "", "", fmt.Errorf("path %s must be inside fuchsia root %s", inputPath, absFuchsiaDir)
	}
	if rel == "." {
		rel = ""
	}
	return absFuchsiaDir, rel, nil
}

// InputContext encapsulates the workspace root, resolved input paths, and assembled configuration.
type InputContext struct {
	FuchsiaDir string
	RelPath    string
	AbsPath    string
	Config     *config.MasterConfig
}

// LoadInputContext normalizes the input path within the Fuchsia workspace and loads the v2 MasterConfig.
func LoadInputContext(fuchsiaDirFlag, inputPath string) (*InputContext, error) {
	absFuchsia, relPath, err := ResolveAndValidatePath(fuchsiaDirFlag, inputPath)
	if err != nil {
		return nil, err
	}
	absPath := filepath.Join(absFuchsia, relPath)
	builder := config.NewBuilder(absFuchsia)
	if err := builder.Assemble(); err != nil {
		return nil, fmt.Errorf("failed to assemble configuration: %w", err)
	}
	return &InputContext{
		FuchsiaDir: absFuchsia,
		RelPath:    relPath,
		AbsPath:    absPath,
		Config:     builder.Config,
	}, nil
}

// ResolveProjectRoot resolves the governing logical project root directory for a given input path.
func (ic *InputContext) ResolveProjectRoot(inputPath string) (string, error) {
	fuchsiaDir, relPath, err := ResolveAndValidatePath(ic.FuchsiaDir, inputPath)
	if err != nil {
		return "", err
	}
	absPath := filepath.Join(fuchsiaDir, relPath)
	if _, err := os.Stat(absPath); err != nil {
		return "", fmt.Errorf("path does not exist: %s", inputPath)
	}
	grouper := boundary.NewGrouper(fuchsiaDir, ic.Config.Boundary)
	return grouper.ResolveProjectRoot(absPath), nil
}

// UpdateConfigFile reads, mutates, and writes back a ConfigFile.
func UpdateConfigFile(destFile string, mutate func(*config.ConfigFile)) error {
	var cfg config.ConfigFile
	if data, err := os.ReadFile(destFile); err == nil {
		json.Unmarshal(data, &cfg)
	}

	mutate(&cfg)

	outData, err := json.MarshalIndent(cfg, "", "    ")
	if err != nil {
		return fmt.Errorf("failed to marshal JSON: %w", err)
	}
	outData = append(outData, '\n') // POSIX standard

	if err := os.WriteFile(destFile, outData, 0644); err != nil {
		return fmt.Errorf("failed to write config file %s: %w", destFile, err)
	}
	return nil
}

// LoadTargets parses and combines target paths from both the command line arguments and an optional file list,
// resolving each path to a canonical absolute path within the Fuchsia repository.
func LoadTargets(fileList, fuchsiaDir string, args []string) ([]string, error) {
	var rawTargets []string
	if fileList != "" {
		absList := fileList
		if strings.HasPrefix(absList, "//") {
			absList = filepath.Join(fuchsiaDir, strings.TrimPrefix(absList, "//"))
		} else if !filepath.IsAbs(absList) {
			absList = filepath.Join(fuchsiaDir, fileList)
		}
		data, err := os.ReadFile(absList)
		if err != nil {
			return nil, fmt.Errorf("failed to read file-list %s: %w", fileList, err)
		}
		for _, line := range strings.Split(string(data), "\n") {
			line = strings.TrimSpace(line)
			if line != "" && !strings.HasPrefix(line, "#") {
				rawTargets = append(rawTargets, line)
			}
		}
	}
	rawTargets = append(rawTargets, args...)
	if len(rawTargets) == 0 {
		return nil, fmt.Errorf("at least one target path must be provided via positional arguments or -file-list")
	}

	var targets []string
	for _, t := range rawTargets {
		t = strings.TrimSpace(t)
		if t == "" {
			continue
		}
		absFuchsia, rel, err := ResolveAndValidatePath(fuchsiaDir, t)
		if err != nil {
			return nil, err
		}
		targets = append(targets, filepath.Join(absFuchsia, rel))
	}
	if len(targets) == 0 {
		return nil, fmt.Errorf("at least one target path must be provided via positional arguments or -file-list")
	}
	return targets, nil
}

// getLogWriters configures destination io.Writers for logging based on the specified level and output directory.
// Log == 0: discard all output
// Log == 1: print to stdout only
// Log == 2: save logs to the outDir folder AND print to stdout
func getLogWriters(logLevel int, outDir string) (io.Writer, error) {
	logTargets := []io.Writer{}

	if logLevel >= 2 {
		if outDir != "" {
			if _, err := os.Stat(outDir); os.IsNotExist(err) {
				err := os.MkdirAll(outDir, 0755)
				if err != nil {
					return nil, fmt.Errorf("failed to create out directory [%v]: %w", outDir, err)
				}
			}
			logfilePath := filepath.Join(outDir, "logs")
			f, err := os.OpenFile(logfilePath, os.O_RDWR|os.O_CREATE|os.O_APPEND, 0666)
			if err != nil {
				return nil, fmt.Errorf("failed to create log file [%v]: %w", logfilePath, err)
			}
			logTargets = append(logTargets, f)
		}
	}

	switch {
	case logLevel <= 0:
		logTargets = append(logTargets, io.Discard)
	case logLevel >= 1:
		logTargets = append(logTargets, os.Stdout)
	}

	w := io.MultiWriter(logTargets...)
	return w, nil
}
