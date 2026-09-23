// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bss::BssDescription;
use crate::mac::MacRole;
use crate::security::SecurityDescriptor;
use anyhow::format_err;
use fidl_fuchsia_wlan_sme as fidl_sme;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::error;
use std::fmt::{self, Display, Formatter};

#[cfg(target_os = "fuchsia")]
use anyhow::Context as _;

/// The compatibility of a BSS with respect to a scanning interface.
///
/// Describes the possible configurations for connection to a compatible BSS or disjoint features
/// that prevent a connection to an incompatible BSS. Here, _compatibility_ refers to the ability
/// to establish a connection.
///
/// When compatibility is `Err` for a BSS, then the scanning interface cannot establish a
/// connection.
pub type Compatibility = Result<Compatible, Incompatible>;

pub trait CompatibilityExt: Sized {
    fn try_from_fidl(
        compatibility: fidl_sme::Compatibility,
    ) -> Result<Self, fidl_sme::Compatibility>;

    fn into_fidl(self) -> fidl_sme::Compatibility;
}

impl CompatibilityExt for Compatibility {
    fn try_from_fidl(
        compatibility: fidl_sme::Compatibility,
    ) -> Result<Self, fidl_sme::Compatibility> {
        match compatibility {
            fidl_sme::Compatibility::Compatible(compatible) => Compatible::try_from(compatible)
                .map(Ok)
                .map_err(fidl_sme::Compatibility::Compatible),
            fidl_sme::Compatibility::Incompatible(incompatible) => {
                Incompatible::try_from(incompatible)
                    .map(Err)
                    .map_err(fidl_sme::Compatibility::Incompatible)
            }
        }
    }

    fn into_fidl(self) -> fidl_sme::Compatibility {
        match self {
            Ok(compatible) => fidl_sme::Compatibility::Compatible(compatible.into()),
            Err(incompatible) => fidl_sme::Compatibility::Incompatible(incompatible.into()),
        }
    }
}

/// Supported configurations for a compatible BSS with respect to a scanning interface.
///
/// Describes the mutually supported features between a compatible BSS and a local scanning
/// interface that can be negotiated and/or used to establish a connection.
#[derive(Debug, Clone, PartialEq)]
pub struct Compatible {
    mutual_security_protocols: HashSet<SecurityDescriptor>,
}

impl Compatible {
    /// Constructs a `Compatible` from mutually supported features.
    ///
    /// Returns `None` if any set of mutually supported features is empty, because this implies
    /// incompatibility.
    ///
    /// Note that the features considered by `Compatible` depend on the needs of downstream code
    /// and may change. This function accepts parameters that represent only these features, which
    /// may be as few in number as one and may grow to many.
    pub fn try_from_features(
        mutual_security_protocols: impl IntoIterator<Item = SecurityDescriptor>,
    ) -> Option<Self> {
        let mutual_security_protocols: HashSet<_> = mutual_security_protocols.into_iter().collect();
        if mutual_security_protocols.is_empty() {
            None
        } else {
            Some(Compatible { mutual_security_protocols })
        }
    }

    /// Constructs a [`Compatibility`] from a `Compatible` from mutually supported features.
    ///
    /// While this function presents a fallible interface and returns a `Compatibility` (`Result`),
    /// it panics on failure and never returns `Err`. This can be used when a `Compatibility` is
    /// needed but it is important to assert that it is compatible (`Ok`), most notably in tests.
    ///
    /// See [`Compatible::try_from_features`].
    ///
    /// # Panics
    ///
    /// Panics if a `Compatible` cannot be constructed from the given mutually supported features.
    /// This occurs if `Compatible::try_from_features` returns `None`.
    pub fn expect_ok(
        mutual_security_protocols: impl IntoIterator<Item = SecurityDescriptor>,
    ) -> Compatibility {
        match Compatible::try_from_features(mutual_security_protocols) {
            Some(compatibility) => Ok(compatibility),
            None => panic!("mutual modes of operation are absent and imply incompatiblity"),
        }
    }

