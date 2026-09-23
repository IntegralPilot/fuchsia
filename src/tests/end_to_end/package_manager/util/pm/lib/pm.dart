// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/42165807): Fix null safety and remove this language version.
// @dart=2.9

import 'dart:async';
import 'dart:convert';
import 'dart:core';
import 'dart:io';

import 'package:async/async.dart';
import 'package:logging/logging.dart';
import 'package:net/curl.dart';
import 'package:net/ports.dart';
import 'package:path/path.dart' as path;
import 'package:quiver/core.dart' show Optional;
import 'package:retry/retry.dart';
import 'package:test/test.dart';

class PackageManagerRepo {
  final String _ffxPath;
  final String _ffxIsolateDir;
  final String _repoPath;
  final Logger _log;
  Optional<int> _servePort;
  Optional<Process> _serveProcess;

  Optional<int> getServePort() => _servePort;
  String getRepoPath() => _repoPath;

  PackageManagerRepo._create(
      this._ffxPath, this._ffxIsolateDir, this._repoPath, this._log) {
    _servePort = Optional.absent();
    _serveProcess = Optional.absent();
  }

  static Future<PackageManagerRepo> initRepo(String ffxPath, Logger log) async {
    var repoPath = (await Directory.systemTemp.createTemp('repo')).path;
    var ffxIsolateDir =
        (await Directory.systemTemp.createTemp('ffx_isolate_dir')).path;
    return PackageManagerRepo._create(ffxPath, ffxIsolateDir, repoPath, log);
  }

  /// Create new repo using `ffx repository create`.
  ///
  /// Uses this command:
  /// `ffx repository create <repo path>`
  Future<void> ffxRepositoryCreate() async {
    _log.info('Initializing repo: $_repoPath');

    await ffx(['repository', 'create', _repoPath]);
  }

  /// Publish an archive to a repo using `ffx repository publish`.
  ///
  /// Uses this command:
  /// `ffx repository publish --package-archive <archive path> <repo path>`
  Future<void> ffxRepositoryPublish(String archivePath) async {
    _log.info('Publishing $archivePath to repo.');
    await ffx(
        ['repository', 'publish', '--package-archive', archivePath, _repoPath]);
  }

  /// Create archive for a given manifest using `ffx package archive create`.
  ///
  /// Uses this command:
  /// `ffx package archive create <manifest path> --out <archivePath> --root-dir <rootDirectory>`
  Future<void> ffxPackageArchiveCreate(
      String packageManifestPath, String archivePath) async {
    _log.info('Creating archive from a given package manifest.');
    final rootDirectory = Platform.script.resolve('runtime_deps').toFilePath();
    await ffx(
      [
        'package',
        'archive',
        'create',
        packageManifestPath,
        '--out',
        archivePath,
        '--root-dir',
        rootDirectory
      ],
    );
  }

  /// Attempts to start the `ffx repository server` process.
  ///
  /// `curl` will be used as an additional check for whether `ffx repository
  /// server` has successfully started.
  ///
  /// Returns `true` if serve startup was successful.
  Future<bool> tryServe(String repoName, bool bindLoopback, int port) async {
    await stopServer();
    var address = '[::]';
    if (bindLoopback) {
      address = '[::1]';
    }
    final port_path = _repoPath + '/port';
    final process = await Process.start(
        _ffxPath + "/ffx",
        ffxArgs([
          'repository',
          'server',
          'start',
          '--foreground',
          '--repository',
          repoName,
          '--repo-path',
          _repoPath,
          '--address',
          '$address:$port',
          '--port-path',
          port_path,
        ]));
    unawaited(process.exitCode.then((exitCode) async {
      if (exitCode != 0) {
        final stderr = await process.stderr.transform(utf8.decoder).join();
        _log.warning('ffx repository server start failed: $stderr');
      }
    }));
    _serveProcess = Optional.of(process);

    await RetryOptions(maxAttempts: 5).retry(() async {
      final port_string = await File(port_path).readAsString();
      _servePort = Optional.of(int.parse(port_string));
    });

    if (port != 0) {
      expect(_servePort.value, port);
    }
    _log.info('Wait until serve responds to curl.');
    final curlStatus = await retryWaitForCurlHTTPCode(
        ['http://localhost:${_servePort.value}/$repoName/targets.json'], 200,
        logger: _log);
    _log.info('curl return code: $curlStatus');
    return curlStatus == 0;
  }

