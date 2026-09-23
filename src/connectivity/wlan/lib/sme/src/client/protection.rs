// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::ClientConfig;
use crate::client::rsn::Rsna;
use anyhow::{Error, format_err};
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_mlme::DeviceInfo;
use wlan_common::bss::BssDescription;
use wlan_common::ie::rsn::rsne::{self, Rsne};
use wlan_common::ie::wpa::WpaIe;
use wlan_common::ie::{self};
use wlan_common::security::wep::{self, WepKey};
use wlan_common::security::{SecurityAuthenticator, wpa};
use wlan_rsn::auth::psk::ToPsk;
use wlan_rsn::auth::{self};
use wlan_rsn::nonce::NonceReader;
use wlan_rsn::{NegotiatedProtection, ProtectionInfo, PweMethod};

#[derive(Debug, PartialEq)]
pub enum Protection {
    Open,
    Wep(WepKey),
    // WPA1 is based off of a modified pre-release version of IEEE 802.11i. It is similar enough
    // that we can reuse the existing RSNA implementation rather than duplicating large pieces of
    // logic.
    LegacyWpa(Rsna),
    Rsna(Rsna),
}

impl Protection {
    pub fn rsn_auth_method(&self) -> Option<auth::MethodName> {
        let rsna = match self {
            Self::LegacyWpa(rsna) => rsna,
            Self::Rsna(rsna) => rsna,
            // Neither WEP or Open use an RSN, so None is returned.
            Self::Wep(_) | Self::Open => {
                return None;
            }
        };

        Some(rsna.supplicant.get_auth_method())
    }
}

#[derive(Debug)]
pub enum ProtectionIe {
    Rsne(Vec<u8>),
    VendorIes(Vec<u8>),
}

/// Context for authentication.
///
/// This ephemeral type is used to query and derive various IEs and RSN entities based on
/// parameterized security data, the configured client, and the target BSS. This is exposed to
/// client code via `TryFrom` implementations, which allow a context to be converted into a
/// negotiated `Protection`. These conversions fail if the combination of an authenticator and a
/// network is incompatible.
///
/// The type parameter `C` represents the parameterized security data and is either a type
/// representing credential data or a `SecurityAuthenticator`.
///
/// # Examples
///
/// To derive a `Protection`, construct a `SecurityContext` using a `SecurityAuthenticator` and
/// perform a conversion.
///
/// ```rust,ignore
/// // See the documentation for `SecurityAuthenticator` for more details.
/// let authenticator = SecurityAuthenticator::try_from(authentication)?;
/// let protection = Protection::try_from(SecurityContext {
///     security: &authenticator,
///     device: &device, // Device information.
///     security_support: &security_support, // Security features.
///     config: &config, // Client configuration.
///     bss: &bss, // BSS description.
/// })?;
/// ```
#[derive(Clone, Copy, Debug)]
pub struct SecurityContext<'a, C> {
    /// Contextual security data. This field has security-related
    pub security: &'a C,
    pub device: &'a DeviceInfo,
    pub security_support: &'a fidl_common::SecuritySupport,
    pub config: &'a ClientConfig,
    pub bss: &'a BssDescription,
}

impl<'a, C> SecurityContext<'a, C> {
    /// Gets a context with a subject replaced by the given subject. Other fields are unmodified.
    fn map<U>(&self, subject: &'a U) -> SecurityContext<'a, U> {
        SecurityContext {
            security: subject,
            device: self.device,
            security_support: self.security_support,
            config: self.config,
            bss: self.bss,
        }
    }
}

struct OweAuthenticator;

impl SecurityContext<'_, OweAuthenticator> {
    /// Gets the authenticator and supplicant RSNEs for OWE from the associated BSS.
    fn authenticator_supplicant_rsne(&self) -> Result<(Rsne, Rsne), Error> {
        let a_rsne_ie = self
            .bss
            .rsne()
            .ok_or_else(|| format_err!("OWE requested but RSNE is not present in BSS."))?;
        let (_, a_rsne) = rsne::from_bytes(a_rsne_ie)
            .map_err(|error| format_err!("Invalid RSNE IE {:02x?}: {:?}", a_rsne_ie, error))?;
        let s_rsne = a_rsne.derive_owe_s_rsne(self.security_support)?;
        Ok((a_rsne, s_rsne))
    }

