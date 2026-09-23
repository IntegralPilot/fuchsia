// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package metrics

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sync"
	"testing"
)

func TestRegistry_Registration(t *testing.T) {
	c := RegisterCounter("registry_test_counter", "test", "l1")
	tm := RegisterTimer("registry_test_timer", "test")

	registry.mu.RLock()
	defer registry.mu.RUnlock()

	if registry.Counters["registry_test_counter"] != c {
		t.Errorf("counter not registered")
	}
	if registry.Timers["registry_test_timer"] != tm {
		t.Errorf("timer not registered")
	}
}

func TestRegistry_ArtifactsConcurrency(t *testing.T) {
	var wg sync.WaitGroup

	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			// Generate arbitrary key
			key := string(rune(idx))
			AddArtifact(key, []byte("data"))
		}(i)
	}

	wg.Wait()

	artifacts := GetArtifacts()
	if len(artifacts) < 100 {
		t.Errorf("expected 100 artifacts, got %d", len(artifacts))
	}
}

func TestRegistry_Export(t *testing.T) {
	_ = RegisterCounter("export_test_counter", "test", "l1").Inc("v1")
	RegisterTimer("export_test_timer", "test")
	tempFile := filepath.Join(t.TempDir(), "metrics.json")
	if err := Export(tempFile); err != nil {
		t.Fatalf("export failed: %v", err)
	}

	b, err := os.ReadFile(tempFile)
	if err != nil {
		t.Fatalf("failed to read exported file: %v", err)
	}

	var data map[string]interface{}
	if err := json.Unmarshal(b, &data); err != nil {
		t.Fatalf("failed to parse json: %v", err)
	}

	counters, ok := data["counters"].(map[string]interface{})
	if !ok || counters["export_test_counter"] == nil {
		t.Errorf("expected export_test_counter in json")
	}

	timers, ok := data["timers"].(map[string]interface{})
	if !ok || timers["export_test_timer"] == nil {
		t.Errorf("expected export_test_timer in json")
	}
}
