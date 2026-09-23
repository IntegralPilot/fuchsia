// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package config

import (
	"encoding/json"
	"encoding/xml"
	"fmt"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/stages/validate"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
)

// Builder handles the Assembly Phase, scanning directories for config files
// and aggregating them into a MasterConfig.
type Builder struct {
	Config *MasterConfig
	seen   map[string]bool
}

// NewBuilder creates a new config assembler.
func NewBuilder(fuchsiaDir string) *Builder {
	b := &Builder{
		Config: NewMasterConfig(fuchsiaDir),
		seen:   make(map[string]bool),
	}
	return b
}

// Assemble starts the recursive configuration discovery from the root v2 config file,
// merging all found JSON files into the internal MasterConfig.
func (b *Builder) Assemble() error {
	if err := b.LoadManifests(); err != nil {
		fmt.Fprintf(os.Stderr, "Warning: Failed to load manifests: %v\n", err)
	}

	rootConfig := filepath.Join(b.Config.FuchsiaDir, "tools", "check-licenses", "config.json")
	if _, err := os.Stat(rootConfig); os.IsNotExist(err) {
		return fmt.Errorf("root config file not found: %s", rootConfig)
	}
	err := b.parseConfigFile(rootConfig)
	assetDirs := []string{
		filepath.Join(b.Config.FuchsiaDir, "tools", "check-licenses", "assets"),
	}
	vendorDir := filepath.Join(b.Config.FuchsiaDir, "vendor")
	if entries, readErr := os.ReadDir(vendorDir); readErr == nil {
		for _, entry := range entries {
			candidate := filepath.Join(vendorDir, entry.Name(), "tools", "check-licenses", "assets")
			if info, statErr := os.Stat(candidate); statErr == nil && info.IsDir() {
				assetDirs = append(assetDirs, candidate)
			}
		}
	}
	for _, assetDir := range assetDirs {
		filepath.WalkDir(assetDir, func(path string, d fs.DirEntry, walkErr error) error {
			if walkErr != nil {
				return nil
			}
			slashPath := filepath.ToSlash(path)
			if idx := strings.Index(slashPath, "/allowed_licenses/"); idx != -1 {
				subParts := strings.Split(slashPath[idx+len("/allowed_licenses/"):], "/")
				if len(subParts) >= 2 && subParts[0] != "" && subParts[1] != "" {
					if b.Config.Classify.LicenseCategories == nil {
						b.Config.Classify.LicenseCategories = make(map[string]string)
					}
					b.Config.Classify.LicenseCategories[subParts[1]] = subParts[0]
				}
			}
			if d.IsDir() {
				return nil
			}
			if d.Name() == "README.fuchsia" {
				rel, relErr := filepath.Rel(b.Config.FuchsiaDir, filepath.Dir(path))
				if relErr == nil {
					logical := filepath.ToSlash(rel)
					if idx := strings.Index(logical, "tools/check-licenses/assets/readmes"); idx != -1 {
						logical = strings.TrimPrefix(logical[idx+len("tools/check-licenses/assets/readmes"):], "/")
						if logical == "" {
							logical = "."
						}
						if _, exists := b.Config.Boundary.OutOfTreeReadmes[logical]; !exists {
							b.Config.Boundary.OutOfTreeReadmes[logical] = path
						}
					}
				}
			}
			return nil
		})
	}

	b.Config.Validate.VirtualReadmeDir = func(projectPath string) string {
		root := b.Config.ReadmeRootFor(projectPath)
		if rel, relErr := filepath.Rel(b.Config.FuchsiaDir, root); relErr == nil && !strings.HasPrefix(rel, "..") {
			return filepath.ToSlash(rel)
		}
		return filepath.ToSlash(root)
	}

	return err
}

