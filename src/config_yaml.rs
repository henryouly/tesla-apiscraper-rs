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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.yml".to_string());
    let tmp_path = parent.join(format!(".{file_name}.tmp"));

    let write_result = (|| -> Result<()> {
        let file = std::fs::File::create(&tmp_path)
            .with_context(|| format!("failed to create {}", tmp_path.display()))?;
        serde_yaml::to_writer(&file, value)
            .with_context(|| format!("failed to write {}", tmp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync {}", tmp_path.display()))?;
        Ok(())
    })();
    if let Err(err) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }

    std::fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to atomically replace {} with {}",
            path.display(),
            tmp_path.display()
        )
    })?;
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

    #[test]
    fn empty_file_uses_defaults() {
        let dir = temp_dir();
        let path = dir.join("settings.yml");
        fs::write(&path, "").unwrap();
        let result: Result<SettingsConfig> = load_or_default(&path);
        assert!(result.is_ok(), "empty YAML should deserialize as defaults");
        let config = result.unwrap();
        assert_eq!(config.global.unit_length, "km");
        assert!(config.cars.is_empty());
    }

    #[test]
    fn save_settings_roundtrip() {
        let dir = temp_dir();
        let mut mgr = YamlConfigManager::load(&dir).unwrap();
        mgr.settings.global.unit_length = "mi".into();
        mgr.settings.global.unit_temperature = "F".into();
        mgr.settings.global.theme = "dark".into();
        mgr.settings.cars.insert(
            "5YJSA1".into(),
            CarSettings {
                enabled: false,
                suspend_after_idle_minutes: 30,
                require_unlocked_for_wake: true,
                ..Default::default()
            },
        );
        mgr.save_settings().unwrap();

        let mgr2 = YamlConfigManager::load(&dir).unwrap();
        assert_eq!(mgr2.settings.global.unit_length, "mi");
        assert_eq!(mgr2.settings.global.unit_temperature, "F");
        assert_eq!(mgr2.settings.global.theme, "dark");
        assert!(!mgr2.settings.cars["5YJSA1"].enabled);
        assert_eq!(mgr2.settings.cars["5YJSA1"].suspend_after_idle_minutes, 30);
        assert!(mgr2.settings.cars["5YJSA1"].require_unlocked_for_wake);
    }

    #[test]
    fn geofences_with_negative_coordinates() {
        let dir = temp_dir();
        let mut mgr = YamlConfigManager::load(&dir).unwrap();
        mgr.geofences.geofences.push(Geofence {
            name: "Sydney".into(),
            latitude: -33.8688,
            longitude: 151.2093,
            radius_meters: 500.0,
            billing: None,
        });
        mgr.geofences.geofences.push(Geofence {
            name: "South Pole".into(),
            latitude: -82.8628,
            longitude: -135.0,
            radius_meters: 1000.0,
            billing: None,
        });
        mgr.save_geofences().unwrap();

        let mgr2 = YamlConfigManager::load(&dir).unwrap();
        assert!((mgr2.geofences.geofences[0].latitude - (-33.8688)).abs() < f64::EPSILON);
        assert!((mgr2.geofences.geofences[1].latitude - (-82.8628)).abs() < f64::EPSILON);
    }

    #[test]
    fn geofences_with_unicode_names() {
        let dir = temp_dir();
        let mut mgr = YamlConfigManager::load(&dir).unwrap();
        mgr.geofences.geofences.push(Geofence {
            name: "東京".into(),
            latitude: 35.6762,
            longitude: 139.6503,
            radius_meters: 100.0,
            billing: None,
        });
        mgr.geofences.geofences.push(Geofence {
            name: "北京".into(),
            latitude: 39.9042,
            longitude: 116.4074,
            radius_meters: 100.0,
            billing: None,
        });
        mgr.save_geofences().unwrap();

        let mgr2 = YamlConfigManager::load(&dir).unwrap();
        assert_eq!(mgr2.geofences.geofences[0].name, "東京");
        assert_eq!(mgr2.geofences.geofences[1].name, "北京");
    }

    #[test]
    fn car_settings_default_values() {
        let default = CarSettings::default();
        assert_eq!(default.suspend_after_idle_minutes, 21);
        assert_eq!(default.suspend_minimum_minutes, 15);
        assert!(!default.require_unlocked_for_wake);
        assert!(!default.free_supercharging);
        assert!(!default.use_streaming_api);
        assert!(default.enabled);
        assert!(!default.lfp_battery);
    }

    #[test]
    fn billing_minimal_fields() {
        let dir = temp_dir();
        let path = dir.join("geofences.yml");
        // Only type set; cost_per_unit and session_fee use defaults (0)
        let input = GeofencesConfig {
            geofences: vec![Geofence {
                name: "Free Charger".into(),
                latitude: 37.0,
                longitude: -122.0,
                radius_meters: default_radius(),
                billing: Some(BillingConfig {
                    billing_type: BillingType::PerMinute,
                    cost_per_unit: 0.0,
                    session_fee: 0.0,
                }),
            }],
        };
        save_yaml(&path, &input).unwrap();
        let loaded: GeofencesConfig = load_or_default(&path).unwrap();
        assert_eq!(
            loaded.geofences[0].billing.as_ref().unwrap().billing_type,
            BillingType::PerMinute
        );
        assert_eq!(loaded.geofences[0].radius_meters, default_radius());
    }

    #[test]
    fn global_settings_default_values() {
        let default = GlobalSettings::default();
        assert_eq!(default.unit_length, "km");
        assert_eq!(default.unit_temperature, "C");
        assert_eq!(default.unit_pressure, "bar");
        assert_eq!(default.preferred_range, "rated");
        assert_eq!(default.language, "en");
        assert_eq!(default.theme, "system");
    }

    #[test]
    fn geofences_empty_yaml_list() {
        let dir = temp_dir();
        let path = dir.join("geofences.yml");
        fs::write(&path, "geofences: []\n").unwrap();
        let loaded: GeofencesConfig = load_or_default(&path).unwrap();
        assert!(loaded.geofences.is_empty());
    }
}
