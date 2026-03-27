// ABOUTME: Loads the tool completion registry from registry.toml.
// ABOUTME: Maps tool names to their shell-specific completion commands.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::sync::Error;

const EMBEDDED_REGISTRY: &str = include_str!("../registry.toml");
const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Raw registry format for deserialization (supports both string and object formats)
#[derive(Debug, Deserialize)]
struct RawToolCompletions {
    zsh: Option<ShellCommand>,
    bash: Option<ShellCommand>,
    fish: Option<ShellCommand>,
}

/// Parsed registry format with patterns and tools sections
#[derive(Debug, Deserialize)]
struct RawRegistry {
    schema_version: Option<u32>,
    #[serde(default)]
    patterns: HashMap<String, RawToolCompletions>,
    #[serde(default)]
    tools: HashMap<String, ToolEntry>,
}

/// A tool entry: either a pattern name or explicit shell commands
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ToolEntry {
    Pattern(String),
    Explicit(RawToolCompletions),
}

/// Shell command with optional environment variables
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ShellCommand {
    Simple(String),
    WithEnv {
        command: String,
        env: Option<std::collections::HashMap<String, String>>,
    },
}

impl ShellCommand {
    fn command(&self) -> &str {
        match self {
            ShellCommand::Simple(cmd) => cmd,
            ShellCommand::WithEnv { command, .. } => command,
        }
    }

    fn env(&self) -> Option<&std::collections::HashMap<String, String>> {
        match self {
            ShellCommand::Simple(_) => None,
            ShellCommand::WithEnv { env, .. } => env.as_ref(),
        }
    }
}

/// Expanded registry with all patterns resolved
#[derive(Debug)]
pub struct Registry {
    pub tools: HashMap<String, ToolCompletions>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolCompletions {
    pub zsh: Option<String>,
    pub bash: Option<String>,
    pub fish: Option<String>,
    pub zsh_env: Option<std::collections::HashMap<String, String>>,
    pub bash_env: Option<std::collections::HashMap<String, String>>,
    pub fish_env: Option<std::collections::HashMap<String, String>>,
}

impl ToolCompletions {
    pub fn get(&self, shell: &str) -> Option<&String> {
        match shell {
            "zsh" => self.zsh.as_ref(),
            "bash" => self.bash.as_ref(),
            "fish" => self.fish.as_ref(),
            _ => None,
        }
    }

    pub fn get_env(&self, shell: &str) -> Option<&std::collections::HashMap<String, String>> {
        match shell {
            "zsh" => self.zsh_env.as_ref(),
            "bash" => self.bash_env.as_ref(),
            "fish" => self.fish_env.as_ref(),
            _ => None,
        }
    }

