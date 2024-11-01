use anyhow::Result;
use std::path::PathBuf;
use tokio::process::Command;

pub struct GitManager {
    repo_url: String,
    repo_dir: PathBuf,
}

impl GitManager {
    pub fn new(username: String, token: String, repo_url: &str) -> Self {
        let repo_name = repo_url.split('/').last().unwrap_or("repo").to_string();

        let repo_url = if repo_url.contains("@") {
            let parts: Vec<&str> = repo_url.splitn(2, '@').collect();
            format!("https://{}:{}@{}", username, token, parts[1])
        } else {
            format!(
                "https://{}:{}@{}",
                username,
                token,
                repo_url.trim_start_matches("https://")
            )
        };

        let repo_dir = std::env::current_dir()
            .expect("Failed to get current directory")
            .join("temp_repos")
            .join(repo_name);

        Self { repo_url, repo_dir }
    }

    pub async fn test_connection(&self) -> Result<()> {
        let repo_name = self
            .repo_dir
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
            return Err(anyhow::anyhow!(
                "Failed to connect to repository {}: {}",
                repo_name,
                error
            ));
        }

        println!("✓ Successfully connected to repository {}", repo_name);
        Ok(())
    }

    // Used when --new is not provided
    pub async fn ensure_repo_exists(&self) -> Result<()> {
        if self.repo_dir.exists() {
            self.update_repo().await
        } else {
            self.clone_repo().await
        }
    }

    // Used when --new is provided
    pub async fn ensure_repo_exists_new(&self) -> Result<()> {
        self.clone_repo().await
    }

    async fn clone_repo(&self) -> Result<()> {
        let repo_name = self
            .repo_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        println!("\nCloning repository {}...", repo_name);

        // Create the repository directory itself, not just the parent
        tokio::fs::create_dir_all(&self.repo_dir).await?;

        // Initialize empty repo
        let output = Command::new("git")
            .args(["init"])
            .current_dir(&self.repo_dir)
            .output()
            .await?;

        if !output.status.success() {
            return Err(anyhow::anyhow!("Failed to initialize repository"));
        }

        // Configure sparse checkout
        let output = Command::new("git")
            .args(["config", "core.sparseCheckout", "true"])
            .current_dir(&self.repo_dir)
            .output()
            .await?;

        if !output.status.success() {
            return Err(anyhow::anyhow!("Failed to configure sparse checkout"));
        }

        // Create sparse-checkout file with pipeline patterns
        let sparse_patterns = [
            "*.yml",
            "*.yaml",
            "**/azure-pipelines.yml",
            "**/azure-pipelines.yaml",
            "**/*.pipeline.yml",
            "**/*.pipeline.yaml",
            ".github/workflows/*.yml",
            ".github/workflows/*.yaml",
            ".gitlab-ci.yml",
        ];

        let sparse_checkout_dir = self.repo_dir.join(".git").join("info");
        tokio::fs::create_dir_all(&sparse_checkout_dir).await?;

        let sparse_checkout_file = sparse_checkout_dir.join("sparse-checkout");
        tokio::fs::write(&sparse_checkout_file, sparse_patterns.join("\n")).await?;

        // Add remote
        let output = Command::new("git")
            .args(["remote", "add", "origin", &self.repo_url])
            .current_dir(&self.repo_dir)
            .output()
            .await?;

        if !output.status.success() {
            return Err(anyhow::anyhow!("Failed to add remote"));
        }

        // Try branches in order: develop, main, master
        let default_branch = match Self::try_fetch_branch(&self.repo_dir, "develop").await {
            Ok(branch) => branch,
            Err(_) => match Self::try_fetch_branch(&self.repo_dir, "main").await {
                Ok(branch) => branch,
                Err(_) => match Self::try_fetch_branch(&self.repo_dir, "master").await {
                    Ok(branch) => branch,
                    Err(_) => {
                        return Err(anyhow::anyhow!(
                            "Failed to fetch repository: no default branch found"
                        ))
                    }
                },
            },
        };

        // Create and checkout the branch properly
        Command::new("git")
            .args([
                "checkout",
                "-b",
                &default_branch,
                &format!("origin/{}", default_branch),
            ])
            .current_dir(&self.repo_dir)
            .output()
            .await?;

        println!(
            "✓ Successfully cloned repository {} with sparse checkout",
            repo_name
        );
        Ok(())
    }

    // Helper function to try fetching a specific branch
    async fn try_fetch_branch(repo_dir: &PathBuf, branch_name: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["fetch", "--depth=1", "origin", branch_name])
            .current_dir(repo_dir)
            .output()
            .await?;

        if output.status.success() {
            Ok(branch_name.to_string())
        } else {
            Err(anyhow::anyhow!("Branch {} not found", branch_name))
        }
    }

    async fn update_repo(&self) -> Result<()> {
        let repo_name = self
            .repo_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        println!("Repository {} exists, updating...", repo_name);

        // Get current branch name
        let branch_output = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&self.repo_dir)
            .output()
            .await?;

        let current_branch = String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string();

        // If we're in detached HEAD state, try branches in order
        if current_branch == "HEAD" {
            let checkout_result = match Self::try_checkout_branch(&self.repo_dir, "develop").await {
                Ok(_) => Ok(()),
                Err(_) => match Self::try_checkout_branch(&self.repo_dir, "main").await {
                    Ok(_) => Ok(()),
                    Err(_) => Self::try_checkout_branch(&self.repo_dir, "master").await,
                },
            };

            if let Err(e) = checkout_result {
                return Err(anyhow::anyhow!(
                    "Failed to checkout any default branch: {}",
                    e
                ));
            }
        }

        // Ensure sparse checkout is enabled
        let output = Command::new("git")
            .args(["config", "core.sparseCheckout", "true"])
            .current_dir(&self.repo_dir)
            .output()
            .await?;

        if !output.status.success() {
            return Err(anyhow::anyhow!("Failed to configure sparse checkout"));
        }

        // Update sparse-checkout patterns if needed
        let sparse_patterns = [
            "*.yml",
            "*.yaml",
            "**/azure-pipelines.yml",
            "**/azure-pipelines.yaml",
            "**/*.pipeline.yml",
            "**/*.pipeline.yaml",
            ".github/workflows/*.yml",
            ".github/workflows/*.yaml",
            ".gitlab-ci.yml",
        ];

        let sparse_checkout_dir = self.repo_dir.join(".git").join("info");
        let sparse_checkout_file = sparse_checkout_dir.join("sparse-checkout");
        tokio::fs::write(&sparse_checkout_file, sparse_patterns.join("\n")).await?;

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

    // Helper function to try checking out a specific branch
    async fn try_checkout_branch(repo_dir: &PathBuf, branch_name: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["checkout", branch_name])
            .current_dir(repo_dir)
            .output()
            .await?;

        if output.status.success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Branch {} not found", branch_name))
        }
    }

    pub async fn ensure_repo_exists_no_update(&self) -> Result<()> {
        if self.repo_dir.exists() {
            Ok(()) // Skip update, just verify it exists
        } else {
            self.clone_repo().await
        }
    }
}
