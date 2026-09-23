// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
// Package measurepower contains utility functions to invoke power measurements in infra.
package idletest

import (
	"bufio"
	"encoding/csv"
	"errors"
	"fmt"
	"io"
	"log"
	"math"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"

	"go.fuchsia.dev/fuchsia/src/lib/go-benchmarking"
)

const testSuite = "fuchsia.vim3_monsoon_idle"

// Welford's method to calculate variance in real time.
type RunningStats struct {
	n    float64
	mean float64
	m2   float64
}

func NewRunningStats() RunningStats {
	return RunningStats{}
}

func (rs *RunningStats) addData(x float64) {
	rs.n++
	delta := x - rs.mean
	rs.mean += delta / rs.n
	delta2 := x - rs.mean
	rs.m2 += delta * delta2
}

func (rs *RunningStats) variance() float64 {
	if rs.n < 2 {
		return math.NaN()
	}
	return rs.m2 / (rs.n - 1)
}

func (rs *RunningStats) standardDeviation() float64 {
	variance := rs.variance()
	if math.IsNaN(variance) {
		return math.NaN()
	}
	return math.Sqrt(variance)
}

type Timer struct {
	startTime time.Time
	stopTime  time.Time
}

func (t *Timer) start() {
	t.startTime = time.Now()
}

func (t *Timer) stop() {
	t.stopTime = time.Now()
}

func (t *Timer) elapsedTime() time.Duration {
	return t.stopTime.Sub(t.startTime)
}

func (t *Timer) elapsedMilliseconds() int64 {
	return t.elapsedTime().Milliseconds()
}

type PowerMeasurement struct {
	done             chan bool
	timeMetric       Timer
	cmd              *exec.Cmd
	csvPath          string
	jsonPath         string
	logger           *log.Logger
	measurementCount int
	currentSum       float64
	voltageSum       float64
	powerSum         float64
	currentVariance  RunningStats
	voltageVariance  RunningStats
	powerVariance    RunningStats
	minCurrent       float64
	maxCurrent       float64
	minVoltage       float64
	maxVoltage       float64
	minPower         float64
	maxPower         float64
}

func NewPowerMeasurement(name string) (*PowerMeasurement, error) {
	logger := log.New(os.Stderr, "", log.LstdFlags)
	env := os.Environ()
	measurePowerPath, err := findMeasurePowerPath(env)
	if err != nil {
		return nil, fmt.Errorf("findMeasurePowerPath: %s", err)
	}
	wd, err := os.Getwd()
	if err != nil {
		return nil, fmt.Errorf("os.Getwd: %v", err)
	}
	csvPath := filepath.Join(wd, "..", "..", "serial_logs", fmt.Sprintf("%s.csv", name))
	jsonPath := filepath.Join(wd, "..", "..", "serial_logs", fmt.Sprintf("%s.json", name))
	cmd := exec.Command(measurePowerPath, "-format", "csv")
	cmd.Env = env
	cmd.Stderr = os.Stderr
	pm := &PowerMeasurement{
		done:       make(chan bool),
		cmd:        cmd,
		csvPath:    csvPath,
		jsonPath:   jsonPath,
		logger:     logger,
		minCurrent: math.Inf(1),
		maxCurrent: math.Inf(-1),
		minVoltage: math.Inf(1),
		maxVoltage: math.Inf(-1),
		minPower:   math.Inf(1),
		maxPower:   math.Inf(-1),
	}
	if err := pm.start(); err != nil {
		return nil, fmt.Errorf("start: %s", err)
	}
	return pm, nil
}

func (pm *PowerMeasurement) start() error {
	if pm.cmd.Process != nil {
		return fmt.Errorf("Already started with PID %d", pm.cmd.Process.Pid)
	}
	deferCleanup := true
	stdout, err := pm.cmd.StdoutPipe()
	if err != nil {
		return fmt.Errorf("StdoutPipe: %v", err)
	}
	defer func() {
		if !deferCleanup {
			return
		}
		if err := stdout.Close(); err != nil {
			pm.logger.Printf("measurepower stdout.Close: %s", err)
		}
	}()
	outFile, err := os.Create(pm.csvPath)
	if err != nil {
		return fmt.Errorf("os.Create: %v", err)
	}
	defer func() {
		if !deferCleanup {
			return
		}
		if err := outFile.Close(); err != nil {
			pm.logger.Printf("meaaurepower outFile.Close: %s", err)
		}
	}()

	pm.timeMetric.start()

	if err := pm.cmd.Start(); err != nil {
		return fmt.Errorf("cmd.Start: %v", err)
	}
	if err := copyAndBlockFirstByte(stdout, outFile); err != nil {
		return fmt.Errorf("copyAndBlockFirstByte: %v", err)
	}
	deferCleanup = false
	go func() {
		defer stdout.Close()
		defer outFile.Close()
		if err := pm.backgroundCopy(stdout, outFile); err != nil {
			pm.logger.Printf("measurepower backgroundCopy error: %s", err)
		}
		pm.done <- true
		close(pm.done)
	}()
	return nil
}

