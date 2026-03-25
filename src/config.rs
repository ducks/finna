use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_debate_providers")]
    pub default_debate_providers: Vec<String>,

    #[serde(default = "default_spec_provider")]
    pub default_spec_provider: String,

    #[serde(default = "default_implement_providers")]
    pub default_implement_providers: Vec<String>,

    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(rename = "type")]
    pub provider_type: String,
    pub command: String,
    pub args: Vec<String>,
}

fn default_debate_providers() -> Vec<String> {
    vec!["claude".to_string(), "codex".to_string()]
}

fn default_spec_provider() -> String {
    "claude".to_string()
}

fn default_implement_providers() -> Vec<String> {
    vec!["claude".to_string(), "codex".to_string()]
}

impl Default for Config {
    fn default() -> Self {
        let mut providers = HashMap::new();

        // Claude provider
        providers.insert(
            "claude".to_string(),
            ProviderConfig {
                provider_type: "claude".to_string(),
                command: "claude".to_string(),
                args: vec!["-p".to_string(), "{prompt}".to_string()],
            },
        );

        // Codex provider
        providers.insert(
            "codex".to_string(),
            ProviderConfig {
                provider_type: "codex".to_string(),
                command: "codex".to_string(),
                args: vec![
                    "exec".to_string(),
                    "--json".to_string(),
                    "-s".to_string(),
                    "read-only".to_string(),
                    "{prompt}".to_string(),
                ],
            },
        );

        // Gemini provider (optional, commented out in default config)
        providers.insert(
            "gemini".to_string(),
            ProviderConfig {
                provider_type: "gemini".to_string(),
                command: "npx".to_string(),
                args: vec!["@google/gemini-cli".to_string(), "{prompt}".to_string()],
            },
        );

        Self {
            default_debate_providers: default_debate_providers(),
            default_spec_provider: default_spec_provider(),
            default_implement_providers: default_implement_providers(),
            providers,
        }
    }
}

impl Config {
    pub fn get_config_path() -> Result<PathBuf> {
        let mut path = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?;
        path.push("finna");
        path.push("config.toml");
        Ok(path)
    }

    pub fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;

        if !config_path.exists() {
            let config = Config::default();
            config.save()?;
            return Ok(config);
        }

        let contents = fs::read_to_string(&config_path)
            .context("Failed to read config file")?;

        let config: Config = toml::from_str(&contents)
            .context("Failed to parse config file")?;

        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let config_path = Self::get_config_path()?;

        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)
                .context("Failed to create config directory")?;
        }

        let contents = toml::to_string_pretty(self)
            .context("Failed to serialize config")?;

        fs::write(&config_path, contents)
            .context("Failed to write config file")?;

        // Set restrictive permissions on Unix systems
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&config_path, perms)?;
        }

        Ok(())
    }

    pub fn get_provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.default_debate_providers, vec!["claude", "codex"]);
        assert_eq!(config.default_spec_provider, "claude");
        assert_eq!(config.default_implement_providers, vec!["claude", "codex"]);
        assert!(config.providers.contains_key("claude"));
        assert!(config.providers.contains_key("codex"));
        assert!(config.providers.contains_key("gemini"));
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(toml_str.contains("[providers.claude]"));
        assert!(toml_str.contains("[providers.codex]"));
        assert!(toml_str.contains("default_debate_providers"));
    }
}
