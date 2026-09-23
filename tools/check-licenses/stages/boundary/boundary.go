// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package boundary

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/readme"
)

// Grouper implements pipeline.Grouper. It consumes a stream of RawPaths,
// buffers them, identifies project boundaries (via READMEs or Barriers),
// and emits grouped Project structs.
type Grouper struct {
	FuchsiaDir string
	Config     Config
}

// NewGrouper creates a new stateless boundary grouper.
func NewGrouper(fuchsiaDir string, config Config) *Grouper {
	absFuchsiaDir, err := filepath.Abs(fuchsiaDir)
	if err == nil {
		fuchsiaDir = absFuchsiaDir
	}
	fuchsiaDir = filepath.Clean(fuchsiaDir)

	return &Grouper{
		FuchsiaDir: fuchsiaDir,
		Config:     config,
	}
}

// Run buffers the incoming paths, determines their project boundaries, and emits the grouped projects.
func (g *Grouper) Run(ctx context.Context, in <-chan pipeline.RawPath) (<-chan pipeline.Project, error) {
	out := make(chan pipeline.Project, 100)

	go func() {
		defer close(out)

		var allFiles []string
		// physicalReadmes maps an absolute directory path to its README.fuchsia, go.mod, or Cargo.toml
		physicalReadmes := make(map[string][]string)

		// PHASE 1: Consume all incoming paths
		for rp := range in {
			if ctx.Err() != nil {
				return
			}
			if rp.IsDir {
				continue
			}

			cleanPath := filepath.Clean(rp.Path)
			allFiles = append(allFiles, cleanPath)

			base := filepath.Base(cleanPath)
			if base == "README.fuchsia" {
				dir := filepath.Dir(cleanPath)
				physicalReadmes[dir] = append(physicalReadmes[dir], cleanPath)
			} else if base == "go.mod" || base == "Cargo.toml" || base == "pubspec.yaml" {
				// Only treat package manifests as project boundaries if they reside in third_party or vendor,
				// are not nested inside a prebuilt directory, and are not sub-packages.
				if !readme.IsManifestSubpackage(cleanPath, g.FuchsiaDir) {
					absDir := filepath.Dir(cleanPath)
					physicalReadmes[absDir] = append(physicalReadmes[absDir], cleanPath)
				}
			}
		}

		// Incorporate Virtual (Out-Of-Tree) READMEs from Config.
		// Out-of-tree READMEs take priority over physical in-tree READMEs, but preserve package manifests (e.g. go.mod).
		for logicalPath, physicalPath := range g.Config.OutOfTreeReadmes {
			absLogicalDir := filepath.Join(g.FuchsiaDir, logicalPath)
			var manifests []string
			for _, p := range physicalReadmes[absLogicalDir] {
				if filepath.Base(p) != "README.fuchsia" {
					manifests = append(manifests, p)
				}
			}
			physicalReadmes[absLogicalDir] = append(manifests, physicalPath)
		}
		if _, hasRoot := physicalReadmes[g.FuchsiaDir]; !hasRoot {
			rootVirtual := filepath.Join(g.FuchsiaDir, "tools/check-licenses/assets/readmes/README.fuchsia")
			if _, err := os.Stat(rootVirtual); err == nil {
				physicalReadmes[g.FuchsiaDir] = append(physicalReadmes[g.FuchsiaDir], rootVirtual)
			}
		}

		// PHASE 2: Parse all READMEs to establish exact project boundaries
		// projectRoots maps a boundary directory to its parsed Readme structs (handling DEPENDENCY DIVIDER)
		projectRoots := make(map[string][]*readme.Readme)

		// First, identify all directories that have a physical or virtual README.fuchsia
		readmeDirs := make(map[string]bool)
		for dir, readmePaths := range physicalReadmes {
			for _, p := range readmePaths {
				if filepath.Base(p) == "README.fuchsia" {
					readmeDirs[dir] = true
					break
				}
			}
		}

		// Register project boundaries: README.fuchsia always registers. Package manifests (Cargo.toml,
		// pubspec.yaml, go.mod) only register if no ancestor directory already has a README.fuchsia.
		for dir, readmePaths := range physicalReadmes {
			isManifestOnly := true
			for _, p := range readmePaths {
				if filepath.Base(p) == "README.fuchsia" {
					isManifestOnly = false
					break
				}
			}

			if isManifestOnly {
				hasReadmeAncestor := false
				for parent := filepath.Dir(dir); parent != "." && parent != "/" && parent != dir; parent = filepath.Dir(parent) {
					if parent == g.FuchsiaDir {
						continue
					}
					if g.IsBarrier(parent) {
						break
					}
					if readmeDirs[parent] {
						hasReadmeAncestor = true
						break
					}
				}
				if hasReadmeAncestor {
					continue
				}
			}

			isException := g.IsBarrierException(dir)

			for _, readmePath := range readmePaths {
				rootReadmes, subReadmes, err := readme.ParseAnyMetadata(readmePath)

				if err != nil || (len(rootReadmes) == 0 && len(subReadmes) == 0) {
					// Even if parsing fails, the file exists, so it is a boundary (unless dir is a barrier exception container)
					if !isException {
						if _, exists := projectRoots[dir]; !exists {
							projectRoots[dir] = nil
						}
					}
					continue
				}

				if len(rootReadmes) > 0 && !isException {
					projectRoots[dir] = append(projectRoots[dir], rootReadmes...)
				}

				for _, subReadme := range subReadmes {
					if subReadme.Location != "" && subReadme.Location != "." {
						absSubProjectDir := filepath.Join(dir, subReadme.Location)

						// It is possible multiple sub-projects share a directory. We append them.
						projectRoots[absSubProjectDir] = append(projectRoots[absSubProjectDir], subReadme)
					}
				}
			}
		}

		// Sort to ensure deterministic grouping
		sort.Strings(allFiles)

		// PHASE 3: Group files by their closest project root
		projects := make(map[string]*pipeline.Project)

		for _, file := range allFiles {
			if ctx.Err() != nil {
				return
			}

			root := g.findProjectRoot(file, projectRoots)

			if _, exists := projects[root]; !exists {
				relRoot, _ := filepath.Rel(g.FuchsiaDir, root)
				proj := &pipeline.Project{
					RootPath:     root,
					Files:        []pipeline.FileInfo{},
					ManifestName: g.Config.ManifestNameFor(relRoot),
					IsPrivate:    g.Config.IsPrivateProject(relRoot),
				}
				readmePath := filepath.Join(root, "README.fuchsia")
				if rPaths, ok := physicalReadmes[root]; ok && len(rPaths) > 0 {
					readmePath = rPaths[0]
				}
				if readmes, ok := projectRoots[root]; ok && len(readmes) > 0 {
					var segs []*pipeline.ReadmeSegment
					for _, r := range readmes {
						if r != nil {
							clone := *r
							segs = append(segs, &pipeline.ReadmeSegment{
								Original: r,
								Updated:  &clone,
							})
						}
					}
					if len(segs) > 0 {
						proj.Readme = &pipeline.ReadmeFile{
							Path:     readmePath,
							Segments: segs,
						}
					}
				}
				projects[root] = proj
			}

			// Determine if this specific file needs a custom parser based on the parsed Readmes at this root
			parser := ""
			listedInReadme := false
			isNonLicense := false
			isLicenseFile := false
			if readmes, ok := projectRoots[root]; ok {
				// Check all Readme structs registered at this boundary (handles sub-projects)
				relToReadme, _ := filepath.Rel(root, file)
				relToFuchsia, _ := filepath.Rel(g.FuchsiaDir, file)

				for _, r := range readmes {
					for _, lf := range r.LicenseFiles {
						cleanLF := filepath.Clean(lf)
						if strings.HasPrefix(cleanLF, "..") || filepath.IsAbs(lf) {
							// External license files pointing outside the project root are disallowed.
							continue
						}
						if cleanLF == relToReadme || cleanLF == relToFuchsia {
							listedInReadme = true
							isLicenseFile = true
							break
						}
					}
					if listedInReadme {
						break
					}
					for _, gnf := range r.GeneratedNoticeFiles {
						cleanGNF := filepath.Clean(gnf)
						if cleanGNF == relToReadme || cleanGNF == relToFuchsia {
							listedInReadme = true
							isLicenseFile = true
							break
						}
						if proj, ok := projects[root]; ok && proj.Readme != nil && proj.Readme.Path != "" {
							relToReadmeDir, _ := filepath.Rel(filepath.Dir(proj.Readme.Path), file)
							if cleanGNF == relToReadmeDir {
								listedInReadme = true
								isLicenseFile = true
								break
							}
						}
					}
					if listedInReadme {
						break
					}
					for _, nlf := range r.NonLicenseFiles {
						cleanNLF := filepath.Clean(nlf)
						if cleanNLF == relToReadme || cleanNLF == relToFuchsia {
							listedInReadme = true
							isNonLicense = true
							break
						}
					}
					if listedInReadme {
						break
					}
				}
			}

			if g.Config.FilesInReadmeOnly && !listedInReadme {
				continue
			}

			projects[root].Files = append(projects[root].Files, pipeline.FileInfo{
				Path:          file,
				LicenseParser: parser,
				IsNonLicense:  isNonLicense,
				IsLicenseFile: isLicenseFile,
			})
		}

		// PHASE 4: Emit the projects downstream in deterministic order
		var roots []string
		for root := range projects {
			roots = append(roots, root)
		}
		sort.Strings(roots)
		for _, root := range roots {
			select {
			case <-ctx.Done():
				return
			case out <- *projects[root]:
			}
		}
	}()

	return out, nil
}

