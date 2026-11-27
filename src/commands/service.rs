//! Application service (`CommandManager`) for the `commands` bounded context.

use super::domain::{
    CommandAlias, CommandError, CommandEvent, CommandStatus, CommandText, ManagedCommand,
    SavedCommand, derive_stop_command, is_detached_compose_command,
};
use super::infra::{OsProcessRunner, TomlCommandStore};
use super::ports::{CommandRepository, ProcessRunner};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

pub struct CommandManager {
    pub entries: Vec<ManagedCommand>,
    next_id: u64,
    repo: Box<dyn CommandRepository>,
    runner: Box<dyn ProcessRunner>,
}

impl CommandManager {
    pub fn default_path() -> PathBuf {
        TomlCommandStore::default_path()
    }

    pub fn load() -> Self {
        Self::with_path(Self::default_path())
    }

    pub fn with_path(config_path: PathBuf) -> Self {
        Self::with_ports(
            Box::new(TomlCommandStore::new(config_path)),
            Box::new(OsProcessRunner),
        )
    }

    /// Dependency-injected constructor for pure unit tests and custom adapters.
    pub fn with_ports(
        repo: Box<dyn CommandRepository>,
        runner: Box<dyn ProcessRunner>,
    ) -> Self {
        let mut entries = Vec::new();
        let mut max_id = 0u64;

        match repo.load() {
            Ok(saved_list) => {
                for saved in saved_list {
                    max_id = max_id.max(saved.id);
                    entries.push(ManagedCommand::new(
                        saved.id,
                        saved.name,
                        saved.command,
                        saved.directory,
                    ));
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "Failed to load saved commands; starting empty");
            }
        }

        Self {
            entries,
            next_id: max_id + 1,
            repo,
            runner,
        }
    }

    pub fn save(&self) -> Result<(), CommandError> {
        let snapshot: Vec<SavedCommand> = self.entries.iter().map(ManagedCommand::to_saved).collect();
        self.repo.save(&snapshot)
    }

    pub fn active_count(&self) -> usize {
        self.entries.iter().filter(|e| e.status.is_active()).count()
    }

