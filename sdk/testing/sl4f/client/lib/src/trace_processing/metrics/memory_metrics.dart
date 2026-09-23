// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// @dart=2.12

import 'package:logging/logging.dart';

import '../metrics_results.dart';
import '../trace_model.dart';
import 'common.dart';

final _log = Logger('MemoryMetricsProcessor');

class _Results {
  late List<double> totalSystemMemory;
  late List<double> vmoMemory;
  late List<double> mmuMemory;
  late List<double> ipcMemory;

  _BandwidthResults? bandwidth;
}

class _BandwidthResults {
  late Map<String, List<double>> channels;
  late List<double> total;
  late List<double> usage;
}

double _sum(Iterable<num> seq) => seq.fold(0.0, (a, b) => a + b);

_Results? _memoryMetrics(Model model) {
  final memoryMonitorEvents =
      filterEvents(getAllEvents(model), category: 'memory_monitor');
  if (memoryMonitorEvents.isEmpty) {
    final duration = getTotalTraceDuration(model);
    _log.warning(
        'Could not find any `memory_monitor` events. Perhaps the trace '
        'duration (${duration.toMilliseconds()} milliseconds) is too short or '
        'the category "memory_monitor" is missing.');
    return null;
  }

  final fixedMemoryEvents =
      filterEventsTyped<CounterEvent>(memoryMonitorEvents, name: 'fixed');
  if (fixedMemoryEvents.isEmpty) {
    _log.warning(
        'Missing ("memory_monitor", "fixed") counter event in trace. No memory data is extracted.');
    return null;
  }
  final totalMemory = fixedMemoryEvents.first.args['total'];
  if (totalMemory == null) {
    _log.warning(
        'Malformed ("memory_monitor", "fixed") counter event in trace. Missing "total" field. No memory data is extracted.');
    return null;
  }
  final allocatedMemoryEvents =
      filterEventsTyped<CounterEvent>(memoryMonitorEvents, name: 'allocated');
  if (allocatedMemoryEvents.isEmpty) {
    final duration = getTotalTraceDuration(model);
    _log.warning(
        'Could not find any allocated memory events. Perhaps the trace '
        'duration (${duration.toMilliseconds()} milliseconds) is too short.');
    return null;
  }
  final vmoMemoryValues =
      getArgValuesFromEvents<num>(allocatedMemoryEvents, 'vmo')
          .map((v) => v.toDouble());
  final mmuMemoryValues =
      getArgValuesFromEvents<num>(allocatedMemoryEvents, 'mmu_overhead')
          .map((v) => v.toDouble());
  final ipcMemoryValues =
      getArgValuesFromEvents<num>(allocatedMemoryEvents, 'ipc')
          .map((v) => v.toDouble());
  final freeMemoryValues = getArgValuesFromEvents<num>(
          filterEventsTyped<CounterEvent>(memoryMonitorEvents, name: 'free'),
          'free')
      .map((v) => v.toDouble());
  final usedMemoryValues = freeMemoryValues.map((freeMemory) {
    final double usedMemory = totalMemory - freeMemory;
    return usedMemory;
  });
  final results = _Results()
    ..totalSystemMemory = usedMemoryValues.toList()
    ..vmoMemory = vmoMemoryValues.toList()
    ..mmuMemory = mmuMemoryValues.toList()
    ..ipcMemory = ipcMemoryValues.toList();

  final bandwidthUsageEvents = filterEventsTyped<CounterEvent>(
      memoryMonitorEvents,
      name: 'bandwidth_usage');
  if (bandwidthUsageEvents.isEmpty) {
    final duration = getTotalTraceDuration(model);
    _log.warning(
        'Could not find any memory bandwidth events. Perhaps the trace '
        'duration (${duration.toMilliseconds()} milliseconds) is too short.');
    return results;
  }

  final bandwidthChannels = <String, List<double>>{};
  for (final event in bandwidthUsageEvents) {
    for (final entry in event.args.entries) {
      bandwidthChannels[entry.key] ??= <double>[];
      bandwidthChannels[entry.key]!.add(entry.value.toDouble());
    }
  }

  final totalBandwidthValues =
      bandwidthUsageEvents.map((e) => _sum(e.args.values.cast<num>()));

  final bandwidthFreeEvents = filterEventsTyped<CounterEvent>(
      memoryMonitorEvents,
      name: 'bandwidth_free');
  final freeBandwidthValues =
      getArgValuesFromEvents<num>(bandwidthFreeEvents, 'value')
          .map((v) => v.toDouble());
  final bandwidthUsagePercentValues = Zip2Iterable<double, double, double>(
      totalBandwidthValues,
      freeBandwidthValues,
      (totalBandwidthValue, freeBandwidthValue) =>
          (100.0 * totalBandwidthValue) /
          (totalBandwidthValue + freeBandwidthValue));
  results.bandwidth = _BandwidthResults()
    ..channels = bandwidthChannels
    ..total = totalBandwidthValues.toList()
    ..usage = bandwidthUsagePercentValues.toList();
  return results;
}