    fn authentication_config(&self) -> Result<auth::Config, Error> {
        Ok(auth::Config::Owe)
    }
}

impl SecurityContext<'_, wpa::Wpa1Credentials> {
    /// Gets the authenticator and supplicant IEs for WPA1 from the associated BSS.
    fn authenticator_supplicant_ie(&self) -> Result<(WpaIe, WpaIe), Error> {
        let a_wpa_ie = self.bss.wpa_ie()?;
        if !crate::client::wpa::is_legacy_wpa_compatible(&a_wpa_ie) {
            return Err(format_err!("Legacy WPA requested but IE is incompatible: {:?}", a_wpa_ie));
        }
        let s_wpa_ie = crate::client::wpa::construct_s_wpa(&a_wpa_ie);
        Ok((a_wpa_ie, s_wpa_ie))
    }

    /// Gets the PSK used to authenticate via WPA1.
    fn authentication_config(&self) -> auth::Config {
        auth::Config::ComputedPsk(self.security.to_psk(&self.bss.ssid).into())
    }
}

impl SecurityContext<'_, wpa::Wpa2PersonalCredentials> {
    /// Gets the authenticator and supplicant RSNEs for WPA2 Personal from the associated BSS.
    fn authenticator_supplicant_rsne(&self) -> Result<(Rsne, Rsne), Error> {
        let a_rsne_ie = self
            .bss
            .rsne()
            .ok_or_else(|| format_err!("WPA2 requested but RSNE is not present in BSS."))?;
        let (_, a_rsne) = rsne::from_bytes(a_rsne_ie)
            .map_err(|error| format_err!("Invalid RSNE IE {:02x?}: {:?}", a_rsne_ie, error))?;
        let s_rsne = a_rsne.derive_wpa2_s_rsne(self.security_support)?;
        Ok((a_rsne, s_rsne))
    }

    /// Gets the PSK used to authenticate via WPA2 Personal.
    fn authentication_config(&self) -> auth::Config {
        auth::Config::ComputedPsk(self.security.to_psk(&self.bss.ssid).into())
    }
}

impl SecurityContext<'_, wpa::Wpa3PersonalCredentials> {
    /// Gets the authenticator and supplicant RSNEs for WPA3 Personal from the associated BSS.
    fn authenticator_supplicant_rsne(&self) -> Result<(Rsne, Rsne), Error> {
        let a_rsne_ie = self
            .bss
            .rsne()
            .ok_or_else(|| format_err!("WPA3 requested but RSNE is not present in BSS."))?;
        let (_, a_rsne) = rsne::from_bytes(a_rsne_ie)
            .map_err(|error| format_err!("Invalid RSNE IE {:02x?}: {:?}", a_rsne_ie, error))?;
        let s_rsne = a_rsne.derive_wpa3_s_rsne(self.security_support)?;
        Ok((a_rsne, s_rsne))
    }

    /// Gets the SAE used to authenticate via WPA3 Personal.
    fn authentication_config(&self) -> Result<auth::Config, Error> {
        match self.security {
            wpa::Wpa3PersonalCredentials::Passphrase(passphrase) => {
                // Prefer SAE in SME.
                if self
                    .security_support
                    .sae
                    .as_ref()
                    .and_then(|sae| sae.sme_handler_supported)
                    .unwrap_or(false)
                {
                    let direct_hash_supported_by_peer = self
                        .bss
                        .rsnxe()
                        .and_then(|rsnxe| rsnxe.rsnxe_octet_1)
                        .map(|octet| octet.sae_hash_to_element())
                        .unwrap_or(false);
                    let direct_hash_supported_by_driver = self
                        .security_support
                        .sae
                        .as_ref()
                        .and_then(|sae| sae.hash_to_element_supported)
                        .unwrap_or(false);
                    Ok(auth::Config::Sae {
                        ssid: self.bss.ssid.clone(),
                        password: passphrase.clone().into(),
                        mac: self.device.sta_addr.into(),
                        peer_mac: self.bss.bssid.into(),
                        pwe_method: if direct_hash_supported_by_peer
                            && direct_hash_supported_by_driver
                        {
                            PweMethod::Direct
                        } else {
                            PweMethod::Loop
                        },
                    })
                } else if self
                    .security_support
                    .sae
                    .as_ref()
                    .and_then(|sae| sae.driver_handler_supported)
                    .unwrap_or(false)
                {
                    Ok(auth::Config::DriverSae { password: passphrase.clone().into() })
                } else {
                    Err(format_err!(
                        "Failed to generate WPA3 authentication config: no SAE SME nor driver \
                         handler"
                    ))
                }
            }
        }
    }
}