    /// Gets the set of mutually supported security protocols.
    ///
    /// This set represents the intersection of security protocols supported by the BSS and the
    /// scanning interface. In this context, this set is never empty, as that would imply
    /// incompatibility.
    pub fn mutual_security_protocols(&self) -> &HashSet<SecurityDescriptor> {
        &self.mutual_security_protocols
    }
}

impl From<Compatible> for fidl_sme::Compatible {
    fn from(compatibility: Compatible) -> Self {
        let Compatible { mutual_security_protocols } = compatibility;
        fidl_sme::Compatible {
            mutual_security_protocols: mutual_security_protocols
                .into_iter()
                .map(From::from)
                .collect(),
        }
    }
}

impl From<Compatible> for HashSet<SecurityDescriptor> {
    fn from(compatibility: Compatible) -> Self {
        compatibility.mutual_security_protocols
    }
}

impl TryFrom<fidl_sme::Compatible> for Compatible {
    type Error = fidl_sme::Compatible;

    fn try_from(compatibility: fidl_sme::Compatible) -> Result<Self, Self::Error> {
        let fidl_sme::Compatible { mutual_security_protocols } = compatibility;
        match Compatible::try_from_features(
            mutual_security_protocols.iter().cloned().map(From::from),
        ) {
            Some(compatible) => Ok(compatible),
            None => Err(fidl_sme::Compatible { mutual_security_protocols }),
        }
    }
}

// TODO(https://fxbug.dev/384797729): Consider supported channels and data rates.
/// Unsupported configurations for an incompatible BSS with respect to a scanning interface.
///
/// Describes disjoint features between an incompatible BSS and a local scanning interface that
/// prevent establishing a connection. Information about modes of operation is best effort;
/// `Incompatible` may provide no additional information at all.
#[derive(Debug, Clone, PartialEq)]
pub struct Incompatible {
    description: Cow<'static, str>,
    disjoint_security_protocols: Option<HashMap<SecurityDescriptor, MacRole>>,
}

impl Incompatible {
    /// Constructs an `Incompatible` from a description with no feature information.
    pub fn from_description(description: impl Into<Cow<'static, str>>) -> Self {
        Incompatible { description: description.into(), disjoint_security_protocols: None }
    }

    /// Constructs an `Incompatible` from a description and disjoint features.
    ///
    /// Returns `None` if any given features are **not** disjoint. For example, `None` is returned
    /// if a security protocol appears more than once with differing roles, because this implies
    /// compatibility (a mutually supported security protocol).
    ///
    /// Note that the features considered by `Incompatible` depend on the needs of downstream code
    /// and may change. This function accepts parameters that represent only these features, which
    /// may be as few in number as one and may grow to many.
    pub fn try_from_features(
        description: impl Into<Cow<'static, str>>,
        disjoint_security_protocols: Option<
            impl IntoIterator<Item = (SecurityDescriptor, MacRole)>,
        >,
    ) -> Option<Self> {
        disjoint_security_protocols
            .map(|disjoint_security_protocols| {
                let mut unique_security_protocols = HashMap::new();
                for (descriptor, role) in disjoint_security_protocols {
                    if let Some(previous) = unique_security_protocols.insert(descriptor, role) {
                        if role != previous {
                            return Err(role);
                        }
                    }
                }
                Ok(unique_security_protocols)
            })
            .transpose()
            .ok()
            .map(move |disjoint_security_protocols| Incompatible {
                description: description.into(),
                disjoint_security_protocols,
            })
    }

    /// Constructs a [`Compatibility`] from an `Incompatible` with no feature information.
    pub const fn unknown() -> Compatibility {
        Err(Incompatible {
            description: Cow::Borrowed("unknown"),
            disjoint_security_protocols: None,
        })
    }

