//! Application service (`SystemMonitor`) for the `monitor` bounded context.

use super::domain::{ProcessSort, SystemSnapshot, sort_processes};
use super::infra::default_probe;
use super::ports::SystemProbe;
use std::collections::VecDeque;
use std::time::Instant;

pub struct SystemMonitor {
    probe: Box<dyn SystemProbe>,
    prev_sample_time: Option<Instant>,
    cpu_history: VecDeque<f32>,
}

impl Default for SystemMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemMonitor {
    pub fn new() -> Self {
        Self::with_probe(default_probe())
    }

    pub fn with_probe(probe: Box<dyn SystemProbe>) -> Self {
        Self {
            probe,
            prev_sample_time: None,
            cpu_history: VecDeque::with_capacity(48),
        }
    }

    pub fn sample(&mut self, sort: ProcessSort) -> SystemSnapshot {
        let now = Instant::now();
        let dt_secs = self
            .prev_sample_time
            .map(|t| now.duration_since(t).as_secs_f64().max(0.05))
            .unwrap_or(1.0);
        self.prev_sample_time = Some(now);

        let mut raw = self.probe.sample_raw(dt_secs);

        if self.cpu_history.len() >= 48 {
            self.cpu_history.pop_front();
        }
        self.cpu_history.push_back(raw.cpu_total_pct);

        sort_processes(&mut raw.processes, sort);

        SystemSnapshot {
            cpu_total_pct: raw.cpu_total_pct,
            cpu_cores_pct: raw.cpu_cores_pct,
            cpu_history: self.cpu_history.iter().copied().collect(),
            load_avg: raw.load_avg,
            uptime_secs: raw.uptime_secs,
            memory: raw.memory,
            disks: raw.disks,
            processes: raw.processes,
            total_processes: raw.total_processes,
            running_processes: raw.running_processes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::domain::{MemoryInfo, ProcessInfo};
    use crate::monitor::ports::RawSystemSample;

    struct FakeProbe {
        cpu: f32,
    }

    impl SystemProbe for FakeProbe {
        fn sample_raw(&mut self, _dt_secs: f64) -> RawSystemSample {
            RawSystemSample {
                cpu_total_pct: self.cpu,
                cpu_cores_pct: vec![self.cpu],
                load_avg: (0.5, 0.4, 0.3),
                uptime_secs: 3600,
                memory: MemoryInfo::default(),
                disks: Vec::new(),
                processes: vec![
                    ProcessInfo {
                        pid: 20,
                        name: "b".into(),
                        state: 'S',
                        cpu_pct: 5.0,
                        mem_bytes: 100,
                        mem_pct: 1.0,
                        threads: 1,
                    },
                    ProcessInfo {
                        pid: 10,
                        name: "a".into(),
                        state: 'R',
                        cpu_pct: 25.0,
                        mem_bytes: 50,
                        mem_pct: 0.5,
                        threads: 2,
                    },
                ],
                total_processes: 2,
                running_processes: 1,
            }
        }
    }

    #[test]
    fn monitor_service_records_history_and_sorts() {
        let mut mon = SystemMonitor::with_probe(Box::new(FakeProbe { cpu: 42.0 }));
        let snap = mon.sample(ProcessSort::Cpu);
        assert_eq!(snap.cpu_history, vec![42.0]);
        assert_eq!(snap.processes[0].pid, 10);

        let snap2 = mon.sample(ProcessSort::Memory);
        assert_eq!(snap2.cpu_history, vec![42.0, 42.0]);
        assert_eq!(snap2.processes[0].pid, 20);
    }
}