// findProjectRoot walks up the directory tree from the file to find the closest
// registered project boundary (from README.fuchsia or package manifests) or barrier root.
func (g *Grouper) findProjectRoot(filePath string, projectRoots map[string][]*readme.Readme) string {
	cleanPath := filepath.Clean(filePath)
	dir := cleanPath
	if _, isBoundary := projectRoots[cleanPath]; !isBoundary {
		dir = filepath.Dir(cleanPath)
	}
	var barrierChild string

	for {
		// Is this directory a registered project boundary?
		if _, isBoundary := projectRoots[dir]; isBoundary {
			if barrierChild != "" {
				return barrierChild
			}
			return dir
		}

		parent := filepath.Dir(dir)
		if g.IsBarrierChild(dir, parent) && barrierChild == "" {
			barrierChild = dir
		}

		if parent == dir || parent == "." || parent == "/" {
			break
		}
		dir = parent
	}

	if barrierChild != "" {
		return barrierChild
	}

	// Fallback to the workspace root if no boundaries exist
	return g.FuchsiaDir
}

func (g *Grouper) relPath(dir string) (string, bool) {
	if strings.HasPrefix(dir, "//") {
		dir = filepath.Join(g.FuchsiaDir, strings.TrimPrefix(dir, "//"))
	} else if !filepath.IsAbs(dir) {
		dir = filepath.Join(g.FuchsiaDir, dir)
	}
	rel, err := filepath.Rel(g.FuchsiaDir, dir)
	if err != nil {
		return "", false
	}
	return filepath.ToSlash(rel), true
}

