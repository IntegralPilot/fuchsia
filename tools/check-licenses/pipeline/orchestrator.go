// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package pipeline

import (
	"context"
	"fmt"
	"log"
)

// Orchestrator manages the execution and channel wiring of the 6-stage compliance pipeline.
type Orchestrator struct {
	Discoverer Discoverer
	Grouper    Grouper
	Pruner     Pruner
	Classifier Classifier
	Validator  Validator
	Renderer   Renderer
}

// NewOrchestrator creates a new pipeline orchestrator with the provided stage implementations.
func NewOrchestrator(d Discoverer, g Grouper, p Pruner, c Classifier, v Validator, r Renderer) *Orchestrator {
	return &Orchestrator{
		Discoverer: d,
		Grouper:    g,
		Pruner:     p,
		Classifier: c,
		Validator:  v,
		Renderer:   r,
	}
}

// Run executes the pipeline synchronously, wiring the channels between stages
// and waiting for the final Renderer stage to complete or fail.
func (o *Orchestrator) Run(ctx context.Context, rootDirs []string) error {
	if o.Discoverer == nil || o.Grouper == nil || o.Pruner == nil || o.Classifier == nil || o.Validator == nil || o.Renderer == nil {
		return fmt.Errorf("all 6 pipeline stages must be non-nil")
	}

	// Stage 1: Discover (Crawler)
	log.Printf("[Orchestrator] Starting Stage 1: Discovery (Crawling %v)...", rootDirs)
	rawPaths, err := o.Discoverer.Run(ctx, rootDirs)
	if err != nil {
		return fmt.Errorf("discovery stage failed: %w", err)
	}

	// Stage 2: Group (Project Boundary)
	log.Printf("[Orchestrator] Starting Stage 2: Boundary Resolution (Grouping files into projects)...")
	projectsChan, err := o.Grouper.Run(ctx, rawPaths)
	if err != nil {
		return fmt.Errorf("grouping stage failed: %w", err)
	}

	var allProjects []*Project
	projectsByRoot := make(map[string]*Project)
	for p := range projectsChan {
		pCopy := p
		allProjects = append(allProjects, &pCopy)
		projectsByRoot[p.RootPath] = &pCopy
	}

	// Stage 3: Prune (Build Graph Filter)
	log.Printf("[Orchestrator] Starting Stage 3: Pruning (Filtering projects by build graph)...")
	filteredProjectsChan := make(chan Project, len(allProjects))
	for _, p := range allProjects {
		filteredProjectsChan <- *p
	}
	close(filteredProjectsChan)

	filteredProjects, err := o.Pruner.Run(ctx, filteredProjectsChan)
	if err != nil {
		return fmt.Errorf("pruning stage failed: %w", err)
	}

	// Stage 4: Classify (License Identification)
	log.Printf("[Orchestrator] Starting Stage 4: Classification (Executing regex engines)...")
	classifiedFiles, err := o.Classifier.Run(ctx, filteredProjects)
	if err != nil {
		return fmt.Errorf("classification stage failed: %w", err)
	}

	// Stage 5: Validate (Policy Engine)
	log.Printf("[Orchestrator] Starting Stage 5: Validation (Checking policies)...")
	filesForValidator := make(chan ClassifiedFile, 100)

	go func() {
		defer close(filesForValidator)
		for f := range classifiedFiles {
			if proj, ok := projectsByRoot[f.ProjectRoot]; ok {
				proj.ClassifiedFiles = append(proj.ClassifiedFiles, f)
			}
			select {
			case <-ctx.Done():
				return
			case filesForValidator <- f:
			}
		}
	}()

	complianceErrors, err := o.Validator.Run(ctx, filesForValidator)
	if err != nil {
		return fmt.Errorf("validation stage failed to start: %w", err)
	}

	var allErrors []ComplianceError
	for e := range complianceErrors {
		allErrors = append(allErrors, e)
	}

	// Stage 6: Render (Reporting & Output Sinks)
	log.Printf("[Orchestrator] Starting Stage 6: Reporting (Executing renderers)...")
	if o.Renderer != nil {
		if err := o.Renderer.Run(ctx, allProjects, allErrors); err != nil {
			return err
		}
	}

	log.Printf("[Orchestrator] Pipeline execution completed successfully.")
	return nil
}