func (b *Builder) walkDir(baseDir string) error {
	return filepath.WalkDir(baseDir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			return nil
		}

		relPath, err := filepath.Rel(baseDir, path)
		if err != nil {
			return err
		}
		parts := strings.Split(relPath, string(os.PathSeparator))
		if len(parts) == 0 {
			return nil
		}

		// Backward compatibility logic for the "assets" directory structure
		category := parts[0]
		if category == "readmes" && filepath.Base(path) == "README.fuchsia" {
			if len(parts) > 1 {
				logicalParts := parts[1 : len(parts)-1]
				logicalPath := filepath.Clean(filepath.Join(logicalParts...))
				b.Config.Boundary.OutOfTreeReadmes[logicalPath] = path
			}
			return nil
		}

		if filepath.Ext(path) == ".json" {
			if filepath.Base(path) == "template.json" || filepath.Base(path) == "config.json" {
				return nil
			}
			return b.parseConfigFile(path)
		}

		return nil
	})
}

func (b *Builder) parseConfigFile(path string) error {
	path, err := filepath.Abs(path)
	if err != nil {
		return err
	}
	if b.seen[path] {
		return nil
	}
	b.seen[path] = true

	bytes, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	var f ConfigFile
	if err := json.Unmarshal(bytes, &f); err != nil {
		return fmt.Errorf("failed to parse config file %q: %w", path, err)
	}

	// 0. Process Includes
	for _, include := range f.Includes {
		absInclude := include
		if !filepath.IsAbs(include) {
			absInclude = filepath.Join(b.Config.FuchsiaDir, include)
		}

		info, err := os.Stat(absInclude)
		if err != nil {
			continue
		}

		if info.IsDir() {
			if err := b.walkDir(absInclude); err != nil {
				return err
			}
		} else {
			if err := b.parseConfigFile(absInclude); err != nil {
				return err
			}
		}
	}

	// 1. Process Skips
	for _, skip := range f.Skips {
		if skip.Bug == "" && filepath.Base(path) != "default.json" && filepath.Base(path) != "test_dirs.json" && filepath.Base(path) != "bazel_vendor.json" {
			return fmt.Errorf("validation error in %s: a 'bug' field is required to track this exception", path)
		}
		for _, p := range skip.Paths {
			if skip.SkipAnywhere {
				b.Config.Discover.SkipAnywhere[p] = true
			} else {
				b.Config.Discover.SkipPaths[p] = true
			}
		}
	}

	// 2. Process TargetExtensions
	if f.TargetExtensions != nil {
		b.Config.Classify.AddExtensions(f.TargetExtensions.Extensions)
	}

	// 2.5 Process CopyrightExtensions
	if f.CopyrightExtensions != nil {
		b.Config.Validate.AddCopyrightExtensions(f.CopyrightExtensions.Extensions)
	}

	// 3. Process Barriers
	for _, barrier := range f.Barriers {
		if barrier.Bug == "" && f.IsBugRequired(path) {
			return fmt.Errorf("validation error in %s: a 'bug' field is required to track this exception", path)
		}
		for _, p := range barrier.Paths {
			b.Config.Boundary.BarrierPaths[filepath.ToSlash(filepath.Clean(strings.TrimPrefix(p, "//")))] = true
		}
		for _, p := range barrier.Exceptions {
			b.Config.Boundary.BarrierExceptions[filepath.ToSlash(filepath.Clean(strings.TrimPrefix(p, "//")))] = true
		}
	}

	// 4. Process PolicyExceptions
	for checkName, entries := range f.PolicyExceptions {
		if _, exists := b.Config.Validate.PolicyExceptions[checkName]; !exists {
			b.Config.Validate.PolicyExceptions[checkName] = make(map[string]validate.RuleMetadata)
		}

		if err := b.addRuleException(b.Config.Validate.PolicyExceptions, checkName, entries, path, &f); err != nil {
			return err
		}
	}

	// 5. Process AllowedLicenses
	if b.Config.Classify.LicenseCategories == nil {
		b.Config.Classify.LicenseCategories = make(map[string]string)
	}
	for licenseName, entries := range f.AllowedLicenses {
		if _, exists := b.Config.Validate.AllowedLicenses[licenseName]; !exists {
			b.Config.Validate.AllowedLicenses[licenseName] = make(map[string]validate.RuleMetadata)
		}

		if _, known := b.Config.Classify.LicenseCategories[licenseName]; !known {
			slashPath := filepath.ToSlash(path)
			if idx := strings.Index(slashPath, "/allowed_licenses/"); idx != -1 {
				subParts := strings.Split(slashPath[idx+len("/allowed_licenses/"):], "/")
				if len(subParts) >= 2 {
					b.Config.Classify.LicenseCategories[licenseName] = subParts[0]
				}
			}
		}
		if err := b.addRuleException(b.Config.Validate.AllowedLicenses, licenseName, entries, path, &f); err != nil {
			return err
		}
	}

	return nil
}

