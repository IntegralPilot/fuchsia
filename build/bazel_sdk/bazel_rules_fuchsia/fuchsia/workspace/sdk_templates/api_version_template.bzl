"""
Generated internal values related to Fuchsia Platform Versioning and API levels.

Do not use these values directly as the structure may change over time.
Instead, use the supported methods to access API levels.
The `*KNOWN*` values should not be used outside constraint definitions.
"""

# All API levels supported by this SDK release for building new artifacts.
# Includes all Supported API levels as well as "NEXT" and "HEAD".
INTERNAL_ONLY_SUPPORTED_API_LEVELS = [{{supported_api_levels}}]

# API levels known to the SDK but not supported by this release. These are
# Sunset and Retired API levels.
INTERNAL_ONLY_KNOWN_UNSUPPORTED_API_LEVELS = [{{unsupported_api_levels}}]

# All API levels known to the SDK, both supported and not.
INTERNAL_ONLY_ALL_KNOWN_API_LEVELS = (
    INTERNAL_ONLY_SUPPORTED_API_LEVELS + INTERNAL_ONLY_KNOWN_UNSUPPORTED_API_LEVELS
)
