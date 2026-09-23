// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/42165807): Fix null safety and remove this language version.
// @dart=2.9

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:archive/archive.dart';
import 'package:archive/archive_io.dart';
import 'package:logging/logging.dart';
import 'package:path/path.dart' as path;
import 'package:pkg/pkg.dart';
import 'package:pm/pm.dart';
import 'package:retry/retry.dart';
import 'package:sl4f/sl4f.dart' as sl4f;
import 'package:test/test.dart';

// TODO(https://fxbug.dev/42130689): update to use test size.
const _timeout = Timeout(Duration(minutes: 5));

void printErrorHelp() {
  print('If this test fails, see '
      'https://fuchsia.googlesource.com/fuchsia/+/HEAD/src/tests/end_to_end/package_manager/README.md'
      ' for details!');
}

// validRepoName replaces invalid characters in the input sequence to ensure
// the returned string complies to
// https://fuchsia.dev/fuchsia-src/concepts/packages/package_url#repository
// The replacement starts with a letter to avoid starting with a - since it makes
// it look like an option to the command.
String validRepoName(String originalName) {
  return originalName.replaceAll(RegExp(r'(?![a-z0-9-]).'), 'x-');
}

Future<String> formattedHostAddress(sl4f.Sl4f sl4fDriver, Logger log) async {
  final output = await sl4fDriver.ssh.run('echo \$SSH_CONNECTION');
  log.info('\$SSH_CONNECTION response: ${output.stdout.toString()}');
  final hostAddress =
      output.stdout.toString().split(' ')[0].replaceAll('%', '%25');
  return '[$hostAddress]';
}

