// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// FindProjectReadme walks up the directory tree from absPath to find the closest
// physical README.fuchsia file or a matching virtual out-of-tree README.
// It also matches the specific file to the correct sub-project (DEPENDENCY DIVIDER)
// defined within that README.
func FindProjectReadme(absPath, fuchsiaDir string, outOfTreeReadmes map[string]string) (*Readme, string, error) {
	absPath, err := filepath.Abs(absPath)
	if err != nil {
		return nil, "", err
	}

	var dir string
	if stat, err := os.Stat(absPath); err == nil && stat.IsDir() {
		dir = absPath
	} else {
		dir = filepath.Dir(absPath)
	}

	// Special rule for Rust mirrors: boundary is always the top-level folder under mirrors/
	mirrorsPath := filepath.Join(fuchsiaDir, "third_party/rust_crates/mirrors")
	inMirrors := false
	if strings.HasPrefix(dir, mirrorsPath) {
		rel, err := filepath.Rel(mirrorsPath, dir)
		if err == nil && rel != "." {
			parts := strings.Split(rel, string(filepath.Separator))
			if len(parts) > 0 {
				dir = filepath.Join(mirrorsPath, parts[0])
				inMirrors = true
			}
		}
	}

	for {
		isBoundary, bestPath, allReadmes, err := IsProjectBoundary(dir, fuchsiaDir, outOfTreeReadmes)
		if err != nil {
			fmt.Printf("[Locator] Error checking boundary in %s: %v\n", dir, err)
		}

		if isBoundary {
			bestMatch := MatchReadme(allReadmes, bestPath, absPath, fuchsiaDir, outOfTreeReadmes)
			if bestMatch != nil {
				return bestMatch, bestPath, nil
			}
		}

		parent := filepath.Dir(dir)

		// Check if we've reached the repository root or the filesystem root
		if dir == fuchsiaDir || parent == dir || dir == "." || dir == "/" {
			break
		}

		if inMirrors {
			break // Don't walk up for mirrors!
		}
		dir = parent
	}

	return nil, "", nil
}

// IsProjectBoundary returns true if the given directory marks the start of a project.
// It also returns the path to the boundary file and the parsed Readme structs.
func IsProjectBoundary(dir, fuchsiaDir string, outOfTreeReadmes map[string]string) (bool, string, []*Readme, error) {
	// Special rule for Rust mirrors: boundary is always the top-level folder under mirrors/
	mirrorsPath := filepath.Join(fuchsiaDir, "third_party/rust_crates/mirrors")
	if strings.HasPrefix(dir, mirrorsPath) {
		rel, err := filepath.Rel(mirrorsPath, dir)
		if err == nil && rel != "." {
			parts := strings.Split(rel, string(filepath.Separator))
			if len(parts) > 0 {
				projectDir := filepath.Join(mirrorsPath, parts[0])
				if dir != projectDir {
					return false, "", nil, nil // Not the boundary!
				}
			}
		}
	}

	var foundReadmePaths []string

	// Check virtual
	relDir, err := filepath.Rel(fuchsiaDir, dir)
	if err == nil {
		if virtualPath, ok := outOfTreeReadmes[relDir]; ok {
			foundReadmePaths = append(foundReadmePaths, virtualPath)
		} else if relDir == "." || relDir == "" {
			if virtualPath, ok := outOfTreeReadmes["."]; ok {
				foundReadmePaths = append(foundReadmePaths, virtualPath)
			} else if virtualPath, ok := outOfTreeReadmes[""]; ok {
				foundReadmePaths = append(foundReadmePaths, virtualPath)
			}
		}
	}
	if len(foundReadmePaths) == 0 && (dir == fuchsiaDir || relDir == "." || relDir == "") {
		rootVirtual := filepath.Join(fuchsiaDir, "tools/check-licenses/assets/readmes/README.fuchsia")
		if _, err := os.Stat(rootVirtual); err == nil {
			foundReadmePaths = append(foundReadmePaths, rootVirtual)
		}
	}

	// Check physical README.fuchsia
	physReadme := filepath.Join(dir, "README.fuchsia")
	if _, err := os.Stat(physReadme); err == nil {
		foundReadmePaths = append(foundReadmePaths, physReadme)
	}

	// If neither virtual nor physical README.fuchsia exists, check fallback manifests
	if len(foundReadmePaths) == 0 {
		for _, name := range []string{"go.mod", "Cargo.toml", "pubspec.yaml"} {
			possiblePath := filepath.Join(dir, name)
			if _, err := os.Stat(possiblePath); err == nil {
				if !IsManifestSubpackage(possiblePath, fuchsiaDir) {
					foundReadmePaths = append(foundReadmePaths, possiblePath)
					break
				}
			}
		}
	}

	var readmeCount int
	for _, p := range foundReadmePaths {
		if filepath.Base(p) == "README.fuchsia" {
			readmeCount++
		}
	}
	if readmeCount > 1 {
		// If both virtual and physical README.fuchsia exist, log a warning
		var b strings.Builder
		b.WriteString(fmt.Sprintf("⚠️ Warning, project %s has multiple READMEs:\n", relDir))
		for _, p := range foundReadmePaths {
			if filepath.Base(p) == "README.fuchsia" {
				kind := "physical"
				if strings.Contains(p, "assets") {
					kind = "virtual "
				}
				b.WriteString(fmt.Sprintf("  * %s: %s\n", kind, p))
			}
		}
		b.WriteString("Out-of-tree asset README will take priority.\n")
		fmt.Fprint(os.Stderr, b.String())
	}

	if len(foundReadmePaths) > 0 {
		// Out-of-tree virtual README is added first and takes priority.
		bestPath := foundReadmePaths[0]
		rootReadmes, subReadmes, parseErr := ParseAnyMetadata(bestPath)
		if parseErr == nil {
			var allReadmes []*Readme
			allReadmes = append(allReadmes, rootReadmes...)
			allReadmes = append(allReadmes, subReadmes...)
			if len(allReadmes) > 0 {
				return true, bestPath, allReadmes, nil
			}
		}
	}

	return false, "", nil, nil
}

