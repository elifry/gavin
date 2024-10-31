use serde::{Serialize, Deserialize};
use std::path::PathBuf;
use std::collections::HashMap;
use anyhow::Result;
use crate::{SupportedTask, TaskValidState, GitVersionState};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub task_states: TaskStates,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TaskStates {
    #[serde(default)]
    pub gitversion: Vec<GitVersionState>,
    // Add other task types here as needed
    #[serde(default)]
    pub other_tasks: HashMap<String, Vec<String>>,
}

impl Config {
    pub fn load(path: Option<&str>) -> Result<Self> {
        let path = path.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("gavinconfig.yml"));
        
        if !path.exists() {
            return Ok(Config {
                task_states: TaskStates::default(),
            });
        }

        let content = std::fs::read_to_string(&path)?;
        let config: Config = serde_yaml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse config file: {}", e))?;
        
        Ok(config)
    }

    pub fn get_valid_states(&self, task: &SupportedTask) -> Vec<TaskValidState> {
        match task {
            SupportedTask::Gitversion => self.task_states.gitversion
                .iter()
                .cloned()
                .map(TaskValidState::Gitversion)
                .collect(),
            // Add other task types here as needed
        }
    }
}
