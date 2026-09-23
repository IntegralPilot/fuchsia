// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/readme_fuchsia"
)

func printUsage() {
	fmt.Fprintf(os.Stderr, "Usage: %s <command> [args...]\n", os.Args[0])
	fmt.Fprintf(os.Stderr, "Commands: validate, format, get, set, add, help\n")
}

func main() {
	if len(os.Args) < 2 {
		printUsage()
		os.Exit(1)
	}

	command := os.Args[1]
	switch command {
	case "help", "-h", "--help":
		printUsage()
		os.Exit(0)
	case "validate":
		if err := runValidate(os.Args[2:]); err != nil {
			if err.Error() != "" {
				fmt.Fprintln(os.Stderr, err)
			}
			os.Exit(1)
		}
	case "format":
		if err := runFormat(os.Args[2:]); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
	case "get":
		if err := runGet(os.Args[2:]); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
	case "set":
		if err := runSet(os.Args[2:]); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
	case "add":
		if err := runAdd(os.Args[2:]); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
	default:
		fmt.Fprintf(os.Stderr, "Unknown command: %s\n", command)
		printUsage()
		os.Exit(1)
	}
}

func resolveReadmePath(path string) string {
	if strings.HasPrefix(path, "//") {
		fuchsiaDir := os.Getenv("FUCHSIA_DIR")
		rel := strings.TrimPrefix(path, "//")
		if fuchsiaDir != "" {
			return filepath.Join(fuchsiaDir, rel)
		}
		return rel
	}
	return path
}

func selectBlock(readmes *[]*readme_fuchsia.Readme, blockSpec string, allowCreate bool) (*readme_fuchsia.Readme, error) {
	if len(*readmes) == 0 {
		if allowCreate && (blockSpec == "" || blockSpec == "0") {
			newReadme := &readme_fuchsia.Readme{}
			*readmes = append(*readmes, newReadme)
			return newReadme, nil
		} else if !allowCreate {
			return nil, errors.New("no projects found in README.fuchsia")
		}
	}

	if blockSpec == "" {
		return (*readmes)[0], nil
	}

	if idx, err := strconv.Atoi(blockSpec); err == nil {
		if idx < 0 || idx > len(*readmes) || (idx == len(*readmes) && !allowCreate) {
			return nil, fmt.Errorf("block index %d out of range (found %d blocks)", idx, len(*readmes))
		}
		if idx == len(*readmes) {
			newReadme := &readme_fuchsia.Readme{}
			*readmes = append(*readmes, newReadme)
			return newReadme, nil
		}
		return (*readmes)[idx], nil
	}

	for _, r := range *readmes {
		if strings.EqualFold(r.Name, blockSpec) {
			return r, nil
		}
	}

	if allowCreate {
		newReadme := &readme_fuchsia.Readme{Name: blockSpec}
		*readmes = append(*readmes, newReadme)
		return newReadme, nil
	}

	return nil, fmt.Errorf("no block found matching %q", blockSpec)
}

