//! The process-global logger is installed only once. Run each scenario in a
//! fresh test process so unrelated tests cannot install it first. These checks
//! call the actual package setup, which builds options and reads metadata;
//! they never enter the GUI runner or start an engine.

use std::{
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

const CHILD_TEST: &str = "NSH_RUNNER_LOGGER_TEST";
const TARGET: &str = "nsh-runner-logger-test";

fn in_fresh_process(test: &str) -> bool {
    if std::env::var_os(CHILD_TEST).as_deref() == Some(std::ffi::OsStr::new(test)) {
        return true;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env(CHILD_TEST, test)
        .env_remove("NSH_APP_DIR")
        .output()
        .expect("start the isolated runner setup test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "isolated {test} failed: {stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("1 passed"),
        "the child must execute the exact test, not an empty filter: {stdout}\n{stderr}"
    );
    false
}

struct Capture(AtomicUsize);

impl log::Log for Capture {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target() == TARGET
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) && record.level() == log::Level::Trace {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn flush(&self) {}
}

#[test]
fn setup_preserves_an_application_logger_and_its_trace_level() {
    if !in_fresh_process("tests::setup_preserves_an_application_logger_and_its_trace_level") {
        return;
    }
    static CAPTURE: Capture = Capture(AtomicUsize::new(0));
    log::set_logger(&CAPTURE).expect("the child has no previously installed logger");
    log::set_max_level(log::LevelFilter::Trace);
    super::setup().expect("Linux package setup succeeds without a display");
    assert_eq!(log::max_level(), log::LevelFilter::Trace);
    log::trace!(target: TARGET, "the application logger still receives trace records");
    assert_eq!(CAPTURE.0.load(Ordering::Relaxed), 1);
}

#[test]
fn setup_installs_the_default_logger_when_the_application_has_none() {
    if !in_fresh_process("tests::setup_installs_the_default_logger_when_the_application_has_none") {
        return;
    }
    assert_eq!(log::max_level(), log::LevelFilter::Off);
    super::setup().expect("Linux package setup succeeds without a display");
    assert_eq!(log::max_level(), log::LevelFilter::Info);
    assert!(log::log_enabled!(target: TARGET, log::Level::Warn));
}