func (pm *PowerMeasurement) Stop() error {
	if pm.cmd.Process == nil {
		return errors.New("not started")
	}
	if err := pm.cmd.Process.Signal(syscall.SIGINT); err != nil {
		return fmt.Errorf("Signal SIGINT: %s", err)
	}
	if err := pm.cmd.Wait(); err != nil {
		return fmt.Errorf("Wait: %s", err)
	}
	select {
	case <-pm.done:
		pm.timeMetric.stop()
		if err := pm.outputFile(pm.jsonPath); err != nil {
			pm.logger.Printf("outputFile %q creation error: %s", pm.jsonPath, err)
		} else {
			pm.logger.Printf("Wrote benchmark values to a file: %s", pm.jsonPath)
		}
		return nil
	case <-time.After(5 * time.Second):
		return errors.New("Backgroundcopy goroutine did not complete and return done signal within the specified time (5sec)")
	}
}

func (pm *PowerMeasurement) backgroundCopy(stdout io.ReadCloser, outFile *os.File) error {
	csvWriter := csv.NewWriter(outFile)
	pm.currentVariance = NewRunningStats()
	pm.voltageVariance = NewRunningStats()
	pm.powerVariance = NewRunningStats()
	defer csvWriter.Flush()

	scanner := bufio.NewScanner(stdout)

	skipLineProcess := true // Skips the CSV header 'Timestamp,Current,Voltage'

	for scanner.Scan() {
		parts, err := processLine(scanner)
		if err != nil {
			return err
		}

		if !skipLineProcess {
			if err := pm.updateValues(parts, csvWriter); err != nil {
				return err
			}
		}
		skipLineProcess = false

		if err := writeToCSV(csvWriter, parts); err != nil {
			return err
		}
	}
	return nil
}

func writeToCSV(csvWriter *csv.Writer, parts []string) error {
	if err := csvWriter.Write(parts); err != nil {
		return fmt.Errorf("Failed to write to CSV: %s", err)
	}

	if err := csvWriter.Error(); err != nil {
		return fmt.Errorf("CSV write error: %s", err)
	}

	return nil
}

func processLine(scanner *bufio.Scanner) ([]string, error) {
	line := scanner.Text()

	parts := strings.Split(line, ",")
	if len(parts) != 3 {
		return nil, fmt.Errorf("Invalid line format: %s", line)
	}

	return parts, nil
}

func (pm *PowerMeasurement) updateValues(parts []string, csvWriter *csv.Writer) error {

	current, err := strconv.ParseFloat(parts[1], 64)
	if err != nil {
		return fmt.Errorf("Failed to parse current: %s", err)
	}
	voltage, err := strconv.ParseFloat(parts[2], 64)
	if err != nil {
		return fmt.Errorf("Failed to parse voltage: %s", err)
	}
	power := current / 1000 * voltage

	pm.currentSum += current
	pm.voltageSum += voltage
	pm.powerSum += power

	pm.currentVariance.addData(current)
	pm.voltageVariance.addData(voltage)
	pm.powerVariance.addData(power)

	pm.measurementCount++

	if voltage < pm.minVoltage {
		pm.minVoltage = voltage
	}
	if voltage > pm.maxVoltage {
		pm.maxVoltage = voltage
	}

	if current < pm.minCurrent {
		pm.minCurrent = current
	}
	if current > pm.maxCurrent {
		pm.maxCurrent = current
	}

	if power < pm.minPower {
		pm.minPower = power
	}
	if power > pm.maxPower {
		pm.maxPower = power
	}

	return nil
}

func copyAndBlockFirstByte(stdout io.ReadCloser, outFile *os.File) error {
	buff := make([]byte, 1)
	if _, err := stdout.Read(buff); err != nil {
		// An EOF here is unexpected and an error.
		return fmt.Errorf("stdout.Read: %v", err)
	}
	if _, err := outFile.Write(buff); err != nil {
		return fmt.Errorf("outFile.Write: %v", err)
	}
	return nil
}

