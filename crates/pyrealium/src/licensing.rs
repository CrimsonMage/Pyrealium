// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Vanquish
//! Public license/source notice for the server and its remote users.
pub const SOURCE_URL: &str = env!("PYREALIUM_COMPILED_SOURCE_URL");
pub const LICENSE_TEXT: &str = include_str!("../../../LICENSE");
static PUBLIC_ORIGIN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
/// The server's public account origin (`accounts.public_origin`); the source offer is served there.
pub fn set_public_origin(origin: String) {
    let _ = PUBLIC_ORIGIN.set(origin);
}
pub fn source_url() -> String {
    PUBLIC_ORIGIN
        .get()
        .cloned()
        .or_else(|| std::env::var("PYREALIUM_PUBLIC_ORIGIN").ok())
        .filter(|url| {
            (url.starts_with("https://") || url.starts_with("http://127.0.0.1:"))
                && url.len() < 240
                && url.bytes().all(|b| b.is_ascii_graphic())
                && !url.contains(['@', '?', '#'])
        })
        .map(|url| format!("{}/source.zip", url.trim_end_matches('/')))
        .unwrap_or_else(|| "http://127.0.0.1:19400/source.zip".into())
}

pub fn notice() -> String {
    format!(
        "Pyrealium, continuing AC2E. Copyright (C) 2026 Vanquish and contributors. AGPL-3.0-only; no warranty. Corresponding source and license: {}",
        source_url()
    )
}
