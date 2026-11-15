#![cfg(feature = "process-tests")]
use lavacat_jsrunner::RunnerController;
use std::time::Duration;
fn executable() -> &'static str {
    env!("CARGO_BIN_EXE_lavacat-jsrunner-quickjs")
}
fn controller() -> RunnerController {
    RunnerController::spawn(executable(), Duration::from_secs(5)).unwrap()
}
fn text(frame: &lavacat_jsrunner::LayoutFrame) -> String {
    frame
        .nodes()
        .map(|node| node.text().to_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn isolated_dom_script_layout_event_and_immutable_frames() {
    let mut runner = controller();
    assert_eq!(runner.engine(), "quickjs-ng");
    assert_ne!(runner.process_id().unwrap(), std::process::id());
    runner
        .create_context("<p id='message'>before</p>", "p{line-height:24px}")
        .unwrap();
    let before = runner.layout(400.0, 300.0).unwrap();
    assert!(text(&before).contains("before"));
    runner.evaluate("document.getElementById('message').textContent = 'after'; document.getElementById('message').addEventListener('click', function(){ document.getElementById('message').textContent = 'clicked'; });").unwrap();
    let after = runner.layout(400.0, 300.0).unwrap();
    assert!(after.sequence() > before.sequence());
    assert_eq!(after.viewport(), [400.0, 300.0]);
    assert!(text(&after).contains("after"));
    let target = after
        .nodes()
        .find(|node| node.text().contains("after"))
        .and_then(|node| node.identity())
        .unwrap();
    runner.dispatch(target, "click").unwrap();
    let clicked = runner.layout(400.0, 300.0).unwrap();
    assert!(text(&clicked).contains("clicked"));
    assert!(
        text(&before).contains("before"),
        "old shared frame was overwritten"
    );
    runner.terminate();
    assert!(
        text(&after).contains("after"),
        "frame must survive process teardown"
    );
}

#[test]
fn crash_recovery_preserves_dom_without_replaying_scripts() {
    let mut runner = controller();
    runner
        .create_context("<p id='message'>before</p>", "")
        .unwrap();
    runner.evaluate("document.getElementById('message').textContent = 'checkpoint'; globalThis.heapOnly = 42;").unwrap();
    runner.layout(400.0, 300.0).unwrap();
    let old = runner.process_id();
    runner.terminate();
    assert!(runner.recover_if_unhealthy().unwrap());
    assert_ne!(runner.process_id(), old);
    assert!(text(&runner.layout(400.0, 300.0).unwrap()).contains("checkpoint"));
    runner.evaluate("if (typeof heapOnly !== 'undefined') throw new Error('heap state must not be invented');").unwrap();
    assert!(!runner.recover_if_unhealthy().unwrap());
}

#[test]
fn failed_handoff_rolls_back_and_valid_handoff_commits_after_readiness() {
    let mut runner = controller();
    runner
        .create_context("<p id='message'>source</p>", "")
        .unwrap();
    runner.layout(400.0, 300.0).unwrap();
    let old = runner.process_id();
    assert!(
        runner
            .switch_distribution("/nonexistent/lavacat-runner")
            .is_err()
    );
    assert_eq!(runner.process_id(), old);
    runner
        .evaluate("document.getElementById('message').textContent = 'resumed';")
        .unwrap();
    runner.switch_distribution(executable()).unwrap();
    assert_ne!(runner.process_id(), old);
    assert!(text(&runner.layout(400.0, 300.0).unwrap()).contains("resumed"));
}

#[test]
fn suspension_script_errors_and_invalid_viewports_leave_channel_synchronized() {
    let mut runner = controller();
    runner.create_context("<p>healthy</p>", "").unwrap();
    runner.suspend().unwrap();
    assert!(runner.evaluate("throw new Error('must not run')").is_err());
    runner.heartbeat().unwrap();
    runner.resume().unwrap();
    assert!(runner.evaluate("throw new Error('expected')").is_err());
    assert!(runner.layout(f32::NAN, 300.0).is_err());
    assert!(runner.layout(0.0, 300.0).is_err());
    assert!(text(&runner.layout(400.0, 300.0).unwrap()).contains("healthy"));
}

#[test]
fn hung_script_hits_deadline_and_recovers_last_committed_dom() {
    let mut runner = RunnerController::spawn(executable(), Duration::from_secs(2)).unwrap();
    runner.create_context("<p>safe checkpoint</p>", "").unwrap();
    assert!(runner.evaluate("while(true) {}").is_err());
    assert!(runner.process_id().is_none());
    runner.recover().unwrap();
    assert!(text(&runner.layout(400.0, 300.0).unwrap()).contains("safe checkpoint"));
}

#[test]
fn unsupported_abi_is_rejected_before_context_creation() {
    assert!(
        RunnerController::spawn(
            env!("CARGO_BIN_EXE_lavacat-jsrunner-bad-abi"),
            Duration::from_secs(3)
        )
        .is_err()
    );
}

#[test]
fn stalled_receiver_cannot_block_a_large_control_message_past_deadline() {
    let mut runner = RunnerController::spawn(
        env!("CARGO_BIN_EXE_lavacat-jsrunner-stalled-peer"),
        Duration::from_secs(2),
    )
    .unwrap();
    let start = std::time::Instant::now();
    assert!(
        runner
            .create_context(&"x".repeat(4 * 1024 * 1024), "")
            .is_err()
    );
    assert!(start.elapsed() < Duration::from_secs(8));
    assert!(runner.process_id().is_none());
}

#[test]
fn handoff_preserves_an_explicitly_suspended_context() {
    let mut runner = controller();
    runner.create_context("<p>suspended</p>", "").unwrap();
    runner.suspend().unwrap();
    assert!(runner.switch_distribution("/nonexistent/runner").is_err());
    assert!(runner.evaluate("1").is_err());
    runner.switch_distribution(executable()).unwrap();
    assert!(runner.evaluate("1").is_err());
    runner.resume().unwrap();
    runner.evaluate("1").unwrap();
}