// ResolveProjectRoot returns the governing logical project root directory for a discovered Readme and README file path.
func ResolveProjectRoot(r *Readme, readmePath, fuchsiaDir string, outOfTreeReadmes map[string]string) string {
	logicalRoot := filepath.Dir(readmePath)
	for logPath, physPath := range outOfTreeReadmes {
		if physPath == readmePath {
			logicalRoot = filepath.Join(fuchsiaDir, logPath)
			break
		}
	}
	if strings.HasSuffix(filepath.ToSlash(readmePath), "assets/readmes/README.fuchsia") {
		logicalRoot = fuchsiaDir
	}
	if r != nil && r.Location != "" && r.Location != "." {
		logicalRoot = filepath.Join(logicalRoot, r.Location)
	}
	return logicalRoot
}

// MatchReadme matches a specific file or directory path (absPath) against a slice of Readmes discovered for bestPath.
func MatchReadme(allReadmes []*Readme, bestPath, absPath, fuchsiaDir string, outOfTreeReadmes map[string]string) *Readme {
	if len(allReadmes) == 0 {
		return nil
	}
	var bestMatch *Readme
	bestPrefixLength := -1

	// Path of the file relative to the README's logical directory
	logicalDir := filepath.Dir(bestPath)
	for logPath, physPath := range outOfTreeReadmes {
		if physPath == bestPath {
			logicalDir = filepath.Join(fuchsiaDir, logPath)
			break
		}
	}
	if strings.HasSuffix(filepath.ToSlash(bestPath), "assets/readmes/README.fuchsia") {
		logicalDir = fuchsiaDir
	}

	relToFile, relErr := filepath.Rel(logicalDir, absPath)
	if relErr == nil {
		for _, r := range allReadmes {
			loc := filepath.Clean(r.Location)
			if loc == "" || loc == "." {
				if bestPrefixLength < 0 {
					bestMatch = r
					bestPrefixLength = 0
				}
			} else {
				if strings.HasPrefix(relToFile, loc+"/") || relToFile == loc {
					if len(loc) > bestPrefixLength {
						bestMatch = r
						bestPrefixLength = len(loc)
					}
				}
			}
		}
	}

	if bestMatch != nil {
		return bestMatch
	}

	// Fallback to the first parsed readme if no best match found and it represents the root directory.
	if len(allReadmes) > 0 {
		loc := filepath.Clean(allReadmes[0].Location)
		if loc == "" || loc == "." {
			return allReadmes[0]
		}
	}
	return nil
}

// IsManifestSubpackage returns true if the manifest resides in a known subpackage directory
// (such as tests, benchmarks, examples, doc, or debug extensions) rather than marking an independent project boundary.
func IsManifestSubpackage(manifestPath, fuchsiaDir string) bool {
	relPath, err := filepath.Rel(fuchsiaDir, manifestPath)
	if err != nil {
		return false
	}
	slashRel := filepath.ToSlash(relPath)
	slash := "/" + slashRel
	isThirdParty := strings.Contains(slash, "/third_party/") || strings.Contains(slash, "/vendor/")
	isPrebuilt := strings.Contains(slash, "/prebuilt/")
	if !isThirdParty || isPrebuilt {
		return true
	}

	if strings.HasPrefix(slashRel, "third_party/go/src/") {
		return true
	}

	dir := filepath.Dir(slashRel)
	parts := strings.Split(dir, "/")
	minDepth := 1
	if strings.HasPrefix(slashRel, "third_party/dart-pkg/pub/") ||
		strings.HasPrefix(slashRel, "third_party/rust_crates/vendor/") ||
		strings.HasPrefix(slashRel, "third_party/rust_crates/forks/") {
		minDepth = 3
	}
	for i, part := range parts {
		if i > minDepth {
			switch part {
			case "example", "examples", "benchmark", "benchmarks", "test", "tests",
				"interop", "debug_extension", "debug_extension_mv3", "doc", "docs", "tools", "misc":
				return true
			}
		}
	}

	if strings.HasPrefix(slashRel, "third_party/rust_crates/mirrors/") {
		if len(parts) > 4 { // third_party / rust_crates / mirrors / <repo> (4 parts)
			return true
		}
	}

	return false
}