impl<'a> TryFrom<SecurityContext<'a, SecurityAuthenticator>> for Protection {
    type Error = Error;

    fn try_from(context: SecurityContext<'a, SecurityAuthenticator>) -> Result<Self, Self::Error> {
        match context.security {
            SecurityAuthenticator::Open => context
                .bss
                .is_open()
                .then(|| Protection::Open)
                .ok_or_else(|| format_err!("BSS is not configured for open authentication")),
            SecurityAuthenticator::Owe => context.map(&OweAuthenticator).try_into(),
            SecurityAuthenticator::Wep(authenticator) => context.map(authenticator).try_into(),
            SecurityAuthenticator::Wpa(wpa) => match wpa {
                wpa::WpaAuthenticator::Wpa1 { credentials, .. } => {
                    context.map(credentials).try_into()
                }
                wpa::WpaAuthenticator::Wpa2 { authentication, .. } => match authentication {
                    wpa::Authentication::Personal(personal) => context.map(personal).try_into(),
                    // TODO(https://fxbug.dev/42174395): Implement conversions for WPA Enterprise.
                    _ => Err(format_err!("WPA Enterprise is unsupported")),
                },
                wpa::WpaAuthenticator::Wpa3 { authentication, .. } => match authentication {
                    wpa::Authentication::Personal(personal) => context.map(personal).try_into(),
                    // TODO(https://fxbug.dev/42174395): Implement conversions for WPA Enterprise.
                    _ => Err(format_err!("WPA Enterprise is unsupported")),
                },
            },
        }
    }
}

impl<'a> TryFrom<SecurityContext<'a, OweAuthenticator>> for Protection {
    type Error = Error;

    fn try_from(context: SecurityContext<'a, OweAuthenticator>) -> Result<Self, Self::Error> {
        context
            .bss
            .has_owe_configured()
            .then(|| -> Result<_, Self::Error> {
                let sta_addr = context.device.sta_addr.into();
                if !context.config.owe_supported {
                    return Err(format_err!("OWE requested but client does not support OWE"));
                }
                let (a_rsne, s_rsne) = context.authenticator_supplicant_rsne()?;
                let negotiated_protection = NegotiatedProtection::from_rsne(&s_rsne)?;
                let supplicant = wlan_rsn::Supplicant::new_wpa_personal(
                    NonceReader::new(&sta_addr)?,
                    context.authentication_config()?,
                    sta_addr,
                    ProtectionInfo::Rsne(s_rsne),
                    context.bss.bssid.into(),
                    ProtectionInfo::Rsne(a_rsne),
                    false,
                )
                .map_err(|error| format_err!("Failed to create ESS-SA: {:?}", error))?;
                Ok(Protection::Rsna(Rsna {
                    negotiated_protection,
                    supplicant: Box::new(supplicant),
                }))
            })
            .transpose()?
            .ok_or_else(|| format_err!("BSS is not configured for OWE"))
    }
}

impl<'a> TryFrom<SecurityContext<'a, wep::WepAuthenticator>> for Protection {
    type Error = Error;

    fn try_from(context: SecurityContext<'a, wep::WepAuthenticator>) -> Result<Self, Self::Error> {
        context
            .bss
            .has_wep_configured()
            .then(|| Protection::Wep(context.security.key.clone()))
            .ok_or_else(|| format_err!("BSS is not configured for WEP"))
    }
}

impl<'a> TryFrom<SecurityContext<'a, wpa::Wpa1Credentials>> for Protection {
    type Error = Error;

