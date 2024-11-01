use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitVersionState {
    pub setup_version: String,
    pub execute_version: String,
    pub spec_version: String,
}

#[derive(Debug)]
pub struct GitVersionImplementation {
    pub setup: Option<(String, Option<String>)>, // (version, spec_version)
    pub execute: Option<String>,                 // version
    pub file_path: PathBuf,
}

impl GitVersionState {
    pub fn new(setup: &str, execute: &str, spec: &str) -> Self {
        GitVersionState {
            setup_version: setup.to_string(),
            execute_version: execute.to_string(),
            spec_version: spec.to_string(),
        }
    }

    pub fn from_string(s: &str) -> Result<Self, String> {
        // Expected format: "setup:VERSION,execute:VERSION,spec:VERSION"
        let parts: Vec<&str> = s.split(',').collect();
        if parts.len() != 3 {
            return Err(
                "Invalid format. Expected 'setup:VERSION,execute:VERSION,spec:VERSION'".to_string(),
            );
        }

        let mut setup = None;
        let mut execute = None;
        let mut spec = None;

        for part in parts {
            let kv: Vec<&str> = part.split(':').collect();
            if kv.len() != 2 {
                return Err("Invalid key-value pair".to_string());
            }
            match kv[0].trim() {
                "setup" => setup = Some(kv[1].trim()),
                "execute" => execute = Some(kv[1].trim()),
                "spec" => spec = Some(kv[1].trim()),
                _ => return Err(format!("Unknown key: {}", kv[0])),
            }
        }

        Ok(GitVersionState::new(
            setup.ok_or("Missing setup version")?,
            execute.ok_or("Missing execute version")?,
            spec.ok_or("Missing spec version")?,
        ))
    }
}
