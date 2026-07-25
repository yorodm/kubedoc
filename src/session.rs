use std::path::PathBuf;

use chrono::Local;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub session_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub entries: Vec<Entry>,
}

pub struct SessionManager {
    sessions_dir: PathBuf,
}

impl SessionManager {
    pub fn new(data_dir: Option<std::path::PathBuf>) -> anyhow::Result<Self> {
        let dir = data_dir
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join(".kubedoc")
            })
            .join("sessions");
        std::fs::create_dir_all(&dir)?;
        Ok(Self { sessions_dir: dir })
    }

    #[cfg(test)]
    pub fn with_dir(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }

    pub fn create(&self) -> SessionData {
        let now = Local::now();
        let id = now.format("session_%Y-%m-%d_%H%M%S%.f").to_string();
        let ts = now.to_rfc3339();
        SessionData {
            session_id: id,
            created_at: ts.clone(),
            updated_at: ts,
            entries: Vec::new(),
        }
    }

    pub fn list(&self) -> anyhow::Result<Vec<SessionData>> {
        let mut sessions = Vec::new();
        for entry in std::fs::read_dir(&self.sessions_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json")
                && let Ok(content) = std::fs::read_to_string(&path)
                    && let Ok(data) = serde_json::from_str::<SessionData>(&content) {
                        sessions.push(data);
                    }
        }
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    pub fn load(&self, session_id: &str) -> anyhow::Result<Option<SessionData>> {
        let path = self.session_path(session_id);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)?;
        Ok(Some(serde_json::from_str(&content)?))
    }

    pub fn save(&self, data: &SessionData) -> anyhow::Result<()> {
        let path = self.session_path(&data.session_id);
        let content = serde_json::to_string_pretty(data)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    pub fn add_entry(&self, data: &mut SessionData, role: &str, content: &str) -> anyhow::Result<()> {
        data.entries.push(Entry {
            role: role.to_string(),
            content: content.to_string(),
        });
        data.updated_at = Local::now().to_rfc3339();
        self.save(data)
    }

    pub fn delete(&self, session_id: &str) -> anyhow::Result<()> {
        let path = self.session_path(session_id);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        self.sessions_dir.join(format!("{session_id}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_session() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let manager = SessionManager::with_dir(dir.clone());
        let session = manager.create();

        assert!(!session.session_id.is_empty());
        assert!(!session.created_at.is_empty());
        assert_eq!(session.entries.len(), 0);
        assert_eq!(session.updated_at, session.created_at);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_and_load_session() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let manager = SessionManager::with_dir(dir.clone());

        let mut session = manager.create();
        session.entries.push(Entry {
            role: "user".into(),
            content: "hello".into(),
        });
        manager.save(&session).unwrap();

        let loaded = manager.load(&session.session_id).unwrap().unwrap();
        assert_eq!(loaded.session_id, session.session_id);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].role, "user");
        assert_eq!(loaded.entries[0].content, "hello");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_add_entry() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let manager = SessionManager::with_dir(dir.clone());

        let mut session = manager.create();
        manager
            .add_entry(&mut session, "assistant", "hi there")
            .unwrap();

        assert_eq!(session.entries.len(), 1);
        assert!(session.updated_at > session.created_at);

        let loaded = manager.load(&session.session_id).unwrap().unwrap();
        assert_eq!(loaded.entries[0].content, "hi there");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_delete_session() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let manager = SessionManager::with_dir(dir.clone());

        let session = manager.create();
        manager.save(&session).unwrap();
        assert!(manager.load(&session.session_id).unwrap().is_some());

        manager.delete(&session.session_id).unwrap();
        assert!(manager.load(&session.session_id).unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_nonexistent() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let manager = SessionManager::with_dir(dir.clone());

        let result = manager.load("nonexistent-session-id").unwrap();
        assert!(result.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_list_sessions() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let manager = SessionManager::with_dir(dir.clone());

        let s1 = manager.create();
        manager.save(&s1).unwrap();

        let s2 = manager.create();
        manager.save(&s2).unwrap();

        let sessions = manager.list().unwrap();
        assert_eq!(sessions.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
