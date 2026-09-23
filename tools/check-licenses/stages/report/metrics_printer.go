// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package report

import (
	"context"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"time"

	"go.fuchsia.dev/fuchsia/tools/check-licenses/metrics"
	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
)

// SummaryMetricsPrinter prints the execution summary table.
type SummaryMetricsPrinter struct {
	FuchsiaDir string
	StartTime  time.Time
}

// NewSummaryMetricsPrinter creates a new SummaryMetricsPrinter.
func NewSummaryMetricsPrinter(fuchsiaDir string, startTime time.Time) *SummaryMetricsPrinter {
	return &SummaryMetricsPrinter{
		FuchsiaDir: fuchsiaDir,
		StartTime:  startTime,
	}
}

func (p *SummaryMetricsPrinter) Run(ctx context.Context, projects []*pipeline.Project, errors []pipeline.ComplianceError) error {
	duration := time.Since(p.StartTime)
	for _, proj := range projects {
		relProj, _ := filepath.Rel(p.FuchsiaDir, proj.RootPath)
		fmt.Printf("\nUpdate Summary for %s\n", relProj)
		fmt.Printf("----------------------------------------------------\n")
		fmt.Printf("Files discovered in project: %d\n", len(proj.Files))
		fmt.Printf("Files classified:            %d\n", len(proj.ClassifiedFiles))
		fmt.Printf("Files containing licenses:   %d\n", len(proj.FoundLicenses()))
		fmt.Printf("Total execution time:        %v\n", duration)
	}
	return nil
}

// MetricsRenderer prints the global pipeline execution metrics table and exports metrics.json.
type MetricsRenderer struct {
	OutDir string
}

// NewMetricsRenderer creates a new MetricsRenderer.
func NewMetricsRenderer(outDir string) *MetricsRenderer {
	return &MetricsRenderer{
		OutDir: outDir,
	}
}

func (m *MetricsRenderer) Run(ctx context.Context, projects []*pipeline.Project, errors []pipeline.ComplianceError) error {
	log.Println("\n[check-licenses] Execution Summary")
	log.Println("----------------------------------")
	log.Printf("Total Wall Time:                  %v\n", metrics.TotalRuntime.GetTotalDuration())
	log.Printf("Time spent in GN Filter:          %v\n", metrics.FilterDuration.GetTotalDuration())
	log.Printf("Wall time spent in Classifier:    %v\n", metrics.AnalyzeDuration.GetTotalDuration())
	log.Printf("Thread time spent in Classifier:  %v\n", metrics.ClassifierDuration.GetTotalDuration())

	totalFiles, _ := metrics.TotalFilesProcessed.GetCount()
	licenseFiles, _ := metrics.LicenseFilesFound.GetCount()
	sourceFilesWithLic, _ := metrics.SourceFilesWithLicenses.GetCount()

	log.Printf("Total Files Processed:            %d\n", totalFiles)
	log.Printf("License Files Found:              %d\n", licenseFiles)
	log.Printf("Source Files with Licenses:       %d\n", sourceFilesWithLic)

	projectsAnalyzed, err := metrics.ProjectsProcessed.GetCount("kept_by_gn")
	if err != nil {
		projectsAnalyzed = 0
	}
	log.Printf("Projects Analyzed:         %d\n", projectsAnalyzed)

	rawTexts, err := metrics.LicenseDeduplication.GetCount("raw_texts")
	if err != nil {
		rawTexts = 0
	}
	uniqueTexts, err := metrics.LicenseDeduplication.GetCount("unique_texts")
	if err != nil {
		uniqueTexts = 0
	}
	compression := 0.0
	if rawTexts > 0 {
		compression = float64(rawTexts-uniqueTexts) / float64(rawTexts) * 100.0
	}
	log.Printf("Licenses Deduplicated:     %.1f%% compression (%d raw -> %d unique)\n", compression, rawTexts, uniqueTexts)

	var validationErrors int64 = 0
	var allowlistHits int64 = 0
	for _, e := range errors {
		vErr, _ := metrics.ValidationErrors.GetCount(e.CheckName)
		validationErrors += vErr
		aHits, _ := metrics.AllowlistHits.GetCount(e.CheckName)
		allowlistHits += aHits
	}
	log.Printf("Validation Errors:         %d (%d Hidden by Allowlist)\n", validationErrors, allowlistHits)

	if m.OutDir != "" {
		if err := os.MkdirAll(m.OutDir, 0755); err != nil {
			log.Printf("Failed to create outDir for metrics: %v\n", err)
		} else {
			metricsExportPath := filepath.Join(m.OutDir, "metrics.json")
			if err := metrics.Export(metricsExportPath); err != nil {
				log.Printf("Failed to export metrics to JSON: %v\n", err)
			} else {
				log.Printf("\nExported full metrics to:  %s\n", metricsExportPath)
			}
		}
	}

	return nil
}
