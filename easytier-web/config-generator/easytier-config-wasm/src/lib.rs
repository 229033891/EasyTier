//! WASM `cdylib` surface for the config-generator UI.
//!
//! Kept as a thin wrapper so `easytier-core` remains `rlib`-only (avoids the
//! musl "dropping unsupported crate type `cdylib`" warning on native builds).
//!
//! Exports are `wasm32-unknown-unknown` only so `cargo clippy --all` on host
//! targets does not require `easytier_core::config::browser` (that module is
//! itself gated to browser WASM).

#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

use wasm_bindgen::prelude::*;

fn js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

#[wasm_bindgen]
pub fn generate_config(config_json: &str) -> Result<String, JsValue> {
    easytier_core::config::browser::generate_config(config_json).map_err(js_error)
}

#[wasm_bindgen]
pub fn merge_config(original_toml: &str, config_json: &str) -> Result<String, JsValue> {
    easytier_core::config::browser::merge_config(original_toml, config_json).map_err(js_error)
}

#[wasm_bindgen]
pub fn parse_config(toml_config: &str) -> Result<String, JsValue> {
    easytier_core::config::browser::parse_config(toml_config).map_err(js_error)
}
