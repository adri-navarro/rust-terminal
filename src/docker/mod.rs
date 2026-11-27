//! Bounded context: Docker Compose directories and running containers.

pub mod domain;
pub mod infra;
pub mod ports;
pub mod service;

pub use domain::{
    COMPOSE_FILES, CONTAINER_JOB_ID, ComposeAction, ComposeDir, ComposeStatus, DockerError,
    DockerEvent, MAX_LOG_LINES, RunningContainer, SavedComposeDir, parse_ps_output, reselect,
};
pub use infra::{
    TomlDockerStore, complete_directory, compose_args, compose_base, find_compose_file,
    project_rebuild_args, spawn_compose_job, spawn_compose_job_args, spawn_container_stop,
    spawn_ps_scan,
};
pub use ports::DockerProjectStore;
pub use service::DockerManager;
