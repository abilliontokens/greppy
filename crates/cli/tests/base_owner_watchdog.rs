use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const OWNER_MARKER: &str = "GREPPY_INTERNAL_BASE_BUILD_OWNER_STDIN";
const HOLD_MS: &str = "GREPPY_TEST_BASE_OWNER_HOLD_MS";
const READY: &str = "GREPPY_TEST_BASE_OWNER_READY";

fn greppy() -> Command {
    Command::new(env!("CARGO_BIN_EXE_greppy"))
}

struct ChildGuard(std::process::Child);

impl ChildGuard {
    fn wait_bounded(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "watchdog subprocess did not exit"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[test]
fn held_owner_pipe_allows_completion_and_is_not_inherited() {
    let mut child = ChildGuard(
        greppy()
            .env(OWNER_MARKER, "1")
            .env(HOLD_MS, "25")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let owner = child.0.stdin.take().unwrap();
    let status = child.wait_bounded(Duration::from_secs(5));
    drop(owner);
    assert_eq!(status.code(), Some(0));
}

#[test]
fn closing_owner_pipe_stops_nested_process_with_io_exit() {
    let temp = tempfile::tempdir().unwrap();
    let ready = temp.path().join("ready");
    let mut child = ChildGuard(
        greppy()
            .env(OWNER_MARKER, "1")
            .env(HOLD_MS, "30000")
            .env(READY, &ready)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let owner = child.0.stdin.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.is_file() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.is_file(), "watchdog subprocess did not become ready");
    assert!(child.0.try_wait().unwrap().is_none());

    drop(owner);
    assert_eq!(child.wait_bounded(Duration::from_secs(5)).code(), Some(73));
}

#[test]
fn ordinary_cli_is_not_gated_by_owner_pipe() {
    let status = greppy()
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}