func findMeasurePowerPath(env []string) (string, error) {
	for _, e := range env {
		eSplit := strings.SplitN(e, "=", 2)
		if eSplit[0] == "MEASUREPOWER_PATH" {
			return eSplit[1], nil
		}
	}
	return "", fmt.Errorf("No MEASUREPOWER_PATH on env: %s", env)
}

func (pm *PowerMeasurement) generateTestResults() benchmarking.TestResultsFile {
	return benchmarking.TestResultsFile{
		{
			Label:     "Go/label/measurementCount",
			TestSuite: testSuite,
			Unit:      "Count",
			Values:    []float64{float64(pm.measurementCount)},
		},
		{
			Label:     "Go/label/elapsedMilliseconds",
			TestSuite: testSuite,
			Unit:      "Milliseconds",
			Values:    []float64{float64(pm.timeMetric.elapsedMilliseconds())},
		},
		{
			Label:     "Go/label/powerSum",
			TestSuite: testSuite,
			Unit:      "W Sum(W)",
			Values:    []float64{pm.powerSum},
		},
		{
			Label:     "Go/label/minVoltage",
			TestSuite: testSuite,
			Unit:      "V",
			Values:    []float64{pm.minVoltage},
		},
		{
			Label:     "Go/label/maxVoltage",
			TestSuite: testSuite,
			Unit:      "V",
			Values:    []float64{pm.maxVoltage},
		},
		{
			Label:     "Go/label/minCurrent",
			TestSuite: testSuite,
			Unit:      "mA",
			Values:    []float64{pm.minCurrent},
		},
		{
			Label:     "Go/label/maxCurrent",
			TestSuite: testSuite,
			Unit:      "mA",
			Values:    []float64{pm.maxCurrent},
		},
		{
			Label:     "Go/label/minPower",
			TestSuite: testSuite,
			Unit:      "Watt",
			Values:    []float64{pm.minPower},
		},
		{
			Label:     "Go/label/maxPower",
			TestSuite: testSuite,
			Unit:      "Watt",
			Values:    []float64{pm.maxPower},
		},
		{
			Label:     "Go/label/AverageVoltage",
			TestSuite: testSuite,
			Unit:      "V",
			Values:    []float64{pm.voltageSum / float64(pm.measurementCount)},
		},
		{
			Label:     "Go/label/AverageCurrent",
			TestSuite: testSuite,
			Unit:      "mA",
			Values:    []float64{pm.currentSum / float64(pm.measurementCount)},
		},
		{
			Label:     "Go/label/AveragePower",
			TestSuite: testSuite,
			Unit:      "Watt",
			Values:    []float64{pm.powerSum / float64(pm.measurementCount)},
		},
		{
			Label:     "Go/label/CurrentVariance",
			TestSuite: testSuite,
			Unit:      "mA²",
			Values:    []float64{pm.currentVariance.variance()},
		},
		{
			Label:     "Go/label/VoltageVariance",
			TestSuite: testSuite,
			Unit:      "V²",
			Values:    []float64{pm.voltageVariance.variance()},
		},
		{
			Label:     "Go/label/PowerVariance",
			TestSuite: testSuite,
			Unit:      "W²",
			Values:    []float64{pm.powerVariance.variance()},
		},
		{
			Label:     "Go/label/CurrentStdDeviation",
			TestSuite: testSuite,
			Unit:      "mA",
			Values:    []float64{pm.currentVariance.standardDeviation()},
		},
		{
			Label:     "Go/label/VoltageStdDeviation",
			TestSuite: testSuite,
			Unit:      "V",
			Values:    []float64{pm.voltageVariance.standardDeviation()},
		},
		{
			Label:     "Go/label/PowerStdDeviation",
			TestSuite: testSuite,
			Unit:      "W",
			Values:    []float64{pm.powerVariance.standardDeviation()},
		},
	}
}

func (pm *PowerMeasurement) outputFile(outputFilename string) error {
	results := pm.generateTestResults()
	out, err := os.Create(outputFilename)
	if err != nil {
		return fmt.Errorf("failed to create file %q: %w", outputFilename, err)
	}
	defer out.Close()

	if err := results.Encode(out); err != nil {
		return fmt.Errorf("failed to write results to file %q: %w", outputFilename, err)
	}
	return nil
}