func runValidate(args []string) error {
	fs := flag.NewFlagSet("validate", flag.ExitOnError)
	projectRoot := fs.String("project-root", "", "Optional override for the project's physical location")
	allowMissingLicense := fs.Bool("allow-missing-license", false, "Allow missing license/license file")
	findingsFile := fs.String("findings_file", "", "Optional path to write structured JSON findings")
	fs.StringVar(findingsFile, "findings-file", "", "Alias for -findings_file")

	fs.Parse(args)

	if fs.NArg() != 1 {
		return fmt.Errorf("usage: validate [--project-root <dir>] [--allow-missing-license] [--findings_file <path>] <path/to/README.fuchsia>")
	}

	readmePath := resolveReadmePath(fs.Arg(0))

	readmes, err := readme_fuchsia.ParseFile(readmePath)
	if err != nil {
		return fmt.Errorf("failed to parse %s: %w", readmePath, err)
	}

	root := *projectRoot
	if root == "" {
		dir := filepath.Dir(readmePath)
		if idx := strings.Index(dir, "tools/check-licenses/assets/readmes/"); idx != -1 {
			root = dir[idx+len("tools/check-licenses/assets/readmes/"):]
		} else if idx := strings.Index(dir, "vendor/google/tools/check-licenses/assets/readmes/"); idx != -1 {
			root = dir[idx+len("vendor/google/tools/check-licenses/assets/readmes/"):]
		} else {
			root = dir
		}
	}

	errs := readme_fuchsia.Validate(root, readmes)
	if *allowMissingLicense && len(errs) > 0 {
		var filteredErrs []error
		for _, err := range errs {
			msg := err.Error()
			if strings.Contains(msg, "Missing required field 'License'") || strings.Contains(msg, "Missing required field 'License File'") {
				continue
			}
			filteredErrs = append(filteredErrs, err)
		}
		errs = filteredErrs
	}

	if *findingsFile != "" {
		findings := []readme_fuchsia.Finding{}
		for _, err := range errs {
			if f, ok := err.(readme_fuchsia.Finding); ok {
				if f.FilePath == "" {
					f.FilePath = readmePath
				}
				findings = append(findings, f)
			} else {
				findings = append(findings, readme_fuchsia.Finding{
					FilePath: readmePath,
					Level:    "error",
					Message:  err.Error(),
				})
			}
		}

		data, marshalErr := json.MarshalIndent(findings, "", "  ")
		if marshalErr != nil {
			return fmt.Errorf("failed to marshal findings: %w", marshalErr)
		}
		if dir := filepath.Dir(*findingsFile); dir != "" && dir != "." {
			if err := os.MkdirAll(dir, 0755); err != nil {
				return fmt.Errorf("failed to create directory for findings file: %w", err)
			}
		}
		if err := os.WriteFile(*findingsFile, append(data, '\n'), 0644); err != nil {
			return fmt.Errorf("failed to write findings file: %w", err)
		}
	}

	if len(errs) > 0 {
		fmt.Fprintf(os.Stderr, "validation failed for %s\n", readmePath)
		for _, err := range errs {
			fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		}
		return errors.New("")
	}

	fmt.Printf("Validation passed for %s\n", readmePath)
	return nil
}

func runFormat(args []string) error {
	fs := flag.NewFlagSet("format", flag.ExitOnError)
	stdout := fs.Bool("stdout", false, "Print formatted content to stdout instead of modifying the file in-place")
	fs.Parse(args)

	if fs.NArg() > 1 {
		return fmt.Errorf("usage: format [--stdout] [path/to/README.fuchsia]")
	}

	readmePath := "README.fuchsia"
	if fs.NArg() == 1 {
		readmePath = fs.Arg(0)
	}
	readmePath = resolveReadmePath(readmePath)

	readmes, err := readme_fuchsia.ParseFile(readmePath)
	if err != nil {
		return fmt.Errorf("failed to parse %s: %w", readmePath, err)
	}

	formatted := readme_fuchsia.Format(readmes)

	if *stdout {
		fmt.Print(formatted)
		return nil
	}

	err = os.WriteFile(readmePath, []byte(formatted), 0644)
	if err != nil {
		return fmt.Errorf("failed to write formatted content to %s: %w", readmePath, err)
	}

	fmt.Printf("Formatted %s\n", readmePath)
	return nil
}

