//! Hexagonal persistence port for Docker Compose directories.

use super::domain::{DockerError, SavedComposeDir};

pub trait DockerProjectStore: Send + Sync {
    fn load(&self) -> Result<Vec<SavedComposeDir>, DockerError>;
    fn save(&self, directories: &[SavedComposeDir]) -> Result<(), DockerError>;
}