    fn try_from(context: SecurityContext<'a, wpa::Wpa1Credentials>) -> Result<Self, Self::Error> {
        context
            .bss
            .has_wpa1_configured()
            .then(|| -> Result<_, Self::Error> {
                let sta_addr = context.device.sta_addr.into();
                let (a_wpa_ie, s_wpa_ie) = context.authenticator_supplicant_ie()?;
                let negotiated_protection = NegotiatedProtection::from_legacy_wpa(&s_wpa_ie)?;
                let supplicant = wlan_rsn::Supplicant::new_wpa_personal(
                    NonceReader::new(&sta_addr)?,
                    context.authentication_config(),
                    sta_addr,
                    ProtectionInfo::LegacyWpa(s_wpa_ie),
                    context.bss.bssid.into(),
                    ProtectionInfo::LegacyWpa(a_wpa_ie),
                    false,
                )
                .map_err(|error| format_err!("Failed to create ESS-SA: {:?}", error))?;
                Ok(Protection::LegacyWpa(Rsna {
                    negotiated_protection,
                    supplicant: Box::new(supplicant),
                }))
            })
            .transpose()?
            .ok_or_else(|| format_err!("BSS is not configured for WPA1"))
    }
}

impl<'a> TryFrom<SecurityContext<'a, wpa::Wpa2PersonalCredentials>> for Protection {
    type Error = Error;

    fn try_from(
        context: SecurityContext<'a, wpa::Wpa2PersonalCredentials>,
    ) -> Result<Self, Self::Error> {
        context
            .bss
            .has_wpa2_personal_configured()
            .then(|| -> Result<_, Self::Error> {
                let sta_addr = context.device.sta_addr.into();
                let (a_rsne, s_rsne) = context.authenticator_supplicant_rsne()?;
                let negotiated_protection = NegotiatedProtection::from_rsne(&s_rsne)?;
                let supplicant = wlan_rsn::Supplicant::new_wpa_personal(
                    NonceReader::new(&sta_addr)?,
                    context.authentication_config(),
                    sta_addr,
                    ProtectionInfo::Rsne(s_rsne),
                    context.bss.bssid.into(),
                    ProtectionInfo::Rsne(a_rsne),
                    false,
                )
                .map_err(|error| format_err!("Failed to creat ESS-SA: {:?}", error))?;
                Ok(Protection::Rsna(Rsna {
                    negotiated_protection,
                    supplicant: Box::new(supplicant),
                }))
            })
            .transpose()?
            .ok_or_else(|| format_err!("BSS is not configured for WPA2 Personal"))
    }
}

impl<'a> TryFrom<SecurityContext<'a, wpa::Wpa3PersonalCredentials>> for Protection {
    type Error = Error;

    fn try_from(
        context: SecurityContext<'a, wpa::Wpa3PersonalCredentials>,
    ) -> Result<Self, Self::Error> {
        context
            .bss
            .has_wpa3_personal_configured()
            .then(|| -> Result<_, Self::Error> {
                let sta_addr = context.device.sta_addr.into();
                if !context.config.wpa3_supported {
                    return Err(format_err!("WPA3 requested but client does not support WPA3"));
                }
                let (a_rsne, s_rsne) = context.authenticator_supplicant_rsne()?;
                let negotiated_protection = NegotiatedProtection::from_rsne(&s_rsne)?;
                let pmksa_caching_supported = context
                    .security_support
                    .sae
                    .as_ref()
                    .and_then(|sae| sae.pmksa_caching_supported)
                    .unwrap_or(false);
                let supplicant = wlan_rsn::Supplicant::new_wpa_personal(
                    NonceReader::new(&sta_addr)?,
                    context.authentication_config()?,
                    sta_addr,
                    ProtectionInfo::Rsne(s_rsne),
                    context.bss.bssid.into(),
                    ProtectionInfo::Rsne(a_rsne),
                    pmksa_caching_supported,
                )
                .map_err(|error| format_err!("Failed to create ESS-SA: {:?}", error))?;
                Ok(Protection::Rsna(Rsna {
                    negotiated_protection,
                    supplicant: Box::new(supplicant),
                }))
            })
            .transpose()?
            .ok_or_else(|| format_err!("BSS is not configured for WPA3 Personal"))
    }
}

