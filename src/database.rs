use anyhow::Result;
use rusqlite::{Connection, params};
use crate::SupportedTask;
use crate::TaskValidState;
use crate::git_manager::GitManager;
use std::path::PathBuf;
use crate::config::Config;

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn new() -> Result<Self> {
        let db_path = std::env::current_dir()?.join("gavin.db");
        let conn = Connection::open(db_path)?;
        
        conn.execute(
            "CREATE TABLE IF NOT EXISTS repositories (
                id INTEGER PRIMARY KEY,
                url TEXT NOT NULL UNIQUE
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS git_credentials (
                id INTEGER PRIMARY KEY,
                username TEXT NOT NULL,
                token BLOB NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS valid_states (
                id INTEGER PRIMARY KEY,
                task TEXT NOT NULL,
                state_json TEXT NOT NULL
            )",
            [],
        )?;

        Ok(Database { conn })
    }

    pub async fn add_repository(&self, url: &str, is_new: bool) -> Result<()> {
        let credentials = self.get_git_credentials()?
            .ok_or_else(|| anyhow::anyhow!("Git credentials not found. Please set them first with --set-git-credentials"))?;
        
        let git_manager = GitManager::new(credentials.0, credentials.1, url);
        
        if is_new {
            git_manager.ensure_repo_exists_new().await?;
        } else {
            git_manager.ensure_repo_exists().await?;
        }
        
        self.add_repository_sync(url)?;
        Ok(())
    }

    pub fn delete_repository(&self, url: &str) -> Result<()> {
        let rows_affected = self.conn.execute(
            "DELETE FROM repositories WHERE url = ?1",
            params![url],
        )?;

        if rows_affected > 0 {
            println!("Deleted repository: {}", url);
        } else {
            println!("Repository not found: {}", url);
        }
        Ok(())
    }

    pub fn list_repositories(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT url FROM repositories")?;
        let urls = stmt.query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<String>, _>>()?;
        
        Ok(urls)
    }

    fn serialize_state(state: &TaskValidState) -> Result<String> {
        serde_json::to_string(state)
            .map_err(|e| anyhow::anyhow!("Failed to serialize state: {}", e))
    }

    fn deserialize_state(json: &str) -> Result<TaskValidState> {
        serde_json::from_str(json)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize state: {}", e))
    }

    pub fn add_valid_state(&self, task: &SupportedTask, state: &TaskValidState) -> Result<()> {
        let state_json = Self::serialize_state(state)?;
        // println!("Storing task '{}' with state_json: {}", task, state_json);
        self.conn.execute(
            "INSERT INTO valid_states (task, state_json) VALUES (?1, ?2)",
            params![task.to_string(), state_json],
        )?;
        Ok(())
    }

    pub fn list_valid_states(&self, task: &SupportedTask) -> Result<Vec<TaskValidState>> {
        let mut stmt = self.prepare_statement(
            "SELECT DISTINCT state_json FROM valid_states WHERE LOWER(task) = LOWER(?1)"
        )?;
        
        let states = stmt.query_map([&task.to_string()], |row| {
            let json: String = row.get(0)?;
            println!("Found state_json: {}", json);
            Self::deserialize_state(&json)
                .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))
        })?;

        let mut result = Vec::new();
        for state in states {
            result.push(state?);
        }
        Ok(result)
    }

    pub fn delete_valid_state(&self, task: &SupportedTask, state: &TaskValidState) -> Result<()> {
        let task_name = task.to_string().to_lowercase();
        let state_json = serde_json::to_string(state)
            .map_err(|e| anyhow::anyhow!("Failed to serialize state: {}", e))?;

        self.conn.execute(
            "DELETE FROM valid_states WHERE task = ?1 AND state_json = ?2",
            params![task_name, state_json],
        )?;

        Ok(())
    }

    pub fn set_git_credentials(&self, credentials: &str) -> Result<()> {
        let parts: Vec<&str> = credentials.split(':').collect();
        if parts.len() != 2 {
            return Err(anyhow::anyhow!("Invalid credentials format. Expected 'username:token'"));
        }
        
        let username = parts[0];
        let token = parts[1];
        
        let encrypted_token = token.as_bytes().iter()
            .map(|b| b ^ 0xFF)
            .collect::<Vec<u8>>();
        
        self.conn.execute("DELETE FROM git_credentials", [])?;
        
        self.conn.execute(
            "INSERT INTO git_credentials (username, token) VALUES (?1, ?2)",
            params![username, encrypted_token],
        )?;
        
        Ok(())
    }

    pub fn get_git_credentials(&self) -> Result<Option<(String, String)>> {
        let result = self.conn.query_row(
            "SELECT username, token FROM git_credentials LIMIT 1",
            [],
            |row| {
                let username: String = row.get(0)?;
                let encrypted_token: Vec<u8> = row.get(1)?;
                
                let token = encrypted_token.iter()
                    .map(|b| b ^ 0xFF)
                    .collect::<Vec<u8>>();
                
                let token = String::from_utf8(token)
                    .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
                
                Ok((username, token))
            },
        );
        
        match result {
            Ok(creds) => Ok(Some(creds)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn get_local_path(&self, repo_url: &str) -> PathBuf {
        let repo_name = repo_url
            .split('/')
            .last()
            .unwrap_or("repo");
        
        std::env::current_dir()
            .expect("Failed to get current directory")
            .join("temp_repos")
            .join(repo_name)
    }

    pub fn add_repository_sync(&self, url: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO repositories (url) VALUES (?1)",
            params![url],
        )?;
        Ok(())
    }

    pub fn merge_config_states(&self, config: &Config) -> Result<()> {
        // Only merge states if they exist in the config
        if !config.task_states.gitversion.is_empty() {
            // Clear existing gitversion states
            self.conn.execute(
                "DELETE FROM valid_states WHERE LOWER(task) = 'gitversion'", 
                [],
            )?;
            
            // Add gitversion states
            let task = SupportedTask::Gitversion;
            for state in config.get_valid_states(&task) {
                self.add_valid_state(&task, &state)?;
            }
        }

        // Handle other tasks from config
        for (task_name, versions) in &config.task_states.other_tasks {
            if !versions.is_empty() {
                let task = SupportedTask::Default(task_name.clone());
                // Clear existing states for this task
                self.conn.execute(
                    "DELETE FROM valid_states WHERE LOWER(task) = LOWER(?1)",
                    params![task_name],
                )?;
                
                // Add new states
                for state in config.get_valid_states(&task) {
                    self.add_valid_state(&task, &state)?;
                }
            }
        }
        
        Ok(())
    }

    pub fn prepare_statement(&self, sql: &str) -> Result<rusqlite::Statement> {
        Ok(self.conn.prepare(sql)?)
    }

    pub fn get_all_tasks(&self) -> Result<Vec<SupportedTask>> {
        let mut tasks = vec![SupportedTask::Gitversion];
        
        // Add any other tasks found in the database
        let mut stmt = self.prepare_statement("SELECT DISTINCT task FROM valid_states")?;
        let task_iter = stmt.query_map([], |row| {
            let task_str: String = row.get(0)?;
            Ok(task_str)
        })?;

        for task_result in task_iter {
            let task_str = task_result?;
            if task_str.to_lowercase() != "gitversion" {
                tasks.push(SupportedTask::Default(task_str));
            }
        }

        Ok(tasks)
    }
}
