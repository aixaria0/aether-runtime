use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub const CPP_EXECUTOR: &str = "cpp-grpc-v1";
pub const RUST_EXECUTOR: &str = "rust-builtin-v1";
const QUARANTINE_AFTER: u32 = 3;
const QUARANTINE_NS: u64 = 30_000_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutorStats {
    pub executor_id: String,
    pub successes: u64,
    pub failures: u64,
    pub consecutive_failures: u32,
    pub ewma_latency_ms: f64,
    pub quarantined_until_ns: u64,
}

impl ExecutorStats {
    pub fn new(id: &str) -> Self {
        Self {
            executor_id: id.to_string(),
            successes: 0,
            failures: 0,
            consecutive_failures: 0,
            ewma_latency_ms: 0.0,
            quarantined_until_ns: 0,
        }
    }

    pub fn available(&self, now_ns: u64) -> bool {
        self.quarantined_until_ns <= now_ns
    }

    pub fn score(&self, now_ns: u64) -> f64 {
        if !self.available(now_ns) {
            return f64::NEG_INFINITY;
        }
        let total = self.successes + self.failures;
        let reliability = (self.successes as f64 + 2.0) / (total as f64 + 4.0);
        let latency_penalty = if self.ewma_latency_ms <= 0.0 {
            0.0
        } else {
            (self.ewma_latency_ms / 100.0).min(0.35)
        };
        reliability - latency_penalty - (self.consecutive_failures as f64 * 0.08)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SchedulerSnapshot {
    pub selected: Option<String>,
    pub executors: Vec<ExecutorStats>,
}

#[derive(Clone, Debug)]
pub struct AdaptiveScheduler {
    stats: BTreeMap<String, ExecutorStats>,
}

impl AdaptiveScheduler {
    pub fn new(seed: Vec<ExecutorStats>) -> Self {
        let mut stats = BTreeMap::new();
        for id in [CPP_EXECUTOR, RUST_EXECUTOR] {
            stats.insert(id.to_string(), ExecutorStats::new(id));
        }
        for entry in seed {
            stats.insert(entry.executor_id.clone(), entry);
        }
        Self { stats }
    }

    pub fn select(&self, exclude: &[&str]) -> Option<String> {
        let now = now_ns();
        self.stats
            .values()
            .filter(|s| !exclude.iter().any(|id| *id == s.executor_id))
            .max_by(|a, b| {
                a.score(now)
                    .total_cmp(&b.score(now))
                    .then_with(|| primary_bias(a).cmp(&primary_bias(b)))
            })
            .filter(|s| s.available(now))
            .map(|s| s.executor_id.clone())
    }

    pub fn observe(&mut self, executor_id: &str, success: bool, elapsed_ms: u64) -> ExecutorStats {
        let entry = self
            .stats
            .entry(executor_id.to_string())
            .or_insert_with(|| ExecutorStats::new(executor_id));
        let sample = elapsed_ms as f64;
        entry.ewma_latency_ms = if entry.ewma_latency_ms == 0.0 {
            sample
        } else {
            0.8 * entry.ewma_latency_ms + 0.2 * sample
        };
        if success {
            entry.successes += 1;
            entry.consecutive_failures = 0;
            entry.quarantined_until_ns = 0;
        } else {
            entry.failures += 1;
            entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
            if entry.consecutive_failures >= QUARANTINE_AFTER {
                entry.quarantined_until_ns = now_ns().saturating_add(QUARANTINE_NS);
            }
        }
        entry.clone()
    }

    pub fn snapshot(&self) -> SchedulerSnapshot {
        let selected = self.select(&[]);
        SchedulerSnapshot {
            selected,
            executors: self.stats.values().cloned().collect(),
        }
    }
}

fn primary_bias(stats: &ExecutorStats) -> u8 {
    u8::from(stats.executor_id == CPP_EXECUTOR)
}

pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_start_prefers_cpp_primary() {
        let s = AdaptiveScheduler::new(vec![]);
        assert_eq!(s.select(&[]).as_deref(), Some(CPP_EXECUTOR));
    }

    #[test]
    fn scheduler_prefers_observed_reliable_executor() {
        let mut s = AdaptiveScheduler::new(vec![]);
        for _ in 0..5 {
            s.observe(CPP_EXECUTOR, false, 10);
            s.observe(RUST_EXECUTOR, true, 3);
        }
        assert_eq!(s.select(&[]).as_deref(), Some(RUST_EXECUTOR));
    }

    #[test]
    fn repeated_failures_quarantine_executor() {
        let mut s = AdaptiveScheduler::new(vec![]);
        for _ in 0..QUARANTINE_AFTER {
            s.observe(CPP_EXECUTOR, false, 1);
        }
        assert!(!s.stats.get(CPP_EXECUTOR).unwrap().available(now_ns()));
        assert_eq!(s.select(&[]).as_deref(), Some(RUST_EXECUTOR));
    }
}