/// Based on the type of protection, derive either RSNE or Vendor IEs:
/// No Protection or WEP: Neither
/// WPA2: RSNE
/// WPA1: Vendor IEs
pub(crate) fn build_protection_ie(protection: &Protection) -> Result<Option<ProtectionIe>, Error> {
    match protection {
        Protection::Open | Protection::Wep(_) => Ok(None),
        Protection::LegacyWpa(rsna) => {
            let s_protection = rsna.negotiated_protection.to_full_protection();
            let s_wpa = match s_protection {
                ProtectionInfo::Rsne(_) => {
                    return Err(format_err!("found RSNE protection inside a WPA1 association..."));
                }
                ProtectionInfo::LegacyWpa(wpa) => wpa,
            };
            let mut buf = vec![];
            #[expect(clippy::unwrap_used)]
            ie::write_wpa1_ie(&mut buf, &s_wpa).unwrap(); // Writing to a Vec never fails
            Ok(Some(ProtectionIe::VendorIes(buf)))
        }
        Protection::Rsna(rsna) => {
            let s_protection = rsna.negotiated_protection.to_full_protection();
            let s_rsne = match s_protection {
                ProtectionInfo::Rsne(rsne) => rsne,
                ProtectionInfo::LegacyWpa(_) => {
                    return Err(format_err!("found WPA protection inside an RSNA..."));
                }
            };
            let mut buf = Vec::with_capacity(s_rsne.len());
            // Writing an RSNE into a Vector can never fail as a Vector can be grown when more
            // space is required. If this panic ever triggers, something is clearly broken
            // somewhere else.
            #[expect(clippy::unwrap_used)]
            let () = s_rsne.write_into(&mut buf).unwrap();
            Ok(Some(ProtectionIe::Rsne(buf)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{self};
    use assert_matches::assert_matches;
    use test_case::test_case;
    use wlan_common::fake_bss_description;
    use wlan_common::ie::fake_ies::fake_wpa_ie;
    use wlan_common::ie::rsn::fake_rsnes::{fake_wpa2_s_rsne, fake_wpa3_s_rsne};
    use wlan_common::security::wep::{WEP40_KEY_BYTES, WEP104_KEY_BYTES};
    use wlan_common::security::wpa::credential::PSK_SIZE_BYTES;
    use wlan_common::test_utils::fake_features::{
        fake_security_support, fake_security_support_empty,
    };

    #[test]
    fn rsn_auth_method() {
        // Open
        let protection = Protection::Open;
        assert!(protection.rsn_auth_method().is_none());

        // Wep
        let protection = Protection::Wep(WepKey::parse([1; 5]).expect("unable to parse WEP key"));
        assert!(protection.rsn_auth_method().is_none());

        // WPA1
        let protection_info = ProtectionInfo::LegacyWpa(fake_wpa_ie());
        let negotiated_protection = NegotiatedProtection::from_protection(&protection_info)
            .expect("could create mocked WPA1 NegotiatedProtection");
        let protection = Protection::LegacyWpa(Rsna {
            negotiated_protection,
            supplicant: Box::new(client::test_utils::mock_psk_supplicant().0),
        });
        assert_eq!(protection.rsn_auth_method(), Some(auth::MethodName::Psk));

        // WPA2
        let protection_info = ProtectionInfo::Rsne(fake_wpa2_s_rsne());
        let negotiated_protection = NegotiatedProtection::from_protection(&protection_info)
            .expect("could create mocked WPA2 NegotiatedProtection");
        let protection = Protection::Rsna(Rsna {
            negotiated_protection,
            supplicant: Box::new(client::test_utils::mock_psk_supplicant().0),
        });
        assert_eq!(protection.rsn_auth_method(), Some(auth::MethodName::Psk));

        // WPA3
        let protection_info = ProtectionInfo::Rsne(fake_wpa3_s_rsne());
        let negotiated_protection = NegotiatedProtection::from_protection(&protection_info)
            .expect("could create mocked WPA3 NegotiatedProtection");
        let protection = Protection::Rsna(Rsna {
            negotiated_protection,
            supplicant: Box::new(client::test_utils::mock_sae_supplicant().0),
        });
        assert_eq!(protection.rsn_auth_method(), Some(auth::MethodName::Sae));

        // OWE
        let protection_info = ProtectionInfo::Rsne(Rsne::common_owe_rsne());
        let negotiated_protection = NegotiatedProtection::from_protection(&protection_info)
            .expect("could create mocked OWE NegotiatedProtection");
        let protection = Protection::Rsna(Rsna {
            negotiated_protection,
            supplicant: Box::new(client::test_utils::mock_owe_supplicant().0),
        });
        assert_eq!(protection.rsn_auth_method(), Some(auth::MethodName::Owe));
    }

    #[test]
    fn protection_from_wep40() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = Default::default();
        let bss = fake_bss_description!(Wep);
        let authenticator = wep::WepAuthenticator { key: WepKey::from([1u8; WEP40_KEY_BYTES]) };
        let protection = Protection::try_from(SecurityContext {
            security: &authenticator,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        })
        .unwrap();
        assert!(matches!(protection, Protection::Wep(_)));
    }

    #[test]
    fn protection_from_wep104() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = Default::default();
        let bss = fake_bss_description!(Wep);
        let authenticator = wep::WepAuthenticator { key: WepKey::from([1u8; WEP104_KEY_BYTES]) };
        let protection = Protection::try_from(SecurityContext {
            security: &authenticator,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        })
        .unwrap();
        assert!(matches!(protection, Protection::Wep(_)));
    }

    #[test]
    fn protection_from_wpa1_psk() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = Default::default();
        let bss = fake_bss_description!(Wpa1);
        let credentials = wpa::Wpa1Credentials::Psk([1u8; PSK_SIZE_BYTES].into());
        let protection = Protection::try_from(SecurityContext {
            security: &credentials,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        })
        .unwrap();
        assert!(matches!(protection, Protection::LegacyWpa(_)));
    }

    #[test]
    fn protection_from_wpa2_personal_psk() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = Default::default();
        let bss = fake_bss_description!(Wpa2);
        let credentials = wpa::Wpa2PersonalCredentials::Psk([1u8; PSK_SIZE_BYTES].into());
        let protection = Protection::try_from(SecurityContext {
            security: &credentials,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        })
        .unwrap();
        assert!(matches!(protection, Protection::Rsna(_)));
    }

    #[test]
    fn protection_from_wpa2_personal_passphrase() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = Default::default();
        let bss = fake_bss_description!(Wpa2);
        let credentials =
            wpa::Wpa2PersonalCredentials::Passphrase("password".as_bytes().try_into().unwrap());
        let protection = Protection::try_from(SecurityContext {
            security: &credentials,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        })
        .unwrap();
        assert!(matches!(protection, Protection::Rsna(_)));
    }

    #[test]
    fn protection_from_wpa2_personal_tkip_only_passphrase() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = Default::default();
        let bss = fake_bss_description!(Wpa2TkipOnly);
        let credentials =
            wpa::Wpa2PersonalCredentials::Passphrase("password".as_bytes().try_into().unwrap());
        let protection = Protection::try_from(SecurityContext {
            security: &credentials,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        })
        .unwrap();
        assert!(matches!(protection, Protection::Rsna(_)));
    }

    #[test]
    fn protection_from_wpa3_personal_passphrase() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = ClientConfig { wpa3_supported: true, ..Default::default() };
        let bss = fake_bss_description!(Wpa3);
        let credentials =
            wpa::Wpa3PersonalCredentials::Passphrase("password".as_bytes().try_into().unwrap());
        let protection = Protection::try_from(SecurityContext {
            security: &credentials,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        })
        .unwrap();
        assert!(matches!(protection, Protection::Rsna(_)));
    }

    #[test]
    fn protection_from_owe() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = ClientConfig { owe_supported: true, ..Default::default() };
        let bss = fake_bss_description!(Owe);
        let security = SecurityAuthenticator::Owe;
        let protection = Protection::try_from(SecurityContext {
            security: &security,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        })
        .unwrap();
        assert!(matches!(protection, Protection::Rsna(_)));
    }

    #[test]
    fn protection_from_owe_no_security_support_features() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support_empty();
        let config = Default::default();
        let bss = fake_bss_description!(Owe);
        let security = SecurityAuthenticator::Owe;
        let protection = Protection::try_from(SecurityContext {
            security: &security,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        });
        let _ = protection.expect_err("created OWE auth config for incompatible device");
    }

    #[test]
    fn protection_from_wpa1_passphrase_with_open_bss() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = Default::default();
        let bss = fake_bss_description!(Open);
        let credentials =
            wpa::Wpa1Credentials::Passphrase("password".as_bytes().try_into().unwrap());
        let _ = Protection::try_from(SecurityContext {
            security: &credentials,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        })
        .expect_err("incorrectly accepted WPA1 passphrase credentials with open BSS");
    }

    #[test]
    fn protection_from_open_authenticator_with_wpa1_bss() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = Default::default();
        let bss = fake_bss_description!(Wpa1);
        // Note that there is no credentials type associated with an open authenticator, so an
        // authenticator is used here instead.
        let authenticator = SecurityAuthenticator::Open;
        let _ = Protection::try_from(SecurityContext {
            security: &authenticator,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        })
        .expect_err("incorrectly accepted open authenticator with WPA1 BSS");
    }

    #[test]
    fn protection_from_wpa2_personal_passphrase_with_wpa3_bss() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = ClientConfig { wpa3_supported: true, ..Default::default() };
        let bss = fake_bss_description!(Wpa3);
        let credentials =
            wpa::Wpa2PersonalCredentials::Passphrase("password".as_bytes().try_into().unwrap());
        let _ = Protection::try_from(SecurityContext {
            security: &credentials,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        })
        .expect_err("incorrectly accepted WPA2 passphrase credentials with WPA3 BSS");
    }

    #[test]
    fn wpa1_psk_rsna() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = Default::default();
        let bss = fake_bss_description!(Wpa1);
        let credentials = wpa::Wpa1Credentials::Psk([1u8; PSK_SIZE_BYTES].into());
        let context = SecurityContext {
            security: &credentials,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        };
        assert!(context.authenticator_supplicant_ie().is_ok());
        assert!(matches!(context.authentication_config(), auth::Config::ComputedPsk(_)));

        let protection = Protection::try_from(context).unwrap();
        assert_matches!(protection, Protection::LegacyWpa(rsna) => {
            assert_eq!(rsna.supplicant.get_auth_method(), auth::MethodName::Psk);
        });
    }

    #[test]
    fn wpa2_personal_psk_rsna() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = Default::default();
        let bss = fake_bss_description!(Wpa2);
        let credentials = wpa::Wpa2PersonalCredentials::Psk([1u8; PSK_SIZE_BYTES].into());
        let context = SecurityContext {
            security: &credentials,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        };
        assert!(context.authenticator_supplicant_rsne().is_ok());
        assert!(matches!(context.authentication_config(), auth::Config::ComputedPsk(_)));

        let protection = Protection::try_from(context).unwrap();
        assert_matches!(protection, Protection::Rsna(rsna) => {
            assert_eq!(rsna.supplicant.get_auth_method(), auth::MethodName::Psk);
        });
    }

    #[test]
    fn wpa3_personal_passphrase_rsna_sme_auth() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = ClientConfig { wpa3_supported: true, ..Default::default() };
        let bss = fake_bss_description!(Wpa3);
        let credentials =
            wpa::Wpa3PersonalCredentials::Passphrase("password".as_bytes().try_into().unwrap());
        let context = SecurityContext {
            security: &credentials,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        };
        assert!(context.authenticator_supplicant_rsne().is_ok());
        assert!(matches!(context.authentication_config(), Ok(auth::Config::Sae { .. })));

        let protection = Protection::try_from(context).unwrap();
        assert_matches!(protection, Protection::Rsna(rsna) => {
            assert_eq!(rsna.supplicant.get_auth_method(), auth::MethodName::Sae);
        });
    }

    #[test_case(false, fake_bss_description!(Wpa3), PweMethod::Loop; "looping")]
    #[test_case(true, fake_bss_description!(Wpa3), PweMethod::Loop; "only driver supports direct")]
    #[test_case(false, fake_bss_description!(Wpa3, sae_hash_to_element: true), PweMethod::Loop; "only peer supports direct")]
    #[test_case(true, fake_bss_description!(Wpa3, sae_hash_to_element: true), PweMethod::Direct; "direct")]
    fn wpa3_personal_passphrase_rsna_sme_auth_hash_to_element(
        driver_supports_h2e: bool,
        bss: BssDescription,
        expected_pwe_method: PweMethod,
    ) {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let mut security_support = fake_security_support();
        if driver_supports_h2e {
            security_support.sae.as_mut().unwrap().hash_to_element_supported = Some(true);
        }
        let config = ClientConfig { wpa3_supported: true, ..Default::default() };
        let credentials =
            wpa::Wpa3PersonalCredentials::Passphrase("password".as_bytes().try_into().unwrap());
        let context = SecurityContext {
            security: &credentials,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        };
        assert!(context.authenticator_supplicant_rsne().is_ok());
        let pwe_method = assert_matches!(
            context.authentication_config(),
            Ok(auth::Config::Sae { pwe_method, .. }) => pwe_method
        );
        assert_eq!(pwe_method, expected_pwe_method);

        let protection = Protection::try_from(context).unwrap();
        assert_matches!(protection, Protection::Rsna(rsna) => {
            assert_eq!(rsna.supplicant.get_auth_method(), auth::MethodName::Sae);
        });
    }

    #[test]
    fn wpa3_personal_passphrase_rsna_driver_auth() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let mut security_support = fake_security_support_empty();
        security_support.mfp.get_or_insert_with(Default::default).supported = Some(true);
        security_support.sae.get_or_insert_with(Default::default).driver_handler_supported =
            Some(true);
        let config = ClientConfig { wpa3_supported: true, ..Default::default() };
        let bss = fake_bss_description!(Wpa3);
        let credentials =
            wpa::Wpa3PersonalCredentials::Passphrase("password".as_bytes().try_into().unwrap());
        let context = SecurityContext {
            security: &credentials,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        };
        assert!(context.authenticator_supplicant_rsne().is_ok());
        assert_matches!(context.authentication_config(), Ok(auth::Config::DriverSae { .. }));

        let protection = Protection::try_from(context).unwrap();
        assert_matches!(protection, Protection::Rsna(rsna) => {
            assert_eq!(rsna.supplicant.get_auth_method(), auth::MethodName::Sae);
        });
    }

    #[test]
    fn wpa3_personal_passphrase_prefer_sme_auth() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let mut security_support = fake_security_support_empty();
        security_support.mfp.get_or_insert_with(Default::default).supported = Some(true);
        security_support.sae.get_or_insert_with(Default::default).driver_handler_supported =
            Some(true);
        security_support.sae.get_or_insert_with(Default::default).sme_handler_supported =
            Some(true);
        let config = ClientConfig { wpa3_supported: true, ..Default::default() };
        let bss = fake_bss_description!(Wpa3);
        let credentials =
            wpa::Wpa3PersonalCredentials::Passphrase("password".as_bytes().try_into().unwrap());
        let context = SecurityContext {
            security: &credentials,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        };
        assert!(matches!(context.authentication_config(), Ok(auth::Config::Sae { .. })));
    }

    #[test]
    fn wpa3_personal_passphrase_no_security_support_features() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support_empty();
        let config = Default::default();
        let bss = fake_bss_description!(Wpa3);
        let credentials =
            wpa::Wpa3PersonalCredentials::Passphrase("password".as_bytes().try_into().unwrap());
        let context = SecurityContext {
            security: &credentials,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        };
        let _ = context
            .authentication_config()
            .expect_err("created WPA3 auth config for incompatible device");
    }

    #[test]
    fn owe_rsna_sme_auth() {
        let device = crate::test_utils::fake_device_info([1u8; 6].into());
        let security_support = fake_security_support();
        let config = ClientConfig { owe_supported: true, ..Default::default() };
        let bss = fake_bss_description!(Owe);
        let security = OweAuthenticator;
        let context = SecurityContext {
            security: &security,
            device: &device,
            security_support: &security_support,
            config: &config,
            bss: &bss,
        };
        assert!(context.authenticator_supplicant_rsne().is_ok());
        assert!(matches!(context.authentication_config(), Ok(auth::Config::Owe)));

        let protection = Protection::try_from(context).unwrap();
        assert_matches!(protection, Protection::Rsna(rsna) => {
            assert_eq!(rsna.supplicant.get_auth_method(), auth::MethodName::Owe);
        });
    }
}
