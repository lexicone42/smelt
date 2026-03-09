use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Project configuration from `smelt.toml`.
///
/// Defines environments, their layer chains, and per-environment defaults.
///
/// ```toml
/// [project]
/// name = "my-infra"
/// default_environment = "dev"
///
/// [environments.dev]
/// layers = ["base"]
/// region = "us-east-1"
///
/// [environments.staging]
/// layers = ["base", "staging"]
/// region = "us-east-1"
///
/// [environments.production]
/// layers = ["base", "production"]
/// region = "us-west-2"
/// protected = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub project: ProjectMeta,
    #[serde(default)]
    pub environments: BTreeMap<String, EnvironmentConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub name: String,
    #[serde(default = "default_env")]
    pub default_environment: String,
}

fn default_env() -> String {
    "default".to_string()
}

/// Configuration for a single environment.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EnvironmentConfig {
    /// Layer names to apply, in order (first = base, last = highest priority)
    #[serde(default)]
    pub layers: Vec<String>,

    /// Cloud region for this environment
    #[serde(default)]
    pub region: Option<String>,

    /// Cloud project/account for this environment
    #[serde(default)]
    pub project_id: Option<String>,

    /// Whether this environment requires `--yes` (no interactive prompts)
    #[serde(default)]
    pub protected: bool,

    /// Arbitrary per-environment variables accessible as `env.key` in .smelt files
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid smelt.toml: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("environment '{0}' not found in smelt.toml")]
    EnvironmentNotFound(String),
    #[error("environment '{0}' already exists")]
    EnvironmentExists(String),
    #[error("cannot delete the default environment '{0}'")]
    CannotDeleteDefault(String),
    #[error("no smelt.toml found — run `smelt init` to create one")]
    NotFound,
}

impl ProjectConfig {
    /// Load project config from `smelt.toml` in the given directory.
    ///
    /// Validates that `default_environment` refers to a defined environment.
    pub fn load(project_root: &Path) -> Result<Self, ConfigError> {
        let path = config_path(project_root);
        if !path.exists() {
            return Err(ConfigError::NotFound);
        }
        let content = fs::read_to_string(&path)?;
        let config: Self = toml::from_str(&content)?;

        // Validate that default_environment exists
        if !config
            .environments
            .contains_key(&config.project.default_environment)
        {
            return Err(ConfigError::EnvironmentNotFound(format!(
                "default_environment '{}' is not defined in [environments]",
                config.project.default_environment
            )));
        }

        Ok(config)
    }

    /// Load project config, or return a default if no smelt.toml exists.
    ///
    /// Only falls back to default on `NotFound`. Parse errors and I/O errors
    /// are propagated — silently swallowing them would hide broken configs.
    pub fn load_or_default(project_root: &Path) -> Result<Self, ConfigError> {
        match Self::load(project_root) {
            Ok(config) => Ok(config),
            Err(ConfigError::NotFound) => Ok(Self::default_config("smelt-project")),
            Err(e) => Err(e),
        }
    }

    /// Save project config to `smelt.toml`.
    pub fn save(&self, project_root: &Path) -> Result<(), ConfigError> {
        let path = config_path(project_root);
        let content = toml::to_string_pretty(self)?;
        fs::write(&path, content)?;
        Ok(())
    }

    /// Create a default config with a single "default" environment.
    pub fn default_config(name: &str) -> Self {
        let mut environments = BTreeMap::new();
        environments.insert("default".to_string(), EnvironmentConfig::default());
        Self {
            project: ProjectMeta {
                name: name.to_string(),
                default_environment: "default".to_string(),
            },
            environments,
        }
    }

    /// Get the config for an environment, falling back to defaults.
    pub fn get_env(&self, name: &str) -> Result<&EnvironmentConfig, ConfigError> {
        self.environments
            .get(name)
            .ok_or_else(|| ConfigError::EnvironmentNotFound(name.to_string()))
    }

    /// Add a new environment.
    pub fn add_env(&mut self, name: &str, config: EnvironmentConfig) -> Result<(), ConfigError> {
        if self.environments.contains_key(name) {
            return Err(ConfigError::EnvironmentExists(name.to_string()));
        }
        self.environments.insert(name.to_string(), config);
        Ok(())
    }

    /// Remove an environment.
    pub fn remove_env(&mut self, name: &str) -> Result<EnvironmentConfig, ConfigError> {
        if name == self.project.default_environment {
            return Err(ConfigError::CannotDeleteDefault(name.to_string()));
        }
        self.environments
            .remove(name)
            .ok_or_else(|| ConfigError::EnvironmentNotFound(name.to_string()))
    }

    /// List all environment names.
    pub fn env_names(&self) -> Vec<&str> {
        self.environments.keys().map(|s| s.as_str()).collect()
    }

    /// Resolve which layers apply to a given environment, in order.
    pub fn layers_for_env(&self, name: &str) -> Vec<String> {
        match self.environments.get(name) {
            Some(env) => env.layers.clone(),
            None => Vec::new(),
        }
    }
}

fn config_path(project_root: &Path) -> PathBuf {
    project_root.join("smelt.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("smelt-config-test-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_config_roundtrip() {
        let dir = temp_dir();
        let config = ProjectConfig::default_config("test-project");
        config.save(&dir).unwrap();

        let loaded = ProjectConfig::load(&dir).unwrap();
        assert_eq!(loaded.project.name, "test-project");
        assert_eq!(loaded.project.default_environment, "default");
        assert!(loaded.environments.contains_key("default"));
    }

    #[test]
    fn add_and_remove_environments() {
        let mut config = ProjectConfig::default_config("test");

        config
            .add_env(
                "staging",
                EnvironmentConfig {
                    layers: vec!["base".into(), "staging".into()],
                    region: Some("us-east-1".into()),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(config.env_names().len(), 2);
        assert!(config.get_env("staging").is_ok());
        assert_eq!(
            config.layers_for_env("staging"),
            vec!["base".to_string(), "staging".to_string()]
        );

        // Can't add duplicate
        assert!(
            config
                .add_env("staging", EnvironmentConfig::default())
                .is_err()
        );

        // Can't delete default
        assert!(config.remove_env("default").is_err());

        // Can delete staging
        config.remove_env("staging").unwrap();
        assert_eq!(config.env_names().len(), 1);
    }

    #[test]
    fn parse_toml() {
        let toml = r#"
[project]
name = "my-infra"
default_environment = "dev"

[environments.dev]
layers = ["base"]
region = "us-east-1"

[environments.staging]
layers = ["base", "staging"]
region = "us-east-1"

[environments.production]
layers = ["base", "production"]
region = "us-west-2"
protected = true
vars = { account_id = "123456789" }
"#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.project.name, "my-infra");
        assert_eq!(config.environments.len(), 3);
        assert!(config.environments["production"].protected);
        assert_eq!(
            config.environments["production"].vars["account_id"],
            "123456789"
        );
    }

    #[test]
    fn load_or_default_when_missing() {
        let dir = temp_dir();
        let config = ProjectConfig::load_or_default(&dir).unwrap();
        assert_eq!(config.project.name, "smelt-project");
    }
}
