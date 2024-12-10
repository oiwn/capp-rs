use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct SharedStats<'a> {
    workers_stats: Vec<&'a WorkerStats>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WorkerStats {
    pub total_execution_time: std::time::Duration,
    pub tasks_processed: usize,
    pub tasks_succeeded: usize,
    pub tasks_failed: usize,
}

impl Default for WorkerStats {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerStats {
    pub fn new() -> Self {
        Self {
            total_execution_time: std::time::Duration::new(0, 0),
            tasks_processed: 0,
            tasks_succeeded: 0,
            tasks_failed: 0,
        }
    }

    pub fn record_execution_time(&mut self, duration: std::time::Duration) {
        self.total_execution_time += duration;
        self.tasks_processed += 1;
    }

    pub fn record_success(&mut self) {
        self.tasks_succeeded += 1;
    }

    pub fn record_failure(&mut self) {
        self.tasks_failed += 1;
    }

    pub fn average_execution_time(&self) -> std::time::Duration {
        if self.tasks_processed == 0 {
            return std::time::Duration::new(0, 0);
        }
        self.total_execution_time / self.tasks_processed as u32
    }
}

impl<'a> SharedStats<'a> {
    pub fn new() -> Self {
        Self {
            workers_stats: Vec::new(),
        }
    }

    pub fn add_worker_stats(&mut self, stats: &'a WorkerStats) {
        self.workers_stats.push(stats);
    }
}

#[allow(clippy::needless_lifetimes)]
impl<'a> Default for SharedStats<'a> {
    fn default() -> Self {
        Self::new()
    }
}