func runGet(args []string) error {
	fs := flag.NewFlagSet("get", flag.ExitOnError)
	block := fs.String("block", "", "Target block by index (0-based) or project Name (default is first block)")
	fs.Parse(args)

	if fs.NArg() < 1 || fs.NArg() > 2 {
		return fmt.Errorf("usage: get [--block <index|name>] <field> [path/to/README.fuchsia]")
	}

	field := fs.Arg(0)
	readmePath := "README.fuchsia"
	if fs.NArg() == 2 {
		readmePath = fs.Arg(1)
	}
	readmePath = resolveReadmePath(readmePath)

	readmes, err := readme_fuchsia.ParseFile(readmePath)
	if err != nil {
		return fmt.Errorf("failed to parse %s: %w", readmePath, err)
	}

	readme, err := selectBlock(&readmes, *block, false)
	if err != nil {
		return fmt.Errorf("failed to select block in %s: %w", readmePath, err)
	}
	val, ok := readme.GetField(field)
	if !ok {
		return fmt.Errorf("field %q not found in %s", field, readmePath)
	}

	fmt.Println(val)
	return nil
}

func runSet(args []string) error {
	fs := flag.NewFlagSet("set", flag.ExitOnError)
	block := fs.String("block", "", "Target block by index (0-based) or project Name (default is first block)")
	fs.Parse(args)

	if fs.NArg() < 2 || fs.NArg() > 3 {
		return fmt.Errorf("usage: set [--block <index|name>] <field> <value> [path/to/README.fuchsia]")
	}

	field := fs.Arg(0)
	value := fs.Arg(1)
	readmePath := "README.fuchsia"
	if fs.NArg() == 3 {
		readmePath = fs.Arg(2)
	}
	readmePath = resolveReadmePath(readmePath)

	readmes, err := readme_fuchsia.ParseFile(readmePath)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			readmes = []*readme_fuchsia.Readme{}
		} else {
			return fmt.Errorf("failed to parse %s: %w", readmePath, err)
		}
	}

	readme, err := selectBlock(&readmes, *block, true)
	if err != nil {
		return fmt.Errorf("failed to select block in %s: %w", readmePath, err)
	}
	err = readme.SetField(field, value)
	if err != nil {
		return fmt.Errorf("failed to set field %q: %w", field, err)
	}

	formatted := readme_fuchsia.Format(readmes)
	if err := os.MkdirAll(filepath.Dir(readmePath), 0755); err != nil {
		return fmt.Errorf("failed to create directory for %s: %w", readmePath, err)
	}
	err = os.WriteFile(readmePath, []byte(formatted), 0644)
	if err != nil {
		return fmt.Errorf("failed to write to %s: %w", readmePath, err)
	}

	return nil
}

func runAdd(args []string) error {
	fs := flag.NewFlagSet("add", flag.ExitOnError)
	block := fs.String("block", "", "Target block by index (0-based) or project Name (default is first block)")
	fs.Parse(args)

	if fs.NArg() < 2 || fs.NArg() > 3 {
		return fmt.Errorf("usage: add [--block <index|name>] <field> <value> [path/to/README.fuchsia]")
	}

	field := fs.Arg(0)
	value := fs.Arg(1)
	readmePath := "README.fuchsia"
	if fs.NArg() == 3 {
		readmePath = fs.Arg(2)
	}
	readmePath = resolveReadmePath(readmePath)

	readmes, err := readme_fuchsia.ParseFile(readmePath)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			readmes = []*readme_fuchsia.Readme{}
		} else {
			return fmt.Errorf("failed to parse %s: %w", readmePath, err)
		}
	}

	readme, err := selectBlock(&readmes, *block, true)
	if err != nil {
		return fmt.Errorf("failed to select block in %s: %w", readmePath, err)
	}
	err = readme.AddField(field, value)
	if err != nil {
		return fmt.Errorf("failed to add field %q: %w", field, err)
	}

	formatted := readme_fuchsia.Format(readmes)
	if err := os.MkdirAll(filepath.Dir(readmePath), 0755); err != nil {
		return fmt.Errorf("failed to create directory for %s: %w", readmePath, err)
	}
	err = os.WriteFile(readmePath, []byte(formatted), 0644)
	if err != nil {
		return fmt.Errorf("failed to write to %s: %w", readmePath, err)
	}

	return nil
}
