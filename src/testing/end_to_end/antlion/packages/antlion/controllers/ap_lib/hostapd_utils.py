# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.


from antlion import utils
from antlion.controllers.ap_lib import hostapd_constants
from antlion.controllers.ap_lib.hostapd_security import Security, SecurityMode


def generate_random_password(
    security_mode: SecurityMode = SecurityMode.OPEN,
    length: int | None = None,
    hex: int | None = None,
) -> str:
    """Generates a random password. Defaults to an 8 character ASCII password.

    Args:
        security_mode: Used to determine if length should be WEP compatible
            (useful for generated tests to simply pass in security mode)
        length: Length of password to generate. Defaults to 8, unless
            security_mode is WEP, then 13
        hex: If True, generates a hex string, else ascii
    """
    if hex:
        generator_func = utils.rand_hex_str
    else:
        generator_func = utils.rand_ascii_str

    if length:
        return generator_func(length)
    if security_mode is SecurityMode.WEP:
        return generator_func(hostapd_constants.WEP_DEFAULT_STR_LENGTH)
    else:
        return generator_func(hostapd_constants.MIN_WPA_PSK_LENGTH)


def verify_interface(interface: str, valid_interfaces: list[str]) -> None:
    """Raises error if interface is missing or invalid

    Args:
        interface: interface name
        valid_interfaces: valid interface names
    """
    if interface not in valid_interfaces:
        raise ValueError(f"Invalid interface name was passed: {interface}")


def verify_security_mode(
    security_profile: Security, valid_security_modes: list[SecurityMode]
) -> None:
    """Raises error if security mode is not in list of valid security modes.

    Args:
        security_profile: Security to verify
        valid_security_modes: Valid security modes for a profile.
    """
    if security_profile.security_mode not in valid_security_modes:
        raise ValueError(
            f"Invalid Security Mode: {security_profile.security_mode}; "
            f"Valid Security Modes for this profile: {valid_security_modes}"
        )


def verify_cipher(security_profile: Security, valid_ciphers: list[str]) -> None:
    """Raise error if cipher is not in list of valid ciphers.

    Args:
        security_profile: Security profile to verify
        valid_ciphers: A list of valid ciphers for security_profile.
    """
    if security_profile.security_mode is SecurityMode.OPEN:
        raise ValueError("Security mode is open.")
    elif security_profile.security_mode is SecurityMode.WPA:
        if security_profile.wpa_cipher not in valid_ciphers:
            raise ValueError(
                f"Invalid WPA Cipher: {security_profile.wpa_cipher}. "
                f"Valid WPA Ciphers for this profile: {valid_ciphers}"
            )
    elif security_profile.security_mode is SecurityMode.WPA2:
        if security_profile.wpa2_cipher not in valid_ciphers:
            raise ValueError(
                f"Invalid WPA2 Cipher: {security_profile.wpa2_cipher}. "
                f"Valid WPA2 Ciphers for this profile: {valid_ciphers}"
            )
    else:
        raise ValueError(
            f"Invalid Security Mode: {security_profile.security_mode}"
        )
