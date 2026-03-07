use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::Config;

#[derive(Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub ts: i64,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub name: String,
    pub created: i64,
    #[serde(default)]
    pub system_prompt: String,
}

pub struct Store {
    base_dir: PathBuf,
}

impl Store {
    pub fn new(config: &Config) -> Result<Self> {
        let base_dir = config.data_dir.clone();
        fs::create_dir_all(base_dir.join("projects"))?;
        Ok(Self { base_dir })
    }

    fn projects_dir(&self) -> PathBuf {
        self.base_dir.join("projects")
    }

    fn project_dir(&self, project: &str) -> PathBuf {
        self.projects_dir().join(project)
    }

    fn threads_dir(&self, project: &str) -> PathBuf {
        self.project_dir(project).join("threads")
    }

    fn thread_file(&self, project: &str, thread: &str) -> PathBuf {
        self.threads_dir(project).join(format!("{}.jsonl", thread))
    }

    pub fn list_projects(&self) -> Result<Vec<String>> {
        let mut projects = Vec::new();
        let dir = self.projects_dir();
        if dir.exists() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    projects.push(entry.file_name().to_string_lossy().to_string());
                }
            }
        }
        projects.sort();
        Ok(projects)
    }

    pub fn create_project(&self, name: &str) -> Result<()> {
        let dir = self.project_dir(name);
        fs::create_dir_all(dir.join("threads"))?;
        let meta = ProjectMeta {
            name: name.to_string(),
            created: Utc::now().timestamp(),
            system_prompt: String::new(),
        };
        let meta_path = self.project_dir(name).join("project.json");
        let json = serde_json::to_string(&meta)?;
        fs::write(meta_path, json)?;
        Ok(())
    }

    pub fn list_threads(&self, project: &str) -> Result<Vec<String>> {
        let mut threads = Vec::new();
        let dir = self.threads_dir(project);
        if dir.exists() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".jsonl") {
                    threads.push(name.trim_end_matches(".jsonl").to_string());
                }
            }
        }
        threads.sort();
        Ok(threads)
    }

    pub fn create_thread(&self, project: &str, name: &str) -> Result<()> {
        let dir = self.threads_dir(project);
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.jsonl", name));
        // Create empty file
        fs::File::create(path)?;
        Ok(())
    }

    pub fn append_message(
        &self,
        project: &str,
        thread: &str,
        role: &str,
        content: &str,
    ) -> Result<()> {
        let msg = Message {
            id: Uuid::new_v4().to_string(),
            ts: Utc::now().timestamp(),
            role: role.to_string(),
            content: content.to_string(),
            refs: Vec::new(),
            tags: Vec::new(),
            model: None,
            tokens_in: None,
            tokens_out: None,
            cost_usd: None,
        };
        let path = self.thread_file(project, thread);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let json = serde_json::to_string(&msg)?;
        writeln!(file, "{}", json)?;
        Ok(())
    }

    pub fn read_thread(&self, project: &str, thread: &str) -> Result<Vec<Message>> {
        let path = self.thread_file(project, thread);
        let mut messages = Vec::new();
        if path.exists() {
            let file = fs::File::open(path)?;
            let reader = std::io::BufReader::new(file);
            for line in reader.lines() {
                let line = line?;
                if !line.trim().is_empty() {
                    if let Ok(msg) = serde_json::from_str::<Message>(&line) {
                        messages.push(msg);
                    }
                }
            }
        }
        Ok(messages)
    }
}
