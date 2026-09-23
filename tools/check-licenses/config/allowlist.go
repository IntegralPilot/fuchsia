// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package config

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/readme"
)

// FindProjectBasename derives the project's base name used for naming JSON config files.
func (c *MasterConfig) FindProjectBasename(targetPath string) string {
	fuchsiaDir := resolveFuchsiaDir("")
	if c != nil {
		fuchsiaDir = resolveFuchsiaDir(c.FuchsiaDir)
	}
	cleanTargetPath := strings.TrimPrefix(filepath.ToSlash(filepath.Clean(targetPath)), "/")
	absTargetPath := filepath.Join(fuchsiaDir, filepath.FromSlash(cleanTargetPath))

	// Priority 1: Check README.fuchsia (or Cargo.toml / go.mod) for an explicit Name
	metadataFiles := []string{"README.fuchsia", "Cargo.toml", "go.mod", "pubspec.yaml"}
	for _, mf := range metadataFiles {
		metaPath := filepath.Join(absTargetPath, mf)
		if _, err := os.Stat(metaPath); err == nil {
			if rootReadmes, subReadmes, err := readme.ParseAnyMetadata(metaPath); err == nil {
				if len(rootReadmes) > 0 && rootReadmes[0].Name != "" {
					return filepath.Base(rootReadmes[0].Name)
				}
				if len(subReadmes) > 0 && subReadmes[0].Name != "" {
					return filepath.Base(subReadmes[0].Name)
				}
			}
		}
	}

	// Priority 1.5: Check Virtual Out-Of-Tree READMEs
	if c != nil && c.Boundary.OutOfTreeReadmes != nil {
		if virtualReadmePath, ok := c.Boundary.OutOfTreeReadmes[cleanTargetPath]; ok {
			absVirtualPath := virtualReadmePath
			if !filepath.IsAbs(absVirtualPath) {
				absVirtualPath = filepath.Join(fuchsiaDir, virtualReadmePath)
			}
			if _, err := os.Stat(absVirtualPath); err == nil {
				if rootReadmes, _, err := readme.ParseAnyMetadata(absVirtualPath); err == nil && len(rootReadmes) > 0 && rootReadmes[0].Name != "" {
					return filepath.Base(rootReadmes[0].Name)
				}
			}
		}
	}

	// Priority 2: Check Jiri Manifest mapping
	if c != nil {
		if name := c.ManifestNameFor(cleanTargetPath); name != "" {
			return filepath.Base(name)
		}
	}

	// Priority 3: Fallback for first-party or paths not in manifest
	dir := filepath.Dir(cleanTargetPath)
	if dir == "." || dir == "/" || cleanTargetPath == "" {
		return "root"
	}

	parts := strings.Split(cleanTargetPath, "/")
	if len(parts) > 0 && parts[0] != "" {
		if parts[0] == "src" && len(parts) > 1 && parts[1] != "" {
			return parts[1]
		}
		if (parts[0] == "third_party" || parts[0] == "vendor" || parts[0] == "prebuilt") && len(parts) > 1 {
			return filepath.Base(cleanTargetPath)
		}
		return parts[0]
	}
	return "root"
}

// AddAllowlistEntry creates or updates an allowed_licenses JSON config entry on disk.
func (c *MasterConfig) AddAllowlistEntry(projectPath, licenseName, bug, description string) (string, error) {
	relPath := projectPath
	if filepath.IsAbs(relPath) {
		rel, err := filepath.Rel(c.FuchsiaDir, relPath)
		if err == nil {
			relPath = rel
		}
	}

	category := c.CategoryForLicense(licenseName)
	if category == "Uncategorized" {
		return "", fmt.Errorf("unknown or unapproved license name %q. If this is a brand new license, it must first be reviewed by the OSRB and manually categorized under allowed_licenses/ first", licenseName)
	}

	configDir := filepath.Join(c.ConfigRootFor(relPath), "allowed_licenses", category, licenseName)
	if err := os.MkdirAll(configDir, 0755); err != nil {
		return "", fmt.Errorf("failed to create config directory %s: %w", configDir, err)
	}

	baseName := c.FindProjectBasename(relPath)
	destFile := filepath.Join(configDir, baseName+".json")

	if err := UpdateConfigFile(destFile, func(cfg *ConfigFile) {
		if cfg.AllowedLicenses == nil {
			cfg.AllowedLicenses = make(map[string][]AllowlistEntry)
		}
		entry := AllowlistEntry{
			Bug:         bug,
			Description: description,
			Paths:       []string{relPath},
		}
		cfg.AllowedLicenses[licenseName] = append(cfg.AllowedLicenses[licenseName], entry)
	}); err != nil {
		return "", err
	}

	return destFile, nil
}

// UpdateConfigFile reads an existing config JSON file (or creates an empty one),
// runs the mutate function, and writes the formatted JSON back to disk.
func UpdateConfigFile(destFile string, mutate func(*ConfigFile)) error {
	var cfg ConfigFile
	if data, err := os.ReadFile(destFile); err == nil {
		_ = json.Unmarshal(data, &cfg)
	}

	mutate(&cfg)

	outData, err := json.MarshalIndent(cfg, "", "    ")
	if err != nil {
		return fmt.Errorf("failed to marshal JSON: %w", err)
	}
	outData = append(outData, '\n')

	if err := os.WriteFile(destFile, outData, 0644); err != nil {
		return fmt.Errorf("failed to write config file %s: %w", destFile, err)
	}
	return nil
}