// IsBarrier checks if the given directory matches a top-level defined barrier path (e.g. //third_party).
func (g *Grouper) IsBarrier(dir string) bool {
	rel, ok := g.relPath(dir)
	return ok && g.Config.BarrierPaths[rel]
}

// IsBarrierException checks if the given directory is exempted from barrier boundaries.
func (g *Grouper) IsBarrierException(dir string) bool {
	rel, ok := g.relPath(dir)
	return ok && g.Config.BarrierExceptions[rel]
}

// IsBarrierChild returns true if parent is a barrier and dir is not an exempted barrier child.
func (g *Grouper) IsBarrierChild(dir, parent string) bool {
	return g.IsBarrier(parent) && !g.IsBarrierException(dir)
}

// FindProjectReadme walks up the directory tree from targetPath to find the closest
// physical README.fuchsia or virtual out-of-tree README, stopping at defined barriers.
func (g *Grouper) FindProjectReadme(targetPath string) (*readme.Readme, string, error) {
	absTarget := targetPath
	if strings.HasPrefix(absTarget, "//") {
		absTarget = filepath.Join(g.FuchsiaDir, strings.TrimPrefix(absTarget, "//"))
	} else if !filepath.IsAbs(absTarget) {
		absTarget = filepath.Join(g.FuchsiaDir, absTarget)
	}
	absTarget = filepath.Clean(absTarget)

	var dir string
	if stat, err := os.Stat(absTarget); err == nil && stat.IsDir() {
		dir = absTarget
	} else {
		dir = filepath.Dir(absTarget)
	}

	var barrierChild string

	for {
		isBoundary, bestPath, allReadmes, _ := readme.IsProjectBoundary(dir, g.FuchsiaDir, g.Config.OutOfTreeReadmes)
		if isBoundary {
			r := readme.MatchReadme(allReadmes, bestPath, absTarget, g.FuchsiaDir, g.Config.OutOfTreeReadmes)
			if r != nil {
				resolved := readme.ResolveProjectRoot(r, bestPath, g.FuchsiaDir, g.Config.OutOfTreeReadmes)
				if barrierChild == "" || strings.HasPrefix(resolved, barrierChild+"/") || resolved == barrierChild {
					return r, bestPath, nil
				}
			}
			if barrierChild != "" {
				// Crossed a barrier earlier (e.g. third_party), so an ancestor boundary
				// (such as the root virtual README at FuchsiaDir) cannot be inherited.
				return nil, "", nil
			}
			if !g.IsBarrierException(dir) {
				return nil, "", fmt.Errorf("boundary metadata failed to parse")
			}
		}

		parent := filepath.Dir(dir)
		if g.IsBarrierChild(dir, parent) && barrierChild == "" {
			barrierChild = dir
		}

		if dir == g.FuchsiaDir || parent == dir || parent == "." || parent == "/" {
			break
		}
		dir = parent
	}

	return nil, "", nil
}

