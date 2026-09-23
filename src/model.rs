use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub tasks: BTreeMap<String, Task>,
    #[serde(default)]
    pub variables: BTreeMap<String, Variable>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub tools: BTreeMap<String, Tool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub description: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default = "current_directory")]
    pub working_directory: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub parameters: BTreeMap<String, Parameter>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub destructive: bool,
    #[serde(default)]
    pub cleanup: Vec<Vec<String>>,
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub exclusive: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub consumers: Vec<String>,
    #[serde(default)]
    pub pass_environment: Vec<String>,
    pub compose: Option<Compose>,
    pub cache: Option<Cache>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub command: Vec<String>,
    pub version_contains: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Compose {
    pub file: String,
    pub service: String,
    pub project: String,
    #[serde(default)]
    pub build: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cache {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

fn current_directory() -> String {
    ".".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    #[serde(rename = "type")]
    pub kind: ValueType,
    pub default: Option<String>,
    #[serde(default)]
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueType {
    String,
    Boolean,
    Integer,
    Enum,
    Url,
}

impl ValueType {
    pub fn accepts(self, value: &str, values: &[String]) -> bool {
        match self {
            Self::String => true,
            Self::Boolean => matches!(value, "true" | "false"),
            Self::Integer => {
                let digits = value.strip_prefix('-').unwrap_or(value);
                value == "0"
                    || (!digits.is_empty()
                        && !digits.starts_with('0')
                        && digits.bytes().all(|c| c.is_ascii_digit()))
            }
            Self::Enum => values.iter().any(|candidate| candidate == value),
            Self::Url => url::Url::parse(value).is_ok(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    #[serde(rename = "type")]
    pub kind: ValueType,
    pub visibility: Visibility,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub required_in: Vec<String>,
    pub consumers: Vec<String>,
    #[serde(default)]
    pub browser_exposed: bool,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Internal,
    Secret,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub profile: String,
    pub tasks: Vec<PlannedTask>,
    pub tools: BTreeMap<String, Tool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedTask {
    pub id: String,
    pub command: Vec<String>,
    pub working_directory: String,
    pub environment: BTreeMap<String, String>,
    pub destructive: bool,
    pub depends_on: Vec<String>,
    pub cleanup: Vec<Vec<String>>,
    pub timeout_seconds: Option<u64>,
    pub exclusive: Vec<String>,
    pub requires: Vec<String>,
    pub consumers: Vec<String>,
    pub pass_environment: Vec<String>,
    pub compose: Option<Compose>,
    pub cache: Option<Cache>,
}