  /// Start a package server using `ffx repository server` with serve-selected
  /// port.
  ///
  /// Passes in `--address <address>:0` to tell `ffx repository` to choose its own
  /// port.
  ///
  /// Does not return until the serve begins listening, or times out.
  ///
  /// Address is `[::]` or `[::1]` depending on bindLoopback.
  ///
  /// Uses these commands:
  /// `ffx repository server start --foreground --address <address>>:0`
  Future<void> startServer(String repoName, {bool bindLoopback = false}) async {
    _log.info('Server is starting.');
    final retryOptions = RetryOptions(maxAttempts: 5);
    await retryOptions.retry(() async {
      if (!await tryServe(repoName, bindLoopback, 0)) {
        throw Exception(
            'Attempt to bringup `ffx repository server` has failed.');
      }
    });
  }

  /// Start a package server using `ffx repository server` with our own port
  /// selection.
  ///
  /// Does not return until the serve begins listening, or times out.
  ///
  /// Address is `[::]` or `[::1]` depending on bindLoopback.
  ///
  /// Uses these commands:
  /// `ffx repository server start --foreground --address <address>:<port number>`
  Future<void> startServerUnusedPort(String repoName,
      {bool bindLoopback = false}) async {
    await getUnusedPort<bool>((unusedPort) async {
      _log.info('Serve is starting on port: $unusedPort');
      try {
        if (await tryServe(repoName, bindLoopback, unusedPort)) {
          return true;
        }
      } catch (e) {
        _log.warning('Failed to start server on port $unusedPort: $e');
      }
      return null;
    });

    expect(_servePort.isPresent, isTrue);
  }

  /// Register the repo in target using `ffx target repository register`.
  ///
  /// Uses this command:
  /// `ffx target repository register`
  Future<void> ffxTargetRepositoryRegister(String repoName) async {
    await ffx(['target', 'repository', 'register', '--repository', repoName]);
  }

  Future<String> ffx(List<String> args) async {
    final result = await ffxRun(args);
    expect(result.exitCode, 0,
        reason: '`ffx ${args.join(" ")}` failed: ' + result.stderr);
    return result.stdout;
  }

  Future<ProcessResult> ffxRun(List<String> args) async {
    return await Process.run(_ffxPath + "/ffx", ffxArgs(args));
  }

  List<String> ffxArgs(List<String> args) {
    var environment = Platform.environment;
    final dev_addr = environment['FUCHSIA_DEVICE_ADDR'];
    final port = environment['FUCHSIA_SSH_PORT'];
    return [
          '--target',
          '$dev_addr:$port',
          '--config',
          'ffx.subtool-search-paths=' + _ffxPath,
          '--isolate-dir',
          _ffxIsolateDir
        ] +
        args;
  }

  /// Returns the output of `pkgctl repo` as a set.
  ///
  /// Each line in the output is a string in the set.
  Future<Set<String>> getCurrentRepos() async {
    var listSrcsResponse =
        await _sshRun('Reading current registered repos', 'pkgctl repo', [], 0);
    if (listSrcsResponse.exitCode != 0) {
      return {};
    }
    return Set.from(LineSplitter().convert(listSrcsResponse.stdout.toString()));
  }

  /// Resets the pkgctl state to its default state.
  ///
  /// Some tests add new package sources that override defaults. We can't
  /// trust that they will clean up after themselves, so this function
  /// will generically remove all non-original sources and enable the original
  /// rewrite rule.
  Future<bool> resetPkgctl(
      Set<String> originalRepos, String originalRewriteRule) async {
    var currentRepos = await getCurrentRepos();

    // Remove all repos that were not originally existing.
    currentRepos.removeAll(originalRepos);
    for (final server in currentRepos) {
      final rmSrcResponse =
          await _sshRun('Removing $server', 'pkgctl repo rm $server', [], 0);
      if (rmSrcResponse.exitCode != 0) {
        return false;
      }
    }

    final response = await _sshRun('Resetting rewrite rules',
        'pkgctl rule replace json \'$originalRewriteRule\'', [], 0);
    if (response.exitCode != 0) {
      return false;
    }
    return true;
  }

