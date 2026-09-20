//! Compact progress for commands that can wait on index work.
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const INITIAL_DELAY: Duration = Duration::from_secs(2);
const STALL_AFTER: Duration = Duration::from_secs(60);

pub(crate) struct QueryProgress {
    stop: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl QueryProgress {
    fn start(interval: Duration, mut report: impl FnMut(Duration) + Send + 'static) -> Self {
        let (stop, receiver) = mpsc::channel();
        let started = Instant::now();
        let thread = std::thread::Builder::new()
            .name("greppy-query-progress".into())
            .spawn(move || {
                while let Err(RecvTimeoutError::Timeout) = receiver.recv_timeout(interval) {
                    report(started.elapsed());
                }
            })
            .ok();
        Self { stop, thread }
    }
}

impl Drop for QueryProgress {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct JobProgress {
    state: String,
    completed: u64,
    total: u64,
    unit: String,
    pid: Option<u64>,
}

impl JobProgress {
    fn read(path: &std::path::Path) -> Option<Self> {
        let value = crate::read_background_job(path)?;
        Some(Self {
            state: value.get("state")?.as_str()?.to_owned(),
            completed: value
                .get("completed_spans")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            total: value
                .get("total_spans")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            unit: value
                .get("progress_unit")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("items")
                .to_owned(),
            pid: value.get("pid").and_then(serde_json::Value::as_u64),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prognosis {
    Seconds(u64),
    Minutes(u64),
    Complete,
}

impl Prognosis {
    fn from_measurement(remaining: u64, completed: u64, elapsed: Duration) -> Option<Self> {
        if remaining == 0 {
            return Some(Self::Complete);
        }
        if completed == 0 || elapsed.is_zero() {
            return None;
        }
        let seconds = remaining
            .saturating_mul(elapsed.as_secs().max(1))
            .div_ceil(completed);
        if seconds <= 120 {
            Some(Self::Seconds(round_up(seconds, 10)))
        } else {
            Some(Self::Minutes(round_up(seconds.div_ceil(60), 1)))
        }
    }

    fn label(self) -> String {
        match self {
            Self::Seconds(seconds) => format!("about {seconds}s"),
            Self::Minutes(minutes) => format!("about {minutes}m"),
            Self::Complete => "complete".into(),
        }
    }
}

fn round_up(value: u64, quantum: u64) -> u64 {
    value.div_ceil(quantum) * quantum
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReportKey {
    state: Option<String>,
    prognosis: Option<Prognosis>,
    stalled: bool,
}

#[derive(Default)]
struct ProgressReporter {
    phase: Option<String>,
    phase_started_at: Duration,
    phase_started_completed: u64,
    last_completed: u64,
    last_progress_at: Duration,
    prognosis: Option<Prognosis>,
    last_report: Option<ReportKey>,
}

impl ProgressReporter {
    fn observe(
        &mut self,
        command: &str,
        job: Option<JobProgress>,
        elapsed: Duration,
    ) -> Option<String> {
        let Some(job) = job else {
            let key = ReportKey {
                state: None,
                prognosis: None,
                stalled: false,
            };
            if self.last_report.as_ref() == Some(&key) {
                return None;
            }
            self.last_report = Some(key);
            return Some(format!(
                "greppy: {command} still running; waiting for index progress"
            ));
        };

        let phase_changed = self.phase.as_deref() != Some(job.state.as_str());
        if phase_changed {
            self.phase = Some(job.state.clone());
            self.phase_started_at = elapsed;
            self.phase_started_completed = job.completed;
            self.last_completed = job.completed;
            self.last_progress_at = elapsed;
            self.prognosis =
                (job.total > 0 && job.completed >= job.total).then_some(Prognosis::Complete);
        } else if job.completed != self.last_completed {
            self.last_completed = job.completed;
            self.last_progress_at = elapsed;
            self.prognosis = job.total.checked_sub(job.completed).and_then(|remaining| {
                Prognosis::from_measurement(
                    remaining,
                    job.completed.saturating_sub(self.phase_started_completed),
                    elapsed.saturating_sub(self.phase_started_at),
                )
            });
        }

        let stalled = job.completed < job.total
            && elapsed.saturating_sub(self.last_progress_at) >= STALL_AFTER;
        let key = ReportKey {
            state: Some(job.state.clone()),
            prognosis: self.prognosis,
            stalled,
        };
        if !phase_changed && self.last_report.as_ref() == Some(&key) {
            return None;
        }
        self.last_report = Some(key);

        let progress = if job.total > 0 {
            format!("{}/{} {}", job.completed, job.total, job.unit)
        } else {
            format!("{} {}", job.completed, job.unit)
        };
        let prognosis = if stalled {
            format!("no progress reported for {}s", STALL_AFTER.as_secs())
        } else if let Some(prognosis) = self.prognosis {
            format!("phase ETA {}", prognosis.label())
        } else {
            "measuring phase ETA".into()
        };
        let pid = job
            .pid
            .map(|pid| format!("; pid {pid}"))
            .unwrap_or_default();
        Some(format!(
            "greppy: {command} — {}: {progress}; {prognosis}{pid}",
            job.state
        ))
    }
}

pub(crate) fn for_command(
    command: Option<&crate::Command>,
    effective_root: Option<PathBuf>,
) -> Option<QueryProgress> {
    use crate::Command;
    let name = match command? {
        Command::Index { .. } => "index",
        Command::SearchGraph { .. } => "search-graph",
        Command::SearchSymbol { .. } => "search-symbol",
        Command::SearchPattern { .. } => "search-pattern",
        Command::Search { .. } => "search",
        Command::Context { .. } => "context",
        Command::WhoCalls { .. } => "who-calls",
        Command::Callees { .. } => "callees",
        Command::Impact { .. } => "impact",
        Command::Path { .. } => "path",
        Command::Brief { .. } => "brief",
        Command::Trace { .. } => "trace",
        Command::Read { .. } => "read",
        Command::ReadSmart { .. } => "read-smart",
        Command::WhereAmI { .. } => "where-am-i",
        _ => return None,
    };
    let job_path = effective_root.map(|root| crate::background_job_path(&root));
    let mut reporter = ProgressReporter::default();
    Some(QueryProgress::start(INITIAL_DELAY, move |elapsed| {
        let job = job_path.as_deref().and_then(JobProgress::read);
        if let Some(line) = reporter.observe(name, job, elapsed) {
            eprintln!("{line}");
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(state: &str, completed: u64, total: u64) -> JobProgress {
        JobProgress {
            state: state.into(),
            completed,
            total,
            unit: "spans".into(),
            pid: Some(42),
        }
    }

    #[test]
    fn phase_eta_uses_only_progress_measured_in_that_phase() {
        let mut reporter = ProgressReporter::default();
        assert!(reporter
            .observe(
                "search",
                Some(job("embedding", 400, 1000)),
                Duration::from_secs(2)
            )
            .unwrap()
            .contains("measuring phase ETA"));

        let update = reporter
            .observe(
                "search",
                Some(job("embedding", 500, 1000)),
                Duration::from_secs(12),
            )
            .unwrap();
        assert!(update.contains("phase ETA about 50s"), "{update}");

        let new_phase = reporter
            .observe(
                "search",
                Some(job("writing", 500, 1000)),
                Duration::from_secs(13),
            )
            .unwrap();
        assert!(new_phase.contains("measuring phase ETA"), "{new_phase}");
    }

    #[test]
    fn unchanged_progress_is_suppressed_until_prognosis_or_stall_changes() {
        let mut reporter = ProgressReporter::default();
        assert!(reporter
            .observe(
                "search",
                Some(job("indexing", 0, 100)),
                Duration::from_secs(2)
            )
            .is_some());
        assert!(reporter
            .observe(
                "search",
                Some(job("indexing", 0, 100)),
                Duration::from_secs(30)
            )
            .is_none());

        let stalled = reporter
            .observe(
                "search",
                Some(job("indexing", 0, 100)),
                Duration::from_secs(62),
            )
            .unwrap();
        assert!(
            stalled.contains("no progress reported for 60s"),
            "{stalled}"
        );
        assert!(reporter
            .observe(
                "search",
                Some(job("indexing", 0, 100)),
                Duration::from_secs(120)
            )
            .is_none());

        let resumed = reporter
            .observe(
                "search",
                Some(job("indexing", 10, 100)),
                Duration::from_secs(122),
            )
            .unwrap();
        assert!(resumed.contains("phase ETA"), "{resumed}");
    }

    #[test]
    fn missing_job_status_is_emitted_once() {
        let mut reporter = ProgressReporter::default();
        assert!(reporter.observe("search", None, Duration::ZERO).is_some());
        assert!(reporter
            .observe("search", None, Duration::from_secs(30))
            .is_none());
    }
}
