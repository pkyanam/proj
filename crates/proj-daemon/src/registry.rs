//! Project registry - handles project CRUD operations

use anyhow::{Context, Result};
use proj_common::{project_dir, projects_dir, Project};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;

/// Project registry for managing project metadata
pub struct Registry {
    projects: HashMap<String, Project>,
}

impl Registry {
    /// Create a new registry, loading existing projects from disk
    pub async fn new() -> Result<Self> {
        let mut registry = Self {
            projects: HashMap::new(),
        };
        registry.load_all().await?;
        Ok(registry)
    }

    /// Load all projects from disk
    async fn load_all(&mut self) -> Result<()> {
        let projects_path = projects_dir()?;

        if !projects_path.exists() {
            fs::create_dir_all(&projects_path)
                .await
                .context("Failed to create projects directory")?;
            return Ok(());
        }

        let mut entries = fs::read_dir(&projects_path)
            .await
            .context("Failed to read projects directory")?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                let project_file = path.join("project.json");
                if project_file.exists() {
                    match self.load_project(&project_file).await {
                        Ok(project) => {
                            self.projects.insert(project.name.clone(), project);
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to load project from {:?}: {}",
                                project_file,
                                e
                            );
                        }
                    }
                }
            }
        }

        tracing::info!("Loaded {} projects", self.projects.len());
        Ok(())
    }

    /// Load a single project from disk
    async fn load_project(&self, path: &PathBuf) -> Result<Project> {
        let content = fs::read_to_string(path)
            .await
            .context("Failed to read project file")?;
        let project: Project =
            serde_json::from_str(&content).context("Failed to parse project file")?;
        Ok(project)
    }

    /// Save a project to disk
    async fn save_project(&self, project: &Project) -> Result<()> {
        let dir = project_dir(&project.name)?;
        fs::create_dir_all(&dir)
            .await
            .context("Failed to create project directory")?;

        let project_file = dir.join("project.json");
        let content =
            serde_json::to_string_pretty(project).context("Failed to serialize project")?;
        fs::write(&project_file, content)
            .await
            .context("Failed to write project file")?;

        // Create chrome profile directory
        let chrome_dir = dir.join("chrome");
        fs::create_dir_all(&chrome_dir)
            .await
            .context("Failed to create chrome directory")?;

        Ok(())
    }

    /// Create a new project
    pub async fn create(&mut self, name: String, root_dir: PathBuf) -> Result<Project> {
        proj_common::validate_project_name(&name)?;

        if self.projects.contains_key(&name) {
            anyhow::bail!("Project '{}' already exists", name);
        }

        let project = Project::new(name.clone(), root_dir);
        self.save_project(&project).await?;
        self.projects.insert(name, project.clone());

        tracing::info!("Created project: {}", project.name);
        Ok(project)
    }

    /// Get a project by name
    pub fn get(&self, name: &str) -> Option<&Project> {
        self.projects.get(name)
    }

    /// Get a mutable reference to a project
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Project> {
        self.projects.get_mut(name)
    }

    /// List all projects
    pub fn list(&self) -> Vec<&Project> {
        self.projects.values().collect()
    }

    /// Update a project's port
    pub async fn update_port(&mut self, name: &str, port: Option<u16>) -> Result<()> {
        {
            let project = self
                .projects
                .get_mut(name)
                .context(format!("Project '{}' not found", name))?;
            project.port = port;
        }
        // Re-borrow immutably after the mutable borrow is released
        let project = self
            .projects
            .get(name)
            .context(format!("Project '{}' not found", name))?;
        self.save_project(project).await?;
        Ok(())
    }

    /// Get project count
    pub fn count(&self) -> usize {
        self.projects.len()
    }

    /// Find project by port
    pub fn find_by_port(&self, port: u16) -> Option<&Project> {
        self.projects.values().find(|p| p.port == Some(port))
    }
}
