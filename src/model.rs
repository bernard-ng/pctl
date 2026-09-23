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
}

impl ValueType {
    pub fn accepts(self, value: &str, values: &[String]) -> bool {
        match self {
            Self::String => true,
            Self::Boolean => matches!(value, "true" | "false"),
            Self::Integer => value.parse::<i64>().is_ok(),
            Self::Enum => values.iter().any(|candidate| candidate == value),
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

#[derive(Debug, Serialize)]
pub struct Plan {
    pub profile: String,
    pub tasks: Vec<PlannedTask>,
}

#[derive(Debug, Serialize)]
pub struct PlannedTask {
    pub id: String,
    pub command: Vec<String>,
    pub working_directory: String,
    pub environment: BTreeMap<String, String>,
    pub destructive: bool,
}
