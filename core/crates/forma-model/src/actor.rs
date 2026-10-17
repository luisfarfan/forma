//! Who performs a change.

use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::font::ModelError;

const AGENT_PREFIX: &str = "agent:";

/// The party that performs an operation, recorded for attribution.
///
/// Serialized (and displayed) as `"user"`, `"cli"` or `"agent:<name>"`. The
/// agent name must not be empty.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Actor {
    /// A person working in the editor.
    User,
    /// The command-line interface.
    Cli,
    /// An AI agent, identified by the name it reports (for MCP clients,
    /// `clientInfo.name`).
    Agent(String),
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User => f.write_str("user"),
            Self::Cli => f.write_str("cli"),
            Self::Agent(name) => write!(f, "{AGENT_PREFIX}{name}"),
        }
    }
}

impl FromStr for Actor {
    type Err = ModelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "cli" => Ok(Self::Cli),
            _ => match s.strip_prefix(AGENT_PREFIX) {
                Some(name) if !name.is_empty() => Ok(Self::Agent(name.to_owned())),
                _ => Err(ModelError::InvalidActor(s.to_owned())),
            },
        }
    }
}

impl Serialize for Actor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // An unnamed agent would serialize to a string that cannot be read back.
        if matches!(self, Self::Agent(name) if name.is_empty()) {
            return Err(S::Error::custom(ModelError::InvalidActor(self.to_string())));
        }
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Actor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

impl JsonSchema for Actor {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Actor".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "Who performs a change: `user`, `cli` or `agent:<name>`.",
            "pattern": "^(user|cli|agent:[\\s\\S]+)$"
        })
    }
}
