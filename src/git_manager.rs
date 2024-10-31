use anyhow::Result;
use std::path::PathBuf;
use tokio::process::Command;

pub struct GitManager {
    repo_url: String,
    repo_dir: PathBuf,
}

impl GitManager {
    pub fn new(username: String, token: String, repo_url: &str) -> Self {
        let repo_name = repo_url
            .split('/')
            .last()
            .unwrap_or("repo")
            .to_string();

        let repo_url = if repo_url.contains("@") {
            let parts: Vec<&str> = repo_url.splitn(2, '@').collect();
            format!("https://{}:{}@{}", username, token, parts[1])
        } else {
            format!("https://{}:{}@{}", 
                username, 
                token, 
                repo_url.trim_start_matches("https://"))
        };
        
        let repo_dir = std::env::current_dir()
            .expect("Failed to get current directory")
            .join("temp_repos")
            .join(repo_name);

        Self {
            repo_url,
            repo_dir,
        }
    }

    pub async fn test_connection(&self) -> Result<()> {
        let repo_name = self.repo_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
            
        println!("Testing Git connection for {}...", repo_name);
        
        let output = Command::new("git")
            .arg("ls-remote")
            .arg("--heads")
            .arg(&self.repo_url)
            .output()
            .await?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            println!("✗ Failed to connect to repository {}", repo_name);
            println!("Error: {}", error);
            return Err(anyhow::anyhow!("Failed to connect to repository {}: {}", repo_name, error));
        }

        println!("✓ Successfully connected to repository {}", repo_name);
        Ok(())
    }

    pub async fn ensure_repo_exists(&self) -> Result<()> {
        if self.repo_dir.exists() {
            self.update_repo().await
        } else {
            self.clone_repo().await
        }
    }

    async fn clone_repo(&self) -> Result<()> {
        let repo_name = self.repo_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
            
        println!("\nCloning repository {}...", repo_name);
        
        // Create parent directory if it doesn't exist
        tokio::fs::create_dir_all(self.repo_dir.parent().unwrap()).await?;
        
        let output = Command::new("git")
            .arg("clone")
            .arg(&self.repo_url)
            .arg(repo_name)
            .current_dir(self.repo_dir.parent().unwrap())
            .output()
            .await?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            println!("✗ Failed to clone repository {}", repo_name);
            println!("Error: {}", error);
            return Err(anyhow::anyhow!("Failed to clone repository {}", repo_name));
        }

        println!("✓ Successfully cloned repository {}", repo_name);
        Ok(())
    }

    async fn update_repo(&self) -> Result<()> {
        let repo_name = self.repo_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        
        println!("Repository {} exists, updating...", repo_name);
        
        // Reset any local changes
        let reset_output = Command::new("git")
            .args(["reset", "--hard", "HEAD"])
            .current_dir(&self.repo_dir)
            .output()
            .await?;

        if !reset_output.status.success() {
            println!("✗ Failed to reset repository {}", repo_name);
            return Err(anyhow::anyhow!("Failed to reset repository {}", repo_name));
        }

        // Pull latest changes
        let pull_output = Command::new("git")
            .args(["pull", "--force"])
            .current_dir(&self.repo_dir)
            .output()
            .await?;

        if !pull_output.status.success() {
            let error = String::from_utf8_lossy(&pull_output.stderr);
            println!("✗ Failed to update repository {}", repo_name);
            println!("Error: {}", error);
            return Err(anyhow::anyhow!("Failed to update repository {}", repo_name));
        }

        println!("✓ Successfully updated repository {}", repo_name);
        Ok(())
    }
}