    pub fn find_by_id(&self, id: u64) -> Option<&ManagedCommand> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn find_by_id_mut(&mut self, id: u64) -> Option<&mut ManagedCommand> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    #[tracing::instrument(skip(self))]
    pub fn add(&mut self, name: String, command: String, directory: String) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let alias = CommandAlias::from_raw(&name).into_inner();
        let cmd = CommandText::parse(&command)
            .map(CommandText::into_inner)
            .unwrap_or_else(|_| command.trim().to_string());
        let dir = directory.trim().to_string();
        self.entries.push(ManagedCommand::new(id, alias, cmd, dir));
        if let Err(err) = self.save() {
            tracing::warn!(id, error = %err, "Failed to persist added command");
        }
        id
    }

    #[tracing::instrument(skip(self))]
    pub fn update(&mut self, id: u64, name: String, command: String, directory: String) -> bool {
        if let Some(entry) = self.find_by_id_mut(id) {
            entry.name = CommandAlias::from_raw(&name).into_inner();
            entry.command = CommandText::parse(&command)
                .map(CommandText::into_inner)
                .unwrap_or_else(|_| command.trim().to_string());
            entry.directory = directory.trim().to_string();
            if let Err(err) = self.save() {
                tracing::warn!(id, error = %err, "Failed to persist updated command");
            }
            true
        } else {
            false
        }
    }

    #[tracing::instrument(skip(self, on_event))]
    pub fn remove(
        &mut self,
        id: u64,
        on_event: Arc<dyn Fn(CommandEvent) + Send + Sync>,
    ) -> bool {
        if let Some(entry) = self.find_by_id(id) {
            if entry.status.can_stop() {
                let _ = self.stop(id, on_event);
            }
        }
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        if self.entries.len() != before {
            if let Err(err) = self.save() {
                tracing::warn!(id, error = %err, "Failed to persist command removal");
            }
            true
        } else {
            false
        }
    }

    #[tracing::instrument(skip(self, on_event))]
    pub fn start(
        &mut self,
        id: u64,
        on_event: Arc<dyn Fn(CommandEvent) + Send + Sync>,
    ) -> Result<(), CommandError> {
        let (cmd_str, dir_str) = {
            let Some(entry) = self.find_by_id(id) else {
                return Err(CommandError::NotFound(id));
            };
            if entry.status == CommandStatus::Running {
                return Err(CommandError::AlreadyRunning(id));
            }
            (entry.command.clone(), entry.directory.clone())
        };

        let resolved_dir = self.runner.resolve_dir(&dir_str);
        if !self.runner.dir_exists(&resolved_dir) {
            let err = CommandError::DirectoryNotFound(resolved_dir);
            let msg = err.to_string();
            if let Some(entry) = self.find_by_id_mut(id) {
                entry.status = CommandStatus::Failed;
                entry.last_error = Some(msg.clone());
                entry.push_log(format!("✖ {msg}"));
            }
            return Err(err);
        }

        let pid = self
            .runner
            .spawn(id, &cmd_str, &resolved_dir, false, on_event)
            .map_err(|err| {
                let msg = err.to_string();
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.status = CommandStatus::Failed;
                    entry.last_error = Some(msg.clone());
                    entry.push_log(format!("✖ {msg}"));
                }
                err
            })?;

        if let Some(entry) = self.find_by_id_mut(id) {
            entry.status = CommandStatus::Running;
            entry.pid = Some(pid);
            entry.started_at = Some(Instant::now());
            entry.last_error = None;
            entry.push_log(format!("▶ $ {}  (in {})", cmd_str, resolved_dir.display()));
        }

        Ok(())
    }

    #[tracing::instrument(skip(self, on_event))]
    pub fn stop(
        &mut self,
        id: u64,
        on_event: Arc<dyn Fn(CommandEvent) + Send + Sync>,
    ) -> Result<String, CommandError> {
        let (status, pid, cmd_str, dir_str) = {
            let Some(entry) = self.find_by_id(id) else {
                return Err(CommandError::NotFound(id));
            };
            (
                entry.status,
                entry.pid,
                entry.command.clone(),
                entry.directory.clone(),
            )
        };

        match status {
            CommandStatus::Running => {
                if let Some(pid) = pid {
                    if let Some(entry) = self.find_by_id_mut(id) {
                        entry.status = CommandStatus::Stopping;
                        entry.push_log(format!("■ Stopping process group (PID {pid})..."));
                    }
                    self.runner.terminate_group(pid);
                    Ok(format!("Stopping PID {pid}..."))
                } else {
                    if let Some(entry) = self.find_by_id_mut(id) {
                        entry.status = CommandStatus::Stopped;
                        entry.started_at = None;
                    }
                    Ok("Stopped".to_string())
                }
            }
            CommandStatus::Detached => {
                let stop_cmd = derive_stop_command(&cmd_str).ok_or(CommandError::NoStopCommand)?;
                let resolved_dir = self.runner.resolve_dir(&dir_str);
                if !self.runner.dir_exists(&resolved_dir) {
                    return Err(CommandError::DirectoryNotFound(resolved_dir));
                }
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.status = CommandStatus::Stopping;
                    entry.push_log(format!("■ $ {stop_cmd}"));
                }
                let stop_pid = self
                    .runner
                    .spawn(id, &stop_cmd, &resolved_dir, true, on_event)?;
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.pid = Some(stop_pid);
                }
                Ok(format!("Running `{stop_cmd}`..."))
            }
            _ => Err(CommandError::NotRunning(id)),
        }
    }

    pub fn stop_all(&mut self, on_event: Arc<dyn Fn(CommandEvent) + Send + Sync>) -> usize {
        let ids: Vec<u64> = self
            .entries
            .iter()
            .filter(|e| e.status.can_stop())
            .map(|e| e.id)
            .collect();
        let mut count = 0;
        for id in ids {
            if self.stop(id, on_event.clone()).is_ok() {
                count += 1;
            }
        }
        count
    }

    /// Terminate any foreground running processes on application exit (leaves detached containers running).
    pub fn shutdown_foreground(&mut self) {
        for entry in &mut self.entries {
            if entry.status == CommandStatus::Running {
                if let Some(pid) = entry.pid {
                    self.runner.signal_foreground_shutdown(pid);
                }
            }
        }
    }

    pub fn apply_event(&mut self, event: CommandEvent) {
        match event {
            CommandEvent::Output { id, line } => {
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.push_log(line);
                }
            }
            CommandEvent::Exited {
                id,
                code,
                was_stop_action,
            } => {
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.pid = None;
                    if was_stop_action {
                        entry.started_at = None;
                        if code == Some(0) {
                            entry.status = CommandStatus::Stopped;
                            entry.push_log("■ Stopped.".to_string());
                        } else {
                            let c = code.unwrap_or(-1);
                            entry.status = CommandStatus::Exited(c);
                            entry.push_log(format!("■ Stop command exited with code {c}."));
                        }
                    } else if entry.status == CommandStatus::Stopping {
                        entry.status = CommandStatus::Stopped;
                        entry.started_at = None;
                        entry.push_log("■ Stopped.".to_string());
                    } else if code == Some(0) && is_detached_compose_command(&entry.command) {
                        entry.status = CommandStatus::Detached;
                        entry.push_log(
                            "● Started in detached mode (press Stop to run compose down)."
                                .to_string(),
                        );
                    } else {
                        let c = code.unwrap_or(-1);
                        entry.status = CommandStatus::Exited(c);
                        entry.started_at = None;
                        entry.push_log(format!("■ Process exited with code {c}."));
                    }
                }
            }
            CommandEvent::Error { id, message } => {
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.pid = None;
                    entry.started_at = None;
                    entry.status = CommandStatus::Failed;
                    entry.last_error = Some(message.clone());
                    entry.push_log(format!("✖ {message}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    struct InMemoryRepo {
        saved: Arc<Mutex<Vec<SavedCommand>>>,
    }

    impl CommandRepository for InMemoryRepo {
        fn load(&self) -> Result<Vec<SavedCommand>, CommandError> {
            Ok(self.saved.lock().unwrap().clone())
        }

        fn save(&self, commands: &[SavedCommand]) -> Result<(), CommandError> {
            *self.saved.lock().unwrap() = commands.to_vec();
            Ok(())
        }
    }

    struct FakeRunner {
        dir_ok: bool,
        spawned: Arc<Mutex<Vec<(u64, String, bool)>>>,
        terminated: Arc<Mutex<Vec<u32>>>,
    }

    impl ProcessRunner for FakeRunner {
        fn resolve_dir(&self, raw: &str) -> PathBuf {
            PathBuf::from(raw)
        }

        fn dir_exists(&self, _dir: &Path) -> bool {
            self.dir_ok
        }

        fn spawn(
            &self,
            id: u64,
            cmd: &str,
            _dir: &Path,
            was_stop_action: bool,
            _on_event: Arc<dyn Fn(CommandEvent) + Send + Sync>,
        ) -> Result<u32, CommandError> {
            self.spawned
                .lock()
                .unwrap()
                .push((id, cmd.to_string(), was_stop_action));
            Ok(4242)
        }

        fn terminate_group(&self, pid: u32) {
            self.terminated.lock().unwrap().push(pid);
        }

        fn signal_foreground_shutdown(&self, pid: u32) {
            self.terminated.lock().unwrap().push(pid);
        }
    }

    #[test]
    fn start_and_stop_lifecycle_with_in_memory_ports() {
        let store = Arc::new(Mutex::new(Vec::new()));
        let spawned = Arc::new(Mutex::new(Vec::new()));
        let terminated = Arc::new(Mutex::new(Vec::new()));

        let mut mgr = CommandManager::with_ports(
            Box::new(InMemoryRepo {
                saved: store.clone(),
            }),
            Box::new(FakeRunner {
                dir_ok: true,
                spawned: spawned.clone(),
                terminated: terminated.clone(),
            }),
        );

        let id = mgr.add(
            "  API  ".into(),
            "  docker compose up -d  ".into(),
            "/tmp".into(),
        );
        assert_eq!(store.lock().unwrap().len(), 1);
        assert_eq!(store.lock().unwrap()[0].name, "API");

        let cb: Arc<dyn Fn(CommandEvent) + Send + Sync> = Arc::new(|_| {});
        mgr.start(id, cb.clone()).unwrap();
        assert_eq!(mgr.find_by_id(id).unwrap().status, CommandStatus::Running);
        assert_eq!(spawned.lock().unwrap().len(), 1);

        // Simulate exit 0 -> detached compose command
        mgr.apply_event(CommandEvent::Exited {
            id,
            code: Some(0),
            was_stop_action: false,
        });
        assert_eq!(mgr.find_by_id(id).unwrap().status, CommandStatus::Detached);

        // Stopping a detached compose command spawns `docker compose down`
        let msg = mgr.stop(id, cb).unwrap();
        assert!(msg.contains("docker compose down"));
        assert_eq!(spawned.lock().unwrap().len(), 2);
        assert!(spawned.lock().unwrap()[1].2);
    }

    #[test]
    fn start_fails_with_typed_error_when_directory_missing() {
        let mut mgr = CommandManager::with_ports(
            Box::new(InMemoryRepo {
                saved: Arc::new(Mutex::new(Vec::new())),
            }),
            Box::new(FakeRunner {
                dir_ok: false,
                spawned: Arc::new(Mutex::new(Vec::new())),
                terminated: Arc::new(Mutex::new(Vec::new())),
            }),
        );

        let id = mgr.add("".into(), "echo hi".into(), "/nope".into());
        let cb: Arc<dyn Fn(CommandEvent) + Send + Sync> = Arc::new(|_| {});
        let err = mgr.start(id, cb).unwrap_err();
        assert!(matches!(err, CommandError::DirectoryNotFound(_)));
        assert_eq!(mgr.find_by_id(id).unwrap().status, CommandStatus::Failed);
    }
}