    /// Constructs a [`Compatibility`] from an `Incompatible` from disjoint features.
    ///
    /// While this function presents a fallible interface and returns a `Compatibility` (`Result`),
    /// it panics on failure and never returns `Ok`. This can be used when a `Compatibility` is
    /// needed but it is important to assert that it is incompatible (`Err`), most notably in tests.
    ///
    /// See [`Incompatible::try_from_features`].
    ///
    /// # Panics
    ///
    /// Panics if an `Incompatible` cannot be constructed from the given disjoint features. This
    /// occurs if `Incompatible::try_from_features` returns `None`.
    pub fn expect_err(
        description: impl Into<Cow<'static, str>>,
        disjoint_security_protocols: Option<
            impl IntoIterator<Item = (SecurityDescriptor, MacRole)>,
        >,
    ) -> Compatibility {
        match Incompatible::try_from_features(description, disjoint_security_protocols) {
            Some(incompatible) => Err(incompatible),
            None => panic!("disjoint modes of operation intersect and imply compatiblity"),
        }
    }

    /// Gets the sets of disjoint security protocols, if any.
    ///
    /// The disjoint sets are represented as a map from `SecurityDescriptor` to `MacRole`, where
    /// each security protocol is supported only by one station in a particular role (e.g., client
    /// and AP). There is a disjoint set of security protocols for each unique role in the map.
    ///
    /// Returns `None` if no security protocol incompatibility has been detected. When `Some` but
    /// with an empty map, security protocol support is considered incompatible even though the
    /// protocols are not described.
    pub fn disjoint_security_protocols(&self) -> Option<&HashMap<SecurityDescriptor, MacRole>> {
        self.disjoint_security_protocols.as_ref()
    }
}

impl Display for Incompatible {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "incompatibility detected")?;
        if !self.description.is_empty() {
            write!(formatter, ": {}", self.description)?;
        }
        if let Some(disjoint_security_protocols) = self.disjoint_security_protocols() {
            let (client_security_protocols, bss_security_protocols) = disjoint_security_protocols
                .iter()
                .partition::<Vec<_>, _>(|(_, role)| matches!(role, MacRole::Client));
            write!(
                formatter,
                ": supported BSS vs. client security protocols: {:?} vs. {:?}",
                bss_security_protocols, client_security_protocols,
            )?;
        }
        write!(formatter, ".")
    }
}

impl error::Error for Incompatible {}

impl From<Incompatible> for fidl_sme::Incompatible {
    fn from(incompatible: Incompatible) -> Self {
        let Incompatible { description, disjoint_security_protocols } = incompatible;
        fidl_sme::Incompatible {
            description: description.into(),
            disjoint_security_protocols: disjoint_security_protocols.map(
                |disjoint_security_protocols| {
                    disjoint_security_protocols
                        .into_iter()
                        .map(|(security_protocol, role)| fidl_sme::DisjointSecurityProtocol {
                            protocol: security_protocol.into(),
                            role: role.into(),
                        })
                        .collect()
                },
            ),
        }
    }
}

impl TryFrom<fidl_sme::Incompatible> for Incompatible {
    type Error = fidl_sme::Incompatible;

