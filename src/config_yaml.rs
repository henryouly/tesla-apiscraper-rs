#![allow(dead_code)]

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Geofences
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct GeofencesConfig {
    #[serde(default)]
    pub geofences: Vec<Geofence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Geofence {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default = "default_radius")]
    pub radius_meters: f64,
    #[serde(default)]
    pub billing: Option<BillingConfig>,
}

const fn default_radius() -> f64 {
    100.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingConfig {
    #[serde(rename = "type")]
    pub billing_type: BillingType,
    #[serde(default)]
    pub cost_per_unit: f64,
    #[serde(default)]
    pub session_fee: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BillingType {
    #[serde(rename = "per_kwh")]
    PerKwh,
    #[serde(rename = "per_minute")]
    PerMinute,
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalSettings {
    #[serde(default = "default_unit_length")]
    pub unit_length: String,
    #[serde(default = "default_unit_temperature")]
    pub unit_temperature: String,
    #[serde(default = "default_unit_pressure")]
    pub unit_pressure: String,
    #[serde(default = "default_preferred_range")]
    pub preferred_range: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_theme")]
    pub theme: String,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            unit_length: default_unit_length(),
            unit_temperature: default_unit_temperature(),
            unit_pressure: default_unit_pressure(),
            preferred_range: default_preferred_range(),
            language: default_language(),
            theme: default_theme(),
        }
    }
}

fn default_unit_length() -> String {
    "km".into()
}
fn default_unit_temperature() -> String {
    "C".into()
}
fn default_unit_pressure() -> String {
    "bar".into()
}
fn default_preferred_range() -> String {
    "rated".into()
}
fn default_language() -> String {
    "en".into()
}
fn default_theme() -> String {
    "system".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CarSettings {
    #[serde(default = "default_suspend_idle")]
    pub suspend_after_idle_minutes: u64,
    #[serde(default = "default_suspend_min")]
    pub suspend_minimum_minutes: u64,
    #[serde(default)]
    pub require_unlocked_for_wake: bool,
    #[serde(default)]
    pub free_supercharging: bool,
    #[serde(default)]
    pub use_streaming_api: bool,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub lfp_battery: bool,
}

impl Default for CarSettings {
    fn default() -> Self {
        Self {
            suspend_after_idle_minutes: default_suspend_idle(),
            suspend_minimum_minutes: default_suspend_min(),
            require_unlocked_for_wake: false,
            free_supercharging: false,
            use_streaming_api: false,
            enabled: default_enabled(),
            lfp_battery: false,
        }
    }
}

const fn default_suspend_idle() -> u64 {
    21
}
const fn default_suspend_min() -> u64 {
    15
}
const fn default_enabled() -> bool {
    true
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SettingsConfig {
    #[serde(default)]
    pub global: GlobalSettings,
    #[serde(default)]
    pub cars: HashMap<String, CarSettings>,
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokensConfig {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
}

// ---------------------------------------------------------------------------
// YamlConfigManager
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct YamlConfigManager {
    config_dir: PathBuf,
    pub geofences: GeofencesConfig,
    pub settings: SettingsConfig,
    pub tokens: Option<TokensConfig>,
}

impl YamlConfigManager {
    pub fn load(config_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(config_dir).context("failed to create config directory")?;

        let geofences: GeofencesConfig = load_or_default(&config_dir.join("geofences.yml"))?;
        let settings: SettingsConfig = load_or_default(&config_dir.join("settings.yml"))?;
        let tokens: Option<TokensConfig> = load_optional(&config_dir.join("tokens.yml"))?;

        Ok(Self {
            config_dir: config_dir.to_path_buf(),
            geofences,
            settings,
            tokens,
        })
    }

    pub fn save_geofences(&self) -> Result<()> {
        save_yaml(&self.config_dir.join("geofences.yml"), &self.geofences)
            .context("failed to save geofences.yml")
    }

    pub fn save_settings(&self) -> Result<()> {
        save_yaml(&self.config_dir.join("settings.yml"), &self.settings)
            .context("failed to save settings.yml")
    }

    pub fn save_tokens(&self, tokens: &TokensConfig) -> Result<()> {
        save_yaml(&self.config_dir.join("tokens.yml"), tokens).context("failed to save tokens.yml")
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Load a YAML file, or create it with [`Default::default()`] if missing.
fn load_or_default<T>(path: &Path) -> Result<T>
where
    T: serde::de::DeserializeOwned + Serialize + Default,
{
    if path.exists() {
        let file = std::fs::File::open(path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        Ok(serde_yaml::from_reader(file)
            .with_context(|| format!("failed to parse {}", path.display()))?)
    } else {
        let value = T::default();
        let file = std::fs::File::create(path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        serde_yaml::to_writer(file, &value)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(value)
    }
}

/// Load a YAML file that may not exist. Returns `None` if the file is absent.
fn load_optional<T>(path: &Path) -> Result<Option<T>>
where
    T: serde::de::DeserializeOwned,
{
    if path.exists() {
        let file = std::fs::File::open(path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        Ok(Some(serde_yaml::from_reader(file).with_context(|| {
            format!("failed to parse {}", path.display())
        })?))
    } else {
        Ok(None)
    }
}

/// Serialize `value` to YAML and write it to `path`.
fn save_yaml<T>(path: &Path, value: &T) -> Result<()>
where
    T: serde::Serialize,
{
    let file = std::fs::File::create(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    serde_yaml::to_writer(file, value)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir()
            .join("tesla-apiscraper-test")
            .join(ts.to_string());
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_geofences_creates_default() {
        let dir = temp_dir();
        let mgr = YamlConfigManager::load(&dir).unwrap();
        assert!(mgr.geofences.geofences.is_empty());
        assert!(dir.join("geofences.yml").exists());
    }

    #[test]
    fn load_geofences_roundtrip() {
        let dir = temp_dir();
        let path = dir.join("geofences.yml");

        let input = GeofencesConfig {
            geofences: vec![Geofence {
                name: "Home".into(),
                latitude: 37.7749,
                longitude: -122.4194,
                radius_meters: 150.0,
                billing: Some(BillingConfig {
                    billing_type: BillingType::PerKwh,
                    cost_per_unit: 0.12,
                    session_fee: 0.50,
                }),
            }],
        };
        save_yaml(&path, &input).unwrap();
        let loaded: GeofencesConfig = load_or_default(&path).unwrap();
        assert_eq!(loaded.geofences.len(), 1);
        assert_eq!(loaded.geofences[0].name, "Home");
        assert_eq!(
            loaded.geofences[0].billing.as_ref().unwrap().cost_per_unit,
            0.12
        );
    }

    #[test]
    fn load_settings_creates_default() {
        let dir = temp_dir();
        let mgr = YamlConfigManager::load(&dir).unwrap();
        assert_eq!(mgr.settings.global.unit_length, "km");
        assert!(mgr.settings.cars.is_empty());
        assert!(dir.join("settings.yml").exists());
    }

    #[test]
    fn load_settings_with_car_overrides() {
        let dir = temp_dir();
        let path = dir.join("settings.yml");

        let input = SettingsConfig {
            global: GlobalSettings {
                unit_length: "mi".into(),
                ..Default::default()
            },
            cars: [(
                "5YJSA1".into(),
                CarSettings {
                    enabled: false,
                    ..Default::default()
                },
            )]
            .into(),
        };
        save_yaml(&path, &input).unwrap();
        let loaded: SettingsConfig = load_or_default(&path).unwrap();
        assert_eq!(loaded.global.unit_length, "mi");
        assert!(!loaded.cars["5YJSA1"].enabled);
    }

    #[test]
    fn tokens_returns_none_when_missing() {
        let dir = temp_dir();
        let mgr = YamlConfigManager::load(&dir).unwrap();
        assert!(mgr.tokens.is_none());
    }

    #[test]
    fn tokens_roundtrip() {
        let dir = temp_dir();
        let mgr = YamlConfigManager::load(&dir).unwrap();

        let tokens = TokensConfig {
            access_token: "encrypted-access".into(),
            refresh_token: "encrypted-refresh".into(),
            expires_at: 1_700_000_000,
        };
        mgr.save_tokens(&tokens).unwrap();

        let loaded: TokensConfig = load_optional(&dir.join("tokens.yml"))
            .unwrap()
            .expect("tokens.yml should exist");
        assert_eq!(loaded.access_token, "encrypted-access");
        assert_eq!(loaded.expires_at, 1_700_000_000);
    }

    #[test]
    fn save_and_reload_geofences() {
        let dir = temp_dir();
        let mut mgr = YamlConfigManager::load(&dir).unwrap();
        mgr.geofences.geofences.push(Geofence {
            name: "Work".into(),
            latitude: 37.7749,
            longitude: -122.4194,
            radius_meters: 200.0,
            billing: None,
        });
        mgr.save_geofences().unwrap();

        let mgr2 = YamlConfigManager::load(&dir).unwrap();
        assert_eq!(mgr2.geofences.geofences.len(), 1);
        assert_eq!(mgr2.geofences.geofences[0].name, "Work");
    }

    #[test]
    fn invalid_yaml_returns_error() {
        let dir = temp_dir();
        let path = dir.join("settings.yml");
        fs::write(&path, "invalid: [yaml: broken\n").unwrap();
        let result: Result<SettingsConfig> = load_or_default(&path);
        assert!(result.is_err());
    }
}
