// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_POWER_CPP_POWER_SUPPORT_H_
#define LIB_DRIVER_POWER_CPP_POWER_SUPPORT_H_

#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/driver/power/cpp/element-description-builder.h>
#include <lib/driver/power/cpp/types.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fit/function.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/event.h>
#include <lib/zx/handle.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

/// Collection of helpers for driver authors working with the power framework.
/// The basic usage model is
///   * use `fuchsia.hardware.platform.device/Device.GetPowerConfiguration` to
///     retrieve the config supplied by the board driver.
///   * For each power element in the driver's config
///       - Call `PowerAdapter::GetDependencyTokens` to get the element's
///         parents' access tokens.
///       - Calling `PowerAdapter::AddElement` and supplying the configuration,
///         token set from `GetDependencyTokens` and any access tokens the
///         driver needs to declare.
namespace fdf_power {

enum class Error : uint8_t {
  /// The power configuration appears to be invalid. A non-exhaustive list of
  /// possible reasons is it contained no elements, the element definition
  /// appears malformed, or other reasons.
  INVALID_ARGS,
  /// A general I/O error happened which we're not sure about. This should be
  /// a rare occurrence and typically more specific errors should be returned.
  IO,
  /// The configuration has a dependency, but we couldn't get access to the
  /// tokens for it. Maybe a parent didn't offer something expected or SAG
  /// didn't make something available.
  DEPENDENCY_NOT_FOUND,
  /// No token services capability available, maybe it wasn't routed?
  TOKEN_SERVICE_CAPABILITY_NOT_FOUND,
  /// An unexpected error occurred listing service instances.
  READ_INSTANCES,
  /// We were able to access the token service capability, but no instances
  /// were available. Did the parents offer any?
  NO_TOKEN_SERVICE_INSTANCES,
  /// Requesting a token from the provider protocol failed. Maybe the token
  /// provider is not implemented correctly?
  TOKEN_REQUEST,
  /// Couldn't access the capability for System Activity Governor tokens.
  ACTIVITY_GOVERNOR_UNAVAILABLE,
  /// Request to System Activity Governor returned an error.
  ACTIVITY_GOVERNOR_REQUEST,
  /// fuchsia.power.broker/Topology could not be connected to.
  TOPOLOGY_UNAVAILABLE,
  /// The power configuration could not be retrieved.
  CONFIGURATION_UNAVAILABLE,
  /// Could not access the CpuElementManager capability.
  CPU_ELEMENT_MANAGER_UNAVAILABLE,
  /// There was an error making a request to the CpuElementManager protocol.
  CPU_ELEMENT_MANAGER_REQUEST,
  INTERNAL,
  NOT_AUTHORIZED,
};

// Convenience methods that provide an approximate mapping to Zircon error values.
zx::error<zx_status_t> ErrorToZxError(Error e);
zx::error<zx_status_t> LeaseErrorToZxError(fuchsia_power_broker::LeaseError e);
zx::error<zx_status_t> AddElementErrorToZxError(fuchsia_power_broker::AddElementError e);

// Convenience methods for printing errors.
const char* ErrorToString(Error e);
const char* LeaseErrorToString(fuchsia_power_broker::LeaseError e);
const char* AddElementErrorToString(fuchsia_power_broker::AddElementError e);

inline fit::result<zx_status_t, uint8_t> default_level_changer(uint8_t level) {
  return fit::ok(level);
}

/// ElementRunner implementation that only updates PowerBroker immediately.
///
/// This helper class can be used to create an ElementRunner server that has no side effects or
/// conditions when the power level of the given power element changes.
class BasicElementRunner : public fidl::Server<fuchsia_power_broker::ElementRunner> {
 public:
  void SetLevel(SetLevelRequest& request, SetLevelCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::ElementRunner> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override;
};

/// Uses the provided namespace to add the power elements described in
/// |power_configs| to the power topology and returns corresponding
/// `ElementDesc` instances.
/// This function:
///     * Retrieves the tokens of any dependencies via
///       `fuchsia.hardware.power/PowerTokenProvider` instances
///     * Adds the power element via `fuchsia.power.broker/Topology`
///
/// In effect, this function converts the provided |power_configs| into their
/// corresponding `ElementDesc` objects and returns them.
fit::result<Error, std::vector<ElementDesc>> ApplyPowerConfiguration(
    const fdf::Namespace& ns, cpp20::span<PowerElementConfiguration> power_configs,
    bool use_element_runner = false);

/// Given a `PowerElementConfiguration` from driver framework, convert this
/// into a set of Power Broker's `LevelDependency` objects. The map is keyed
/// by the name of the parent/dependency.
///
/// If the `PowerElementConfiguration` expresses no dependencies, we return an
/// empty map.
///
/// NOTE: The `requires_token` of each of the `LevelDependency` objects is
/// **not** populated and must be filled in before providing this map to
/// `AddElement`.
///
/// Error returns:
///   - Error::INVALID_ARGS if `element_config` is missing fields, for example
///     if a level dependency doesn't have a parent level.
fit::result<Error, ElementDependencyMap> LevelDependencyFromConfig(
    const PowerElementConfiguration& element_config);

/// Given a `PowerElementConfiguration` from driver framework, convert this
/// into a set of Power Broker's `PowerLevel` objects.
///
/// If the `PowerElementConfiguration` expresses no levels, we return an
/// empty vector.
std::vector<fuchsia_power_broker::PowerLevel> PowerLevelsFromConfig(
    PowerElementConfiguration element_config);

/// For the Power Element represented by `element_config`, get the tokens for
/// the element's dependencies (ie. "parents") from
/// `fuchsia.hardware.power/PowerTokenProvider` instances in `ns`.
///
/// If the power element represented by `element_config` has no dependencies,
/// this function returns an empty set. If any dependency's token can not be
/// be retrieved we return an error.
/// Error returns:
///   - `Error::INVALID_ARGS` if the element_config appears invalid
///   - `Error::IO` if there is a communication failure when talking to a
///      service or a protocol required to get a token.
///   - `Error::DEPENDENCY_NOT_FOUND` if a token for a required dependency is
///     not available.
fit::result<Error, TokenMap> GetDependencyTokens(const fdf::Namespace& ns,
                                                 const PowerElementConfiguration& element_config);

/// For the Power Element represented by `element_config`, get the tokens for
/// the
/// element's dependencies (ie. "parents") from
/// `fuchsia.hardware.power/PowerTokenProvider` instances in `svcs_dir`.
/// `svcs_dir` should contain an entry for
/// `fuchsia.hardware.power/PowerTokenService`.
///
/// Returns a set of tokens from services instances found in `svcs_dir`. If
/// the power element represented by `element_config` has no dependencies, this
/// function returns an empty set. If any dependency's token can not be
/// be retrieved we return an error.
/// Error returns:
///   - `Error::INVALID_ARGS` if the element_config appears invalid
///   - `Error::IO` if there is a communication failure when talking to a
///      service or a protocol required to get a token.
///   - `Error::DEPENDENCY_NOT_FOUND` if a token for a required dependency is
///     not available.
fit::result<Error, TokenMap> GetDependencyTokens(const PowerElementConfiguration& element_config,
                                                 fidl::ClientEnd<fuchsia_io::Directory> svcs_dir);

/// Call `AddElement` on the `power_broker` channel passed in.
/// This function uses the `config` and `tokens` arguments to properly construct
/// the call to `fuchsia.power.broker/Topology.AddElement`. Optionally callers
/// can pass in tokens to be registered for granting assertive
/// dependency access on the created element.
///
/// Error
///   - Error::DEPENDENCY_NOT_FOUND if there is a dependency specified by
///     `config` which is to found in `tokens`.
///   - Error::INVALID_ARGS if `config` appears to be invalid, we fail to
///     duplicate a token and therefore assume it must have been invalid, or
///     the call to power broker fails for any reason *other* than a closed
///     channel.
fit::result<Error> AddElement(
    const fidl::ClientEnd<fuchsia_power_broker::Topology>& power_broker,
    const PowerElementConfiguration& config, TokenMap tokens,
    const zx::unowned_event& assertive_token,
    std::optional<fidl::ServerEnd<fuchsia_power_broker::Lessor>> lessor,
    std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementControl>> element_control,
    std::optional<fidl::UnownedClientEnd<fuchsia_power_broker::ElementControl>>
        element_control_client,
    std::optional<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>> element_runner);

/// Call `AddElement` on the `power_broker` channel passed in.
/// This function uses `ElementDescription` passed in to make the proper call
/// to `fuchsia.power.broker/Topology.AddElement`. See `ElementDescription` for
/// more information about what fields are inputs to `AddElement`.
///
/// Error
///   - Error::DEPENDENCY_NOT_FOUND if there is a dependency specified by
///     `config` which is to found in `tokens`.
///   - Error::INVALID_ARGS if `config` appears to be invalid, we fail to
///     duplicate a token and therefore assume it must have been invalid, or
///     the call to power broker fails for any reason *other* than a closed
///     channel.
fit::result<Error> AddElement(fidl::ClientEnd<fuchsia_power_broker::Topology>& power_broker,
                              ElementDesc& description);

/// Helper to acquire a lease on an element using `fuchsia.power.broker/Topology.Lease`.
/// Clients should create an eventpair and pass one end as `broker_lease_token` to be held by the
/// broker. The other end should be retained by the client. The lease will remain open until the
/// client's end is dropped.
fit::result<Error> AcquireLease(
    const fidl::UnownedClientEnd<fuchsia_power_broker::Topology>& topology,
    zx::event dependency_token, fuchsia_power_broker::PowerLevel level, std::string_view lease_name,
    bool should_return_pending_lease, fuchsia_power_broker::LeaseToken broker_lease_token);
}  // namespace fdf_power

#endif

#endif  // LIB_DRIVER_POWER_CPP_POWER_SUPPORT_H_