    fn try_from(incompatible: fidl_sme::Incompatible) -> Result<Self, Self::Error> {
        let fidl_sme::Incompatible { description, disjoint_security_protocols } = incompatible;
        match disjoint_security_protocols
            .as_ref()
            .map(|disjoint_security_protocols| {
                disjoint_security_protocols
                    .iter()
                    .copied()
                    .map(|fidl_sme::DisjointSecurityProtocol { protocol, role }| {
                        MacRole::try_from(role).map(|role| (protocol.into(), role))
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()
        {
            Ok(converted_security_protocols) => {
                Incompatible::try_from_features(description.clone(), converted_security_protocols)
                    .ok_or(fidl_sme::Incompatible { description, disjoint_security_protocols })
            }
            Err(_) => Err(fidl_sme::Incompatible { description, disjoint_security_protocols }),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScanResult {
    pub compatibility: Compatibility,
    // Time of the scan result relative to when the system was powered on.
    // See https://fuchsia.dev/fuchsia-src/concepts/time/language_support?hl=en#monotonic_time
    #[cfg(target_os = "fuchsia")]
    pub timestamp: zx::MonotonicInstant,
    pub bss_description: BssDescription,
}

impl ScanResult {
    pub fn is_compatible(&self) -> bool {
        self.compatibility.is_ok()
    }
}

impl From<ScanResult> for fidl_sme::ScanResult {
    fn from(scan_result: ScanResult) -> fidl_sme::ScanResult {
        let ScanResult {
            compatibility,
            #[cfg(target_os = "fuchsia")]
            timestamp,
            bss_description,
        } = scan_result;
        fidl_sme::ScanResult {
            compatibility: compatibility.into_fidl(),
            #[cfg(target_os = "fuchsia")]
            timestamp_nanos: timestamp.into_nanos(),
            #[cfg(not(target_os = "fuchsia"))]
            timestamp_nanos: 0,
            bss_description: bss_description.into(),
        }
    }
}

impl TryFrom<fidl_sme::ScanResult> for ScanResult {
    type Error = anyhow::Error;

    fn try_from(scan_result: fidl_sme::ScanResult) -> Result<ScanResult, Self::Error> {
        let fidl_sme::ScanResult { compatibility, timestamp_nanos, bss_description } = scan_result;
        Ok(ScanResult {
            compatibility: Compatibility::try_from_fidl(compatibility)
                .map_err(|_| format_err!("failed to convert FIDL `Compatibility`"))?,
            #[cfg(target_os = "fuchsia")]
            timestamp: zx::MonotonicInstant::from_nanos(timestamp_nanos),
            bss_description: bss_description.try_into()?,
        })
    }
}

/// Creates a VMO containing FIDL-encoded scan results.
#[cfg(target_os = "fuchsia")]
pub fn write_vmo(results: Vec<fidl_sme::ScanResult>) -> Result<fidl::Vmo, anyhow::Error> {
    let bytes =
        fidl::persist(&fidl_sme::ScanResultVector { results }).context("encoding scan results")?;
    let vmo = fidl::Vmo::create(bytes.len() as u64).context("creating VMO for scan results")?;
    vmo.write(&bytes, 0).context("writing scan results to VMO")?;
    Ok(vmo)
}

/// Reads FIDL-encoded scan results from a VMO.
#[cfg(target_os = "fuchsia")]
pub fn read_vmo(vmo: fidl::Vmo) -> Result<Vec<fidl_sme::ScanResult>, anyhow::Error> {
    let size = vmo.get_content_size().context("getting VMO content size")?;
    let bytes = vmo.read_to_vec(0, size).context("reading VMO of scan results")?;
    let scan_result_vector =
        fidl::unpersist::<fidl_sme::ScanResultVector>(&bytes).context("decoding scan results")?;
    Ok(scan_result_vector.results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatible_from_only_empty_is_none() {
        assert!(Compatible::try_from_features([]).is_none());
    }

    #[test]
    fn compatible_from_mutual_security_protocols_is_some() {
        assert!(Compatible::try_from_features([
            SecurityDescriptor::WPA2_PERSONAL,
            SecurityDescriptor::WPA3_PERSONAL,
        ])
        .is_some());
    }

    #[test]
    fn incompatible_from_only_none_is_some() {
        assert!(Incompatible::try_from_features(
            "dunno",
            None::<[(SecurityDescriptor, MacRole); 0]>
        )
        .is_some());
    }

    #[test]
    fn incompatible_from_only_some_empty_is_some() {
        assert!(Incompatible::try_from_features("dunno", Some([])).is_some());
    }

    #[test]
    fn incompatible_from_disjoint_security_protocols_is_some() {
        assert!(Incompatible::try_from_features(
            "dunno",
            Some([
                (SecurityDescriptor::WPA2_PERSONAL, MacRole::Client),
                (SecurityDescriptor::WPA3_PERSONAL, MacRole::Ap),
            ])
        )
        .is_some());
    }

    #[test]
    fn incompatible_from_mutual_security_protocols_is_none() {
        assert!(Incompatible::try_from_features(
            "dunno",
            Some([
                (SecurityDescriptor::WPA3_PERSONAL, MacRole::Client),
                (SecurityDescriptor::WPA3_PERSONAL, MacRole::Ap),
            ])
        )
        .is_none());
    }

    #[test]
    fn fidl_from_compatible_eq_expected() {
        let security_protocol = SecurityDescriptor::OPEN;
        let fidl =
            fidl_sme::Compatible::from(Compatible::try_from_features([security_protocol]).unwrap());
        assert_eq!(
            fidl,
            fidl_sme::Compatible { mutual_security_protocols: vec![security_protocol.into()] },
        );
    }

    #[test]
    fn compatible_try_from_fidl_eq_ok() {
        let security_protocol = SecurityDescriptor::OPEN;
        let compatible = Compatible::try_from(fidl_sme::Compatible {
            mutual_security_protocols: vec![security_protocol.into()],
        });
        assert_eq!(compatible, Ok(Compatible::try_from_features([security_protocol]).unwrap()));
    }

    #[test]
    fn compatible_try_from_fidl_eq_err() {
        let fidl = fidl_sme::Compatible { mutual_security_protocols: vec![] };
        let compatible = Compatible::try_from(fidl.clone());
        assert_eq!(compatible, Err(fidl));
    }

    #[test]
    fn fidl_from_incompatible_eq_expected() {
        let fidl = fidl_sme::Incompatible::from(
            Incompatible::try_from_features(
                "dunno",
                // Only one protocol-role entry is used here, because entries are stored in a
                // `HashMap` and ordering is arbitrary when this is converted into a `Vec` in the
                // FIDL representation. This causes spurious errors, since `[a, b]` does not equal
                // `[b, a]`, though they are semantically equivalent here.
                Some([(SecurityDescriptor::WPA3_PERSONAL, MacRole::Ap)]),
            )
            .unwrap(),
        );
        assert_eq!(
            fidl,
            fidl_sme::Incompatible {
                description: String::from("dunno"),
                disjoint_security_protocols: Some(vec![fidl_sme::DisjointSecurityProtocol {
                    protocol: SecurityDescriptor::WPA3_PERSONAL.into(),
                    role: MacRole::Ap.into(),
                },]),
            },
        );
    }

    #[test]
    fn incompatible_try_from_fidl_eq_expected() {
        let incompatible = Incompatible::try_from(fidl_sme::Incompatible {
            description: String::from("dunno"),
            disjoint_security_protocols: Some(vec![
                fidl_sme::DisjointSecurityProtocol {
                    protocol: SecurityDescriptor::WPA2_PERSONAL.into(),
                    role: MacRole::Client.into(),
                },
                fidl_sme::DisjointSecurityProtocol {
                    protocol: SecurityDescriptor::WPA3_PERSONAL.into(),
                    role: MacRole::Ap.into(),
                },
            ]),
        });
        assert_eq!(
            incompatible,
            Ok(Incompatible::try_from_features(
                "dunno",
                Some([
                    (SecurityDescriptor::WPA2_PERSONAL, MacRole::Client),
                    (SecurityDescriptor::WPA3_PERSONAL, MacRole::Ap),
                ])
            )
            .unwrap()),
        );
    }

    #[test]
    fn fidl_from_compatibility_eq_expected() {
        let security_protocol = SecurityDescriptor::OPEN;
        let fidl = Compatible::expect_ok([security_protocol]).into_fidl();
        assert_eq!(
            fidl,
            fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                mutual_security_protocols: vec![security_protocol.into()],
            }),
        );
    }

    #[test]
    fn compatibility_try_from_fidl_eq_ok() {
        let security_protocol = SecurityDescriptor::OPEN;
        let compatibility = Compatibility::try_from_fidl(fidl_sme::Compatibility::Compatible(
            fidl_sme::Compatible { mutual_security_protocols: vec![security_protocol.into()] },
        ));
        assert_eq!(compatibility, Ok(Compatible::expect_ok([security_protocol])));
    }

    #[test]
    fn compatibility_try_from_fidl_eq_err() {
        let fidl = fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
            mutual_security_protocols: vec![],
        });
        let compatibility = Compatibility::try_from_fidl(fidl.clone());
        assert_eq!(compatibility, Err(fidl));
    }
}