  /// Get the named component from the repo using `pkgctl resolve`.
  ///
  /// Uses this command:
  /// `pkgctl resolve <component URL>`
  Future<ProcessResult> pkgctlResolve(
      String msg, String url, int retCode) async {
    return _sshRun(msg, 'pkgctl resolve', [url], retCode);
  }

  /// Get the named component from the repo using `pkgctl resolve --verbose`.
  ///
  /// Uses this command:
  /// `pkgctl resolve --verbose <component URL>`
  Future<ProcessResult> pkgctlResolveV(
      String msg, String url, int retCode) async {
    return _sshRun(msg, 'pkgctl resolve --verbose', [url], retCode);
  }

  /// List repo sources using `pkgctl repo`.
  ///
  /// Uses this command:
  /// `pkgctl repo`
  Future<ProcessResult> pkgctlRepo(String msg, int retCode) async {
    return _sshRun(msg, 'pkgctl repo', [], retCode);
  }

  /// Remove a repo source using `pkgctl repo rm`.
  ///
  /// Uses this command:
  /// `pkgctl repo rm <repo name>`
  Future<ProcessResult> pkgctlRepoRm(
      String msg, String repoName, int retCode) async {
    return _sshRun(msg, 'pkgctl repo', ['rm $repoName'], retCode);
  }

  /// Replace dynamic rules using `pkgctl rule replace`.
  ///
  /// Uses this command:
  /// `pkgctl rule replace json <json>`
  Future<ProcessResult> pkgctlRuleReplace(
      String msg, String json, int retCode) async {
    return _sshRun(msg, 'pkgctl rule replace', ['json \'$json\''], retCode);
  }

  /// List redirect rules using `pkgctl rule list`.
  ///
  /// Uses this command:
  /// `pkgctl rule list`
  Future<ProcessResult> pkgctlRuleList(String msg, int retCode) async {
    return _sshRun(msg, 'pkgctl rule list', [], retCode);
  }

  /// List redirect rules using `pkgctl rule dump-dynamic`.
  ///
  /// Uses this command:
  /// `pkgctl rule dump-dynamic`
  Future<ProcessResult> pkgctlRuleDumpdynamic(String msg, int retCode) async {
    return _sshRun(msg, 'pkgctl rule dump-dynamic', [], retCode);
  }

  /// Get a package hash `pkgctl get-hash`.
  ///
  /// Uses this command:
  /// `pkgctl get-hash <package name>`
  Future<ProcessResult> pkgctlGethash(
      String msg, String package, int retCode) async {
    return _sshRun(msg, 'pkgctl', ['get-hash $package'], retCode);
  }

  Future<ProcessResult> _sshRun(
      String msg, String cmd, List<String> params, int retCode,
      {bool randomize = false}) async {
    _log.info(msg);
    if (randomize) {
      params.shuffle();
    }
    var ffxArgs = ['target', 'ssh', cmd] + params;
    final response = await ffxRun(ffxArgs);
    var stdout = response.stdout.toString();
    var stderr = response.stderr.toString();
    if (response.exitCode != 0) {
      _log.info('Error running ffx ${ffxArgs}: $stdout $stderr');
    }

    expect(response.exitCode, retCode);
    return response;
  }

  Future<bool> setupRepo(String farPath, String manifestPath) async {
    final archivePath =
        Platform.script.resolve('runtime_deps/$farPath').toFilePath();
    await Future.wait([
      ffxRepositoryCreate(),
      ffxPackageArchiveCreate(manifestPath, archivePath)
    ]);

    _log.info(
        'Publishing package from archive: $archivePath to repo: $_repoPath');
    await ffxRepositoryPublish(archivePath);

    return true;
  }

  Future<bool> setupServe(String farPath, String manifestPath, String repoName,
      {bool bindLoopback = false}) async {
    await setupRepo(farPath, manifestPath);
    await startServerUnusedPort(repoName, bindLoopback: bindLoopback);
    return true;
  }

  Future<void> stopServer() async {
    if (_serveProcess.isPresent) {
      _log.info('Stopping server...');
      await _serveProcess.value.kill();
      _serveProcess = Optional.absent();
      _servePort = Optional.absent();
    }
  }

  Future<void> cleanup() async {
    await stopServer();
    await Future.wait([_repoPath, _ffxIsolateDir].map((path) async {
      try {
        await Directory(path).delete(recursive: true);
      } on PathNotFoundException {
        // if the directory does not exist, we don't need to do anything.
      }
    }));
  }
}
