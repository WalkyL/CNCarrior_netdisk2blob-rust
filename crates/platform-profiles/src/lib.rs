// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

#[cfg(not(any(
    feature = "full-host",
    feature = "lite-host",
    feature = "esp-client",
    feature = "esp-relay"
)))]
compile_error!(
    "select exactly one platform profile: full-host, lite-host, esp-client, or esp-relay"
);

#[cfg(any(
    all(feature = "full-host", feature = "lite-host"),
    all(feature = "full-host", feature = "esp-client"),
    all(feature = "full-host", feature = "esp-relay"),
    all(feature = "lite-host", feature = "esp-client"),
    all(feature = "lite-host", feature = "esp-relay"),
    all(feature = "esp-client", feature = "esp-relay")
))]
compile_error!("platform profiles are mutually exclusive");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformProfile {
    FullHost,
    LiteHost,
    EspClient,
    EspRelay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformProfileSpec {
    pub profile: PlatformProfile,
    pub name: &'static str,
    pub role: &'static str,
    pub runs_gatewayd: bool,
    pub allows_sqlite: bool,
    pub allows_upstream_http_client: bool,
    pub allows_admin_web: bool,
    pub allows_onedrive: bool,
    pub max_concurrency_hint: u8,
}

#[cfg(feature = "full-host")]
pub const ACTIVE_PROFILE: PlatformProfileSpec = PlatformProfileSpec {
    profile: PlatformProfile::FullHost,
    name: "full-host",
    role: "complete linux host",
    runs_gatewayd: true,
    allows_sqlite: true,
    allows_upstream_http_client: true,
    allows_admin_web: true,
    allows_onedrive: true,
    max_concurrency_hint: 8,
};

#[cfg(feature = "lite-host")]
pub const ACTIVE_PROFILE: PlatformProfileSpec = PlatformProfileSpec {
    profile: PlatformProfile::LiteHost,
    name: "lite-host",
    role: "openwrt arm64 host",
    runs_gatewayd: true,
    allows_sqlite: true,
    allows_upstream_http_client: true,
    allows_admin_web: false,
    allows_onedrive: false,
    max_concurrency_hint: 2,
};

#[cfg(feature = "esp-client")]
pub const ACTIVE_PROFILE: PlatformProfileSpec = PlatformProfileSpec {
    profile: PlatformProfile::EspClient,
    name: "esp-client",
    role: "s3/mcp client only",
    runs_gatewayd: false,
    allows_sqlite: false,
    allows_upstream_http_client: false,
    allows_admin_web: false,
    allows_onedrive: false,
    max_concurrency_hint: 1,
};

#[cfg(feature = "esp-relay")]
pub const ACTIVE_PROFILE: PlatformProfileSpec = PlatformProfileSpec {
    profile: PlatformProfile::EspRelay,
    name: "esp-relay",
    role: "single-provider relay feasibility profile",
    runs_gatewayd: false,
    allows_sqlite: false,
    allows_upstream_http_client: false,
    allows_admin_web: false,
    allows_onedrive: false,
    max_concurrency_hint: 1,
};

pub fn active_profile() -> PlatformProfileSpec {
    ACTIVE_PROFILE
}

#[cfg(test)]
mod tests {
    use super::active_profile;

    #[test]
    fn active_profile_has_stable_name() {
        let profile = active_profile();
        assert!(!profile.name.is_empty());
        assert!(profile.max_concurrency_hint >= 1);
    }

    #[test]
    fn esp_profiles_do_not_claim_host_capabilities() {
        let profile = active_profile();
        if matches!(profile.name, "esp-client" | "esp-relay") {
            assert!(!profile.runs_gatewayd);
            assert!(!profile.allows_sqlite);
            assert!(!profile.allows_admin_web);
            assert!(!profile.allows_onedrive);
        }
    }
}
