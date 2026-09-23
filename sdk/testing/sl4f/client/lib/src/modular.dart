// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// @dart=2.12

import 'dart:convert';

import 'package:logging/logging.dart';

import 'exceptions.dart';
import 'sl4f_client.dart';

final _log = Logger('modular');

/// The function type for making a generic request to Modular.
typedef ModularRequestFn = Future<dynamic> Function(String request,
    [dynamic params]);

/// Controls a Modular session and its components.
///
/// See https://fuchsia.dev/fuchsia-src/concepts/modular/overview for more
/// information on Modular.
class Modular {
  /// Function that makes a request to SL4F.
  final ModularRequestFn _request;

  bool _sessionWasStartedHere = false;

  /// Whether this instance controls the currently running session so it will
  /// shut it down when [shutdown] is called.
  bool get controlsTheSession => _sessionWasStartedHere;

  Modular(Sl4f sl4f) : _request = sl4f.request;

  /// Restarts a Modular session.
  ///
  /// This is equivalent to `ffx session restart`.
  Future<String> restartSession() async =>
      await _request('modular_facade.RestartSession');

  /// Kill Basemgr.
  ///
  /// This is equivalent to stopping the session component.
  Future<String> killBasemgr() async =>
      await _request('modular_facade.KillBasemgr');

  /// Starts a session component.
  ///
  /// Note that there can only be one
  /// session component running at a time. To automatically stop the current
  /// session before starting a new one, use [boot].
  ///
  /// [sessionUrl] is required.
  /// [config] is deprecated and ignored.
  Future<String> startBasemgr([String? config, String? sessionUrl]) async {
    final args = {};
    if (config != null && config.isNotEmpty) {
      args['config'] = json.decode(config);
    }
    if (sessionUrl != null) {
      args['session_url'] = sessionUrl;
    }
    return await _request('modular_facade.StartBasemgr', args);
  }

  /// Whether the session is currently running on the DUT.
  ///
  /// This works whether it was started by this class or not.
  Future<bool> get isRunning async =>
      await _request('modular_facade.IsBasemgrRunning');

  /// Starts the session only if one isn't running yet.
  ///
  /// If [assumeControl] is true (the default) and a session wasn't running, then
  /// this object will stop the session when [shutdown] is called with no arguments.
  ///
  /// If [sessionUrl] must be provided.
  Future<void> boot(
      {String? config, bool assumeControl = true, String? sessionUrl}) async {
    if (await isRunning) {
      _log.info('Not taking control of the session, it is already running.');
      return;
    }

    if (sessionUrl == null) {
      _log.severe('sessionUrl is required. Aborting.');
      return;
    }

    _log.info('Starting ${sessionUrl}');

    await startBasemgr(config, sessionUrl);

    var retry = 0;
    while (retry++ <= 60 && !await isRunning) {
      await Future.delayed(const Duration(seconds: 2));
    }

    if (!await isRunning) {
      throw Sl4fException('Timeout for waiting the session to start.');
    }
    if (assumeControl) {
      _sessionWasStartedHere = true;
    }
  }

  /// Stops the session if it is controlled by this instance, or if
  /// [forceShutdownBasemgr] is true.
  Future<void> shutdown({bool forceShutdownBasemgr = false}) async {
    if (!forceShutdownBasemgr && !_sessionWasStartedHere) {
      _log.info(
          'Modular SL4F client does not control the session, not shutting it down');
      return;
    }
    if (!await isRunning) {
      _log.info('There is no session running, unable to shut down.');
      return;
    }
    _log.info('Stopping the session.');
    await killBasemgr();

    final timer = Stopwatch();
    timer.start();
    final timeout = Duration(seconds: 30).inSeconds;
    while (timer.elapsed.inSeconds < timeout && await isRunning) {
      await Future.delayed(const Duration(seconds: 2));
    }
    _log.info(
        'Waited ${timer.elapsed.inSeconds} seconds for the session to stop.');

    if (await isRunning) {
      throw Sl4fException('Timeout for waiting the session to stop.');
    }
    _sessionWasStartedHere = false;
  }
}