// ResolveProjectRoot resolves the governing logical project root directory for a given target path.
// It respects registered project boundaries (README.fuchsia or package manifests) and barriers.
func (g *Grouper) ResolveProjectRoot(targetPath string) string {
	absTarget := targetPath
	if strings.HasPrefix(absTarget, "//") {
		absTarget = filepath.Join(g.FuchsiaDir, strings.TrimPrefix(absTarget, "//"))
	} else if !filepath.IsAbs(absTarget) {
		absTarget = filepath.Join(g.FuchsiaDir, absTarget)
	}
	absTarget = filepath.Clean(absTarget)

	var dir string
	if stat, err := os.Stat(absTarget); err == nil && stat.IsDir() {
		dir = absTarget
	} else {
		dir = filepath.Dir(absTarget)
	}

	var barrierChild string

	for {
		isBoundary, bestPath, allReadmes, _ := readme.IsProjectBoundary(dir, g.FuchsiaDir, g.Config.OutOfTreeReadmes)
		if isBoundary {
			if len(allReadmes) > 0 {
				r := readme.MatchReadme(allReadmes, bestPath, absTarget, g.FuchsiaDir, g.Config.OutOfTreeReadmes)
				if r != nil {
					resolved := readme.ResolveProjectRoot(r, bestPath, g.FuchsiaDir, g.Config.OutOfTreeReadmes)
					if barrierChild == "" || strings.HasPrefix(resolved, barrierChild+"/") || resolved == barrierChild {
						return resolved
					}
				}
			}
			if barrierChild != "" {
				return barrierChild
			}
			if !g.IsBarrierException(dir) {
				return dir
			}
		}

		parent := filepath.Dir(dir)
		if g.IsBarrierChild(dir, parent) && barrierChild == "" {
			barrierChild = dir
		}

		if dir == g.FuchsiaDir || parent == dir || parent == "." || parent == "/" {
			break
		}
		dir = parent
	}

	if barrierChild != "" {
		return barrierChild
	}

	for p := absTarget; p != g.FuchsiaDir && p != filepath.Dir(p); p = filepath.Dir(p) {
		if g.IsBarrierException(p) {
			return g.FuchsiaDir
		}
	}

	if stat, err := os.Stat(absTarget); err == nil && stat.IsDir() {
		return absTarget
	}
	return filepath.Dir(absTarget)
}

// BelongsToProject returns true if targetPath belongs to projectRoot rather than a nested subproject.
func (g *Grouper) BelongsToProject(targetPath, projectRoot string) bool {
	resolvedRoot := g.ResolveProjectRoot(targetPath)
	absProjRoot := projectRoot
	if strings.HasPrefix(absProjRoot, "//") {
		absProjRoot = filepath.Join(g.FuchsiaDir, strings.TrimPrefix(absProjRoot, "//"))
	} else if !filepath.IsAbs(absProjRoot) {
		absProjRoot = filepath.Join(g.FuchsiaDir, absProjRoot)
	}
	absProjRoot = filepath.Clean(absProjRoot)
	resolvedRoot = filepath.Clean(resolvedRoot)

	if resolvedRoot == absProjRoot {
		return true
	}

	relResolved, err1 := filepath.Rel(g.FuchsiaDir, resolvedRoot)
	relProjRoot, err2 := filepath.Rel(g.FuchsiaDir, absProjRoot)
	if err1 == nil && err2 == nil {
		if relResolved == "." {
			relResolved = ""
		}
		if relProjRoot == "." {
			relProjRoot = ""
		}
		return filepath.ToSlash(relResolved) == filepath.ToSlash(relProjRoot)
	}
	return false
}
