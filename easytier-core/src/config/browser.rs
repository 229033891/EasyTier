//! Browser-facing config helpers used by `easytier-config-wasm`.
//!
//! Kept free of `wasm_bindgen` so `easytier-core` stays `rlib`-only (musl
//! targets cannot produce `cdylib` under the default static CRT).

use super::{
    api_input::{NetworkConfig, NetworkConfigExt, merge_network_config_toml},
    toml::{ConfigLoader, TomlConfig},
};

fn map_err(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}

pub fn generate_config(config_json: &str) -> Result<String, String> {
    let config: NetworkConfig = serde_json::from_str(config_json).map_err(map_err)?;
    config
        .gen_config()
        .map(|config| config.dump())
        .map_err(map_err)
}

pub fn merge_config(original_toml: &str, config_json: &str) -> Result<String, String> {
    let config: NetworkConfig = serde_json::from_str(config_json).map_err(map_err)?;
    merge_network_config_toml(original_toml, &config).map_err(map_err)
}

pub fn parse_config(toml_config: &str) -> Result<String, String> {
    let config = TomlConfig::new_from_str(toml_config)
        .and_then(|config| NetworkConfig::new_from_config(&config))
        .map_err(map_err)?;
    serde_json::to_string(&config).map_err(map_err)
}