func (b *Builder) addRuleException(targetMap map[string]map[string]RuleMetadata, key string, entries []AllowlistEntry, path string, f *ConfigFile) error {
	if _, exists := targetMap[key]; !exists {
		targetMap[key] = make(map[string]RuleMetadata)
	}
	bugRequired := f.IsBugRequired(path)
	descRequired := f.IsDescriptionRequired()
	for _, entry := range entries {
		if entry.Bug == "" && bugRequired {
			return fmt.Errorf("validation error in %s: a 'bug' field is required to track this exception", path)
		}
		if entry.Description == "" && descRequired {
			return fmt.Errorf("validation error in %s: a 'description' field is required to describe this exception", path)
		}
		for _, allowedPath := range entry.Paths {
			cleanPath := normalizeProjectPath(allowedPath)
			meta := RuleMetadata{
				Bug:         entry.Bug,
				Description: entry.Description,
				ConfigPath:  path,
			}
			targetMap[key][cleanPath] = meta
		}
	}
	return nil
}

// XML structures for Jiri manifests
type Manifest struct {
	XMLName         xml.Name  `xml:"manifest"`
	Projects        []Project `xml:"project"`
	ProjectsGrouped []Project `xml:"projects>project"`
	Packages        []Package `xml:"packages>package"`
}

type Project struct {
	Name string `xml:"name,attr"`
	Path string `xml:"path,attr"`
}

type Package struct {
	Name string `xml:"name,attr"`
	Path string `xml:"path,attr"`
}

// LoadManifests scans the manifests and integration directories and populates the mapping.
func (b *Builder) LoadManifests() error {
	dirsToScan := []string{
		filepath.Join(b.Config.FuchsiaDir, "manifests"),
		filepath.Join(b.Config.FuchsiaDir, "integration"),
	}

	for _, dir := range dirsToScan {
		if _, err := os.Stat(dir); os.IsNotExist(err) {
			continue
		}

		err := filepath.WalkDir(dir, func(path string, d fs.DirEntry, err error) error {
			if err != nil {
				return err
			}
			if d.IsDir() {
				return nil
			}

			// Manifest files usually have no extension or .xml
			ext := filepath.Ext(path)
			if ext != "" && ext != ".xml" {
				return nil
			}

			data, err := os.ReadFile(path)
			if err != nil {
				return nil // Skip files we can't read
			}

			var m Manifest
			if err := xml.Unmarshal(data, &m); err != nil {
				return nil // Skip files that aren't valid XML manifests
			}

			isPrivate := strings.Contains(filepath.ToSlash(path), "/internal")

			addEntry := func(path, name string) {
				if path != "" && name != "" {
					cleanPath := filepath.Clean(path)
					b.Config.Boundary.ManifestProjectNames[cleanPath] = name
					if isPrivate {
						b.Config.Boundary.ManifestPrivateProjects[cleanPath] = true
					}
				}
			}

			for _, p := range m.Projects {
				addEntry(p.Path, p.Name)
			}
			for _, p := range m.ProjectsGrouped {
				addEntry(p.Path, p.Name)
			}
			for _, p := range m.Packages {
				addEntry(p.Path, p.Name)
			}

			return nil
		})
		if err != nil {
			return err
		}
	}

	return nil
}

func normalizeProjectPath(p string) string {
	cleanPath := strings.TrimPrefix(p, "//")
	cleanPath = strings.TrimPrefix(cleanPath, "/")
	cleanPath = filepath.Clean(cleanPath)
	if cleanPath == "." {
		cleanPath = ""
	}
	return cleanPath
}