void main() {
  final log = Logger('package_manager_test');
  final runtimeDepsPath = Platform.script.resolve('runtime_deps').toFilePath();
  String hostAddress;
  String manifestPath;

  sl4f.Sl4f sl4fDriver;

  setUpAll(() async {
    Logger.root
      ..level = Level.ALL
      ..onRecord.listen((rec) => print('[${rec.level}]: ${rec.message}'));
    sl4fDriver = sl4f.Sl4f.fromEnvironment();

    hostAddress = await formattedHostAddress(sl4fDriver, log);

    // Extract the `package.tar`.
    final packageTarPath =
        Platform.script.resolve('runtime_deps/package.tar').toFilePath();
    final bytes = File(packageTarPath).readAsBytesSync();
    final packageTar = TarDecoder().decodeBytes(bytes);
    for (final file in packageTar) {
      final filename = file.name;
      if (file.isFile) {
        List<int> data = file.content;
        File(runtimeDepsPath + Platform.pathSeparator + filename)
          ..createSync(recursive: true)
          ..writeAsBytesSync(data);
      }
    }

    // The `package_manifest.json` file comes from the tar extracted above.
    manifestPath = Platform.script
        .resolve('runtime_deps/package_manifest.json')
        .toFilePath();
  });

  tearDownAll(() async {
    sl4fDriver.close();

    printErrorHelp();
  });

  group('Package Manager', () {
    String originalRewriteRuleJson;
    Set<String> originalRepos;
    PackageManagerRepo repoServer;
    String repoName = 'pm-test-repo';
    String testPackageName = 'package-manager-sample';
    String testRepoRewriteRule =
        '{"version":"1","content":[{"host_match":"package-manager-test","host_replacement":"%%NAME%%","path_prefix_match":"/","path_prefix_replacement":"/"}]}';

    setUp(() async {
      repoServer = await PackageManagerRepo.initRepo(runtimeDepsPath, log);

      // Gather the original package management settings before test begins.
      originalRepos = await repoServer.getCurrentRepos();
      originalRewriteRuleJson = (await repoServer.pkgctlRuleDumpdynamic(
              'Save original rewrite rules from `pkgctl rule dump-dynamic`', 0))
          .stdout
          .toString();
    });
    tearDown(() async {
      if (!await repoServer.resetPkgctl(
          originalRepos, originalRewriteRuleJson)) {
        log.severe('Failed to reset pkgctl to default state');
      }
      if (repoServer != null) {
        await repoServer.cleanup();
      }
    });
    test(
        'Test that creates a repository, registers it, and validates that the '
        'package in the repository is visible.', () async {
      // Covers these commands (success cases only):
      //
      // Newly covered:
      // pkgctl get-hash fuchsia-pkg://<repo URL>/<package name>
      // pkgctl rule dump-dynamic
      // pkgctl repo add url <repo URL> -f 1
      // pkgctl repo rm fuchsia-pkg://<repo URL>
      // pkgctl repo
      // ffx repository server start --address [::]:<port> --repository <repo name>
      await repoServer.setupServe(
          '$testPackageName-0.far', manifestPath, repoName);
      final optionalPort = repoServer.getServePort();
      expect(optionalPort.isPresent, isTrue);
      final port = optionalPort.value;

      String repoUrl = 'fuchsia-pkg://$repoName';

      // Confirm our serve is serving what we expect.
      log.info('Getting the available packages');
      final curlResponse = await Process.run(
          'curl', ['http://localhost:$port/$repoName/targets.json', '-i']);

      log.info('curl response: ${curlResponse.stdout.toString()}');
      expect(curlResponse.exitCode, 0);
      final curlOutput = curlResponse.stdout.toString();
      expect(curlOutput.contains('$testPackageName/0'), isTrue);

      var gethashOutput = (await repoServer.pkgctlGethash(
              'Should error when checking for the package',
              '$repoUrl/$testPackageName',
              0))
          .stdout
          .toString();

      // Record what the rule list is before we begin, and confirm that is
      // the rule list when we are finished.
      final originalRuleList = (await repoServer.pkgctlRuleDumpdynamic(
              'Recording the current rule list', 0))
          .stdout
          .toString();

      await repoServer.ffxTargetRepositoryRegister(repoName);

      // Check that our new repo source is listed.
      var listSrcsOutput = (await repoServer.pkgctlRepo(
              'Running pkgctl repo to list sources', 0))
          .stdout
          .toString();

      log.info('listSrcsOutput: $listSrcsOutput');
      expect(listSrcsOutput.contains(repoUrl), isTrue);

      gethashOutput = (await repoServer.pkgctlGethash(
              'Checking if the package now exists',
              '$repoUrl/$testPackageName',
              0))
          .stdout
          .toString();

      log.info('gethashOutput: $gethashOutput');

      expect(
          gethashOutput
              .contains('Error: Failed to get package hash with error:'),
          isFalse);

      var ruleListOutput = (await repoServer.pkgctlRuleDumpdynamic(
              'Confirm rule list did not change.', 0))
          .stdout
          .toString();
      expect(ruleListOutput, originalRuleList);

      await repoServer.pkgctlRepoRm('Delete $repoUrl', repoUrl, 0);

      // Check that our new repo source is gone.
      listSrcsOutput = (await repoServer.pkgctlRepo(
              'Running pkgctl repo to list sources', 0))
          .stdout
          .toString();

      log.info('listSrcsOutput: $listSrcsOutput');
      expect(listSrcsOutput.contains(repoUrl), isFalse);

      ruleListOutput = (await repoServer.pkgctlRuleDumpdynamic(
              'Confirm rule list did not change.', 0))
          .stdout
          .toString();
      expect(ruleListOutput, originalRuleList);
    });
    test(
        'Test that creates a repository, deploys a package, and '
        'validates that the deployed package is visible from the server. ',
        () async {
      // Covers these commands (success cases only):
      //
      // Previously covered:
      // pkgctl repo add url http://<host>:<port>/config.json -n testhost -f 1
      await repoServer.setupServe(
          '$testPackageName-0.far', manifestPath, repoName);
      final optionalPort = repoServer.getServePort();
      expect(optionalPort.isPresent, isTrue);
      final port = optionalPort.value;

      log.info('Getting the available packages');
      final curlResponse = await Process.run(
          'curl', ['http://localhost:$port/$repoName/targets.json', '-i']);

      log.info('curl response: ${curlResponse.stdout.toString()}');
      expect(curlResponse.exitCode, 0);
      final curlOutput = curlResponse.stdout.toString();
      expect(curlOutput.contains('$testPackageName/0'), isTrue);

      await repoServer.ffxTargetRepositoryRegister(repoName);
    });
    test('Test `ffx repository server` chooses its own port number.', () async {
      // Covers these commands (success cases only):
      //
      // Newly covered:
      // ffx repository server start --address [::]:0
      await repoServer.setupRepo('$testPackageName-0.far', manifestPath);

      await repoServer.startServer(repoName);
      final optionalPort = repoServer.getServePort();
      expect(optionalPort.isPresent, isTrue);

      log.info('Checking port ${optionalPort.value} is valid.');
      final curlResponse = await Process.run('curl', [
        'http://localhost:${optionalPort.value}/$repoName/targets.json',
        '-i'
      ]);

      log.info('curl response: ${curlResponse.stdout.toString()}');
      expect(curlResponse.exitCode, 0);
      final curlOutput = curlResponse.stdout.toString();
      expect(curlOutput.contains('$testPackageName/0'), isTrue);
    });
    test('Test `pkgctl resolve` base case.', () async {
      // Covers these commands (success cases only):
      //
      // Newly covered:
      // pkgctl resolve fuchsia-pkg://package-manager-test/<name>
      var resolveProcessResult = await repoServer.pkgctlResolve(
          'Confirm that `$testPackageName` does not exist.',
          'fuchsia-pkg://package-manager-test/$testPackageName',
          1);
      expect(resolveProcessResult.exitCode, isNonZero);
      expect(
          resolveProcessResult.stdout.toString(),
          equals(
              'resolving fuchsia-pkg://package-manager-test/package-manager-sample\n'));

      // Set the repo name to something unique for this test. Following the rules in
      // https://fuchsia.dev/fuchsia-src/concepts/packages/package_url#repository
      var repoNameFixed = validRepoName("test-pkg-resolve-base");

      await repoServer.setupServe(
          '$testPackageName-0.far', manifestPath, repoNameFixed);
      final optionalPort = repoServer.getServePort();
      expect(optionalPort.isPresent, isTrue);
      final port = optionalPort.value;

      await repoServer.ffxTargetRepositoryRegister(repoNameFixed);

      var localRewriteRule = testRepoRewriteRule;
      localRewriteRule =
          localRewriteRule.replaceAll('%%NAME%%', '$repoNameFixed');
      await repoServer.pkgctlRuleReplace(
          'Setting rewriting rule for new repository', localRewriteRule, 0);

      resolveProcessResult = await repoServer.pkgctlResolve(
          'Confirm that `$testPackageName` now exists.',
          'fuchsia-pkg://package-manager-test/$testPackageName',
          0);
      expect(resolveProcessResult.exitCode, isZero);
      expect(
          resolveProcessResult.stdout.toString(),
          equals(
              'resolving fuchsia-pkg://package-manager-test/package-manager-sample\n'));

      await repoServer.pkgctlRuleReplace(
          'Restoring rewriting rule to original state',
          originalRewriteRuleJson,
          0);
    });
    test('Test `pkgctl resolve --verbose` base case.', () async {
      // Covers these commands (success cases only):
      //
      // Newly covered:
      // pkgctl resolve --verbose fuchsia-pkg://package-manager-test/<name>
      var resolveVProcessResult = await repoServer.pkgctlResolveV(
          'Confirm that `$testPackageName` does not exist.',
          'fuchsia-pkg://package-manager-test/$testPackageName',
          1);
      expect(resolveVProcessResult.exitCode, isNonZero);
      expect(resolveVProcessResult.stdout.toString(),
          isNot(contains('package contents:\n')));

      // Set the repo name to something unique for this test. Following the rules in
      // https://fuchsia.dev/fuchsia-src/concepts/packages/package_url#repository
      var repoNameFixed = validRepoName("pkgctl-resolve-verbose-base");

      await repoServer.setupServe(
          '$testPackageName-0.far', manifestPath, repoNameFixed);
      final optionalPort = repoServer.getServePort();
      expect(optionalPort.isPresent, isTrue);
      final port = optionalPort.value;

      await repoServer.ffxTargetRepositoryRegister(repoNameFixed);

      var localRewriteRule = testRepoRewriteRule;
      localRewriteRule =
          localRewriteRule.replaceAll('%%NAME%%', '$repoNameFixed');
      await repoServer.pkgctlRuleReplace(
          'Setting rewriting rule for new repository', localRewriteRule, 0);

      resolveVProcessResult = await repoServer.pkgctlResolveV(
          'Confirm that `$testPackageName` now exists.',
          'fuchsia-pkg://package-manager-test/$testPackageName',
          0);
      expect(resolveVProcessResult.exitCode, isZero);
      expect(resolveVProcessResult.stdout.toString(),
          contains('package contents:\n'));

      await repoServer.pkgctlRuleReplace(
          'Restoring rewriting rule to original state',
          originalRewriteRuleJson,
          0);
    });
    test(
        'Test the flow from repo creation, to archive generation, '
        'to using pkgctl and running the component on the device.', () async {
      // Covers several key steps:
      // 0. Sanity check. The given component is not already available in the repo.
      // 1. The given component is archived into a valid `.far`.
      // 2. We are able to create our own repo.
      // 3. We are able to serve our repo to a given Fuchsia device.
      // 4. The device is able to pull the given component from our repo.
      // 5. The given component contains the expected content.
      var resolveExitCode = (await repoServer.pkgctlResolve(
              'Confirm that `$testPackageName` does not exist.',
              'fuchsia-pkg://package-manager-test/$testPackageName',
              1))
          .exitCode;
      expect(resolveExitCode, isNonZero);

      var repoNameFixed = validRepoName("test-repo-flow-e2e");

      await repoServer.setupServe(
          '$testPackageName-0.far', manifestPath, repoNameFixed);
      final optionalPort = repoServer.getServePort();
      expect(optionalPort.isPresent, isTrue);
      final port = optionalPort.value;

      await repoServer.ffxTargetRepositoryRegister(repoNameFixed);

      var localRewriteRule = testRepoRewriteRule;
      localRewriteRule = localRewriteRule.replaceAll('%%NAME%%', repoNameFixed);
      await repoServer.pkgctlRuleReplace(
          'Setting rewriting rule for new repository', localRewriteRule, 0);

      var response = await sl4fDriver.ssh.run(
          'run-test-suite fuchsia-pkg://package-manager-test/$testPackageName#meta/package-manager-sample.cm');
      expect(response.exitCode, 0);
      expect(response.stdout.toString().contains('Hello, World!\n'), true);
      response = await sl4fDriver.ssh.run(
          'run-test-suite fuchsia-pkg://package-manager-test/$testPackageName#meta/package-manager-sample2.cm');
      expect(response.exitCode, 0);
      expect(response.stdout.toString().contains('Hello, World2!\n'), true);

      await repoServer.pkgctlRuleReplace(
          'Restoring rewriting rule to original state',
          originalRewriteRuleJson,
          0);
    });
    test(
        'Test the flow from repo creation, to archive generation, '
        'to using pkgctl and running the component on the device.', () async {
      // Covers several key steps:
      // 1. The given component is archived into a valid `.far`.
      // 2. We are able to create our own repo.
      // 3. We are able to serve our repo to a given Fuchsia device.
      // 4. The device is able to pull the given component from our repo.
      // 5. The given component contains the expected content.
      await repoServer.setupServe(
          '$testPackageName-0.far', manifestPath, repoName);
      final optionalPort = repoServer.getServePort();
      expect(optionalPort.isPresent, isTrue);
      final port = optionalPort.value;

      String repoUrl = 'fuchsia-pkg://$repoName';

      await repoServer.ffxTargetRepositoryRegister(repoName);

      var localRewriteRule = testRepoRewriteRule;
      localRewriteRule = localRewriteRule.replaceAll('%%NAME%%', '$repoName');
      await repoServer.pkgctlRuleReplace(
          'Setting rewriting rule for new repository', localRewriteRule, 0);

      var response = await sl4fDriver.ssh.run(
          'run-test-suite $repoUrl/$testPackageName#meta/package-manager-sample.cm');
      expect(response.exitCode, 0);
      expect(response.stdout.toString().contains('Hello, World!\n'), true);
      response = await sl4fDriver.ssh.run(
          'run-test-suite $repoUrl/$testPackageName#meta/package-manager-sample2.cm');
      expect(response.exitCode, 0);
      expect(response.stdout.toString().contains('Hello, World2!\n'), true);

      await repoServer.pkgctlRuleReplace(
          'Restoring rewriting rule to original state',
          originalRewriteRuleJson,
          0);
    });
    test('Test that we can serve packages through a forwarding tunnel',
        () async {
      String repoUrl = 'fuchsia-pkg://$repoName';
      final noInitialHashOutput = (await repoServer.pkgctlGethash(
              'Checking if the package initially doesn\'t exist',
              '$repoUrl/$testPackageName',
              1))
          .stdout
          .toString();
      log.info('noInitialHashOutput: $noInitialHashOutput');

      await repoServer.setupServe(
          '$testPackageName-0.far', manifestPath, repoName,
          bindLoopback: true);
      final optionalPort = repoServer.getServePort();
      expect(optionalPort.isPresent, isTrue);
      final port = optionalPort.value;

      // Confirm our serve is serving what we expect.
      log.info('Getting the available packages');
      final curlResponse = await Process.run(
          'curl', ['http://localhost:$port/$repoName/targets.json', '-i']);

      log.info('curl response: ${curlResponse.stdout.toString()}');
      expect(curlResponse.exitCode, 0);
      final curlOutput = curlResponse.stdout.toString();
      expect(curlOutput.contains('$testPackageName/0'), isTrue);

      final gethashOutput = (await repoServer.pkgctlGethash(
              'Checking if the package now exists',
              '$repoUrl/$testPackageName',
              0))
          .stdout
          .toString();
      log.info('gethashOutput: $gethashOutput');
    });
  }, timeout: _timeout);
}
