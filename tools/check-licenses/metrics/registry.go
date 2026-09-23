// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package metrics

import (
	"encoding/json"
	"os"
	"sync"
)

var (
	// Global registry holding all our metrics
	registry = &Registry{
		Counters:  make(map[string]*Counter),
		Timers:    make(map[string]*Timer),
		Artifacts: make(map[string][]byte),
	}
)

// Registry holds the thread-safe maps of all registered metrics.
type Registry struct {
	mu        sync.RWMutex
	Counters  map[string]*Counter `json:"counters"`
	Timers    map[string]*Timer   `json:"timers"`
	Artifacts map[string][]byte   `json:"-"` // Excluded from JSON, saved separately
}

// Snapshot structs for safe JSON marshaling
type RegistrySnapshot struct {
	Counters map[string]CounterSnapshot `json:"counters"`
	Timers   map[string]TimerSnapshot   `json:"timers"`
}

type CounterSnapshot struct {
	Name        string           `json:"name"`
	Description string           `json:"description"`
	LabelKeys   []string         `json:"label_keys"`
	Counts      map[string]int64 `json:"counts"`
}

type TimerSnapshot struct {
	Name          string `json:"name"`
	Description   string `json:"description"`
	TotalDuration int64  `json:"total_duration"`
	CallCount     int64  `json:"call_count"`
	MaxDuration   int64  `json:"max_duration"`
}

// Export dumps the entire registry to a JSON file.
func Export(filepath string) error {
	registry.mu.RLock()
	snap := RegistrySnapshot{
		Counters: make(map[string]CounterSnapshot, len(registry.Counters)),
		Timers:   make(map[string]TimerSnapshot, len(registry.Timers)),
	}

	for k, c := range registry.Counters {
		c.mu.RLock()
		countsCopy := make(map[string]int64, len(c.Counts))
		for l, v := range c.Counts {
			countsCopy[l] = v
		}
		snap.Counters[k] = CounterSnapshot{
			Name:        c.Name,
			Description: c.Description,
			LabelKeys:   c.LabelKeys,
			Counts:      countsCopy,
		}
		c.mu.RUnlock()
	}

	for k, t := range registry.Timers {
		t.mu.RLock()
		snap.Timers[k] = TimerSnapshot{
			Name:          t.Name,
			Description:   t.Description,
			TotalDuration: int64(t.TotalDuration),
			CallCount:     t.CallCount,
			MaxDuration:   int64(t.MaxDuration),
		}
		t.mu.RUnlock()
	}
	registry.mu.RUnlock()

	b, err := json.MarshalIndent(snap, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(filepath, b, 0644)
}

// AddArtifact adds a raw file artifact to the registry.
func AddArtifact(key string, content []byte) {
	registry.mu.Lock()
	defer registry.mu.Unlock()
	registry.Artifacts[key] = content
}

// GetArtifacts returns a shallow copy of the artifacts map for saving to disk.
func GetArtifacts() map[string][]byte {
	registry.mu.RLock()
	defer registry.mu.RUnlock()

	m := make(map[string][]byte, len(registry.Artifacts))
	for k, v := range registry.Artifacts {
		m[k] = v
	}
	return m
}
