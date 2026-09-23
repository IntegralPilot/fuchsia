// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package metrics

import (
	"fmt"
	"strings"
	"sync"
)

// Counter tracks incrementing values partitioned by labels.
type Counter struct {
	Name        string   `json:"name"`
	Description string   `json:"description"`
	LabelKeys   []string `json:"label_keys"`

	mu     sync.RWMutex
	Counts map[string]int64 `json:"counts"` // Key is comma-separated label values
}

// RegisterCounter adds a new Counter to the global registry.
func RegisterCounter(name, description string, labelKeys ...string) *Counter {
	registry.mu.Lock()
	defer registry.mu.Unlock()

	c := &Counter{
		Name:        name,
		Description: description,
		LabelKeys:   labelKeys,
		Counts:      make(map[string]int64),
	}
	registry.Counters[name] = c
	return c
}

// Inc increments the counter for the given label values.
func (c *Counter) Inc(labelValues ...string) error {
	if len(labelValues) != len(c.LabelKeys) {
		return fmt.Errorf("metric %s expected %d labels, got %d", c.Name, len(c.LabelKeys), len(labelValues))
	}

	// Simple key concatenation (e.g., "MIT,rust,Approved")
	key := strings.Join(labelValues, ",")

	c.mu.Lock()
	defer c.mu.Unlock()
	c.Counts[key]++
	return nil
}

// GetCount returns the current count for the given label values.
func (c *Counter) GetCount(labelValues ...string) (int64, error) {
	if len(labelValues) != len(c.LabelKeys) {
		return 0, fmt.Errorf("metric %s expected %d labels, got %d", c.Name, len(c.LabelKeys), len(labelValues))
	}

	key := strings.Join(labelValues, ",")

	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.Counts[key], nil
}