    /// Expand pattern placeholders with tool name
    fn expand(&self, tool_name: &str) -> Self {
        Self {
            zsh: self.zsh.as_ref().map(|s| s.replace("{}", tool_name)),
            bash: self.bash.as_ref().map(|s| s.replace("{}", tool_name)),
            fish: self.fish.as_ref().map(|s| s.replace("{}", tool_name)),
            zsh_env: self.zsh_env.clone(),
            bash_env: self.bash_env.clone(),
            fish_env: self.fish_env.clone(),
        }
    }
}

impl ToolCompletions {
    /// Create from raw shell commands (for deserialization)
    fn from_raw(
        zsh: Option<ShellCommand>,
        bash: Option<ShellCommand>,
        fish: Option<ShellCommand>,
    ) -> Self {
        Self {
            zsh: zsh.as_ref().map(|s| s.command().to_string()),
            bash: bash.as_ref().map(|s| s.command().to_string()),
            fish: fish.as_ref().map(|s| s.command().to_string()),
            zsh_env: zsh.and_then(|s| s.env().cloned()),
            bash_env: bash.and_then(|s| s.env().cloned()),
            fish_env: fish.and_then(|s| s.env().cloned()),
        }
    }
}

/// Try to load registry from external file, with fallback to embedded
fn get_registry_content() -> Result<(String, Option<PathBuf>), Error> {
    // Check for registry.toml next to the executable (allows user customization)
    if let Ok(exe_path) = std::env::current_exe() {
        let alongside = exe_path.parent().unwrap().join("registry.toml");
        if alongside.exists() {
            let content = std::fs::read_to_string(&alongside)
                .map_err(|e| Error::RegistryRead(alongside.clone(), e))?;
            return Ok((content, Some(alongside)));
        }
    }

    // Check XDG data directory for user-provided registry
    if let Some(data_dir) = dirs::data_dir() {
        let user_registry = data_dir.join("mise-completions-sync").join("registry.toml");
        if user_registry.exists() {
            let content = std::fs::read_to_string(&user_registry)
                .map_err(|e| Error::RegistryRead(user_registry.clone(), e))?;
            return Ok((content, Some(user_registry)));
        }
    }

    // Use embedded registry
    Ok((EMBEDDED_REGISTRY.to_string(), None))
}

pub fn load_registry() -> Result<Registry, Error> {
    let (content, path) = get_registry_content()?;
    let path_for_error = path.clone().unwrap_or_else(|| PathBuf::from("<embedded>"));

    let raw: RawRegistry =
        toml::from_str(&content).map_err(|e| Error::RegistryParse(path_for_error.clone(), e))?;

    // Check schema version
    match raw.schema_version {
        None => return Err(Error::MissingSchemaVersion),
        Some(v) if v != CURRENT_SCHEMA_VERSION => {
            return Err(Error::IncompatibleSchema {
                found: v,
                expected: CURRENT_SCHEMA_VERSION,
            })
        }
        Some(_) => {}
    }

    let mut tools = HashMap::new();

    for (tool_name, entry) in raw.tools {
        let completions = match entry {
            ToolEntry::Pattern(pattern_name) => {
                let pattern = raw.patterns.get(&pattern_name).ok_or_else(|| {
                    Error::UnknownPattern(tool_name.clone(), pattern_name.clone())
                })?;
                ToolCompletions::from_raw(
                    pattern.zsh.clone(),
                    pattern.bash.clone(),
                    pattern.fish.clone(),
                )
                .expand(&tool_name)
            }
            ToolEntry::Explicit(raw_completions) => ToolCompletions::from_raw(
                raw_completions.zsh,
                raw_completions.bash,
                raw_completions.fish,
            ),
        };
        tools.insert(tool_name, completions);
    }

    Ok(Registry { tools })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_with_env_vars() {
        let registry = load_registry().expect("Failed to load registry");

        // Check that prek has env vars
        if let Some(prek_completions) = registry.tools.get("prek") {
            // Test shell-specific env vars
            assert!(
                prek_completions.fish_env.is_some(),
                "prek should have fish env vars"
            );
            let fish_env = prek_completions.fish_env.as_ref().unwrap();
            assert_eq!(fish_env.get("COMPLETE"), Some(&"fish".to_string()));
        } else {
            panic!("prek not found in registry");
        }
    }

    #[test]
    fn test_registry_without_env_vars() {
        let registry = load_registry().expect("Failed to load registry");

        // Check that kubectl doesn't have env vars (uses pattern)
        if let Some(kubectl_completions) = registry.tools.get("kubectl") {
            assert!(
                kubectl_completions.zsh_env.is_none(),
                "kubectl should not have zsh env vars"
            );
            assert!(
                kubectl_completions.bash_env.is_none(),
                "kubectl should not have bash env vars"
            );
            assert!(
                kubectl_completions.fish_env.is_none(),
                "kubectl should not have fish env vars"
            );
        } else {
            panic!("kubectl not found in registry");
        }
    }

    #[test]
    fn test_shell_specific_env_vars() {
        // Test that different shells can have different env vars
        let registry = load_registry().expect("Failed to load registry");

        if let Some(prek_completions) = registry.tools.get("prek") {
            // Fish should have COMPLETE=fish
            assert!(prek_completions.fish_env.is_some());
            // Zsh and bash might have different or no env vars
        }
    }
}