List<TestCaseResults> memoryMetricsProcessor(
    Model model, Map<String, dynamic> extraArgs) {
  final results = _memoryMetrics(model);
  if (results == null) {
    return [];
  }

  _log
    ..info(
        'Average Total System Memory in bytes: ${computeMean(results.totalSystemMemory)}')
    ..info('Average VMO Memory in bytes: ${computeMean(results.vmoMemory)}')
    ..info(
        'Average MMU Overhead Memory in bytes: ${computeMean(results.mmuMemory)}')
    ..info('Average IPC Memory in bytes: ${computeMean(results.ipcMemory)}');

  // Assign the bandwidth results to a local to allow type promotion to occur.
  final bandwidthResults = results.bandwidth;
  if (bandwidthResults != null) {
    _log
      ..info('Average Total Memory Bandwidth Usage in bytes: '
          '${computeMean(bandwidthResults.total)}')
      ..info('Average Memory Bandwidth Usage in percent: '
          '${computeMean(bandwidthResults.usage)}');
    for (final entry in bandwidthResults.channels.entries) {
      _log.info(
          'Average ${entry.key} bandwidth usage in bytes: ${computeMean(entry.value)}');
    }
    _log.info([
      'Total bandwidth: ${(computeMean(bandwidthResults.total) / 1024).round()} KB/s',
      'usage: ${computeMean(bandwidthResults.usage).toStringAsFixed(2)}%',
      for (final entry in bandwidthResults.channels.entries)
        '${entry.key}: ${(computeMean(entry.value) / 1024).round()} KB/s',
    ].join(', '));
  }

  // Map bandwidth usage event field names to their metric name.  "Other" is
  // capitalized and all other names are mapped to full upper case
  // (e.g. "cpu" -> "CPU").  This logic is especially important for preserving
  // existing metric names from before we accepted arbitrary bandwidth usage
  // event fields.
  String _toMetricName(String traceName) {
    if (traceName == 'other') {
      return 'Other';
    }
    return traceName.toUpperCase();
  }

  return [
    TestCaseResults(
        'Total System Memory', Unit.bytes, results.totalSystemMemory.toList()),
    TestCaseResults('VMO Memory', Unit.bytes, results.vmoMemory.toList()),
    TestCaseResults(
        'MMU Overhead Memory', Unit.bytes, results.mmuMemory.toList()),
    TestCaseResults('IPC Memory', Unit.bytes, results.ipcMemory.toList()),
    if (bandwidthResults != null)
      for (final entry
          in bandwidthResults.channels.entries.toList()
            ..sort((a, b) => a.key.compareTo(b.key)))
        TestCaseResults('${_toMetricName(entry.key)} Memory Bandwidth Usage',
            Unit.bytes, entry.value.toList()),
    if (bandwidthResults != null)
      TestCaseResults('Total Memory Bandwidth Usage', Unit.bytes,
          bandwidthResults.total.toList()),
    if (bandwidthResults != null)
      TestCaseResults('Memory Bandwidth Usage', Unit.percent,
          bandwidthResults.usage.toList())
  ];
}

String memoryMetricsReport(Model model) {
  final buffer = StringBuffer()..write('''
===
Memory
===

''');

  final results = _memoryMetrics(model);
  if (results != null) {
    buffer
      ..write('total_system_memory:\n')
      ..write(describeValues(results.totalSystemMemory, indent: 2))
      ..write('vmo_memory:\n')
      ..write(describeValues(results.vmoMemory, indent: 2))
      ..write('mmu_overhead_memory:\n')
      ..write(describeValues(results.mmuMemory, indent: 2))
      ..write('ipc_memory:\n')
      ..write(describeValues(results.ipcMemory, indent: 2));
    final bandwidthResults = results.bandwidth;
    if (bandwidthResults != null) {
      for (final entry in bandwidthResults.channels.entries) {
        buffer
          ..write('${entry.key}_memory_bandwidth_usage:\n')
          ..write(describeValues(entry.value, indent: 2));
      }
      buffer
        ..write('total_memory_bandwidth_usage:\n')
        ..write(describeValues(bandwidthResults.total, indent: 2))
        ..write('memory_bandwidth_usage:\n')
        ..write(describeValues(bandwidthResults.usage, indent: 2))
        ..write('\n');
    }
  }
  return buffer.toString();
}
