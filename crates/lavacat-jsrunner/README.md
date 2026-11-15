# JsRunner process prototype

An optional API for one child process per page, with DOM+QuickJS+layout together
and immutable shared-memory layout views in the controller. It does not replace
the existing browser window or provide its full paint/input payload yet.

```rust,no_run
use lavacat_jsrunner::RunnerController;
use std::time::Duration;
let mut runner = RunnerController::spawn("target/debug/lavacat-jsrunner-quickjs", Duration::from_secs(5))?;
runner.create_context("<p id='message'>before</p>", "p{line-height:24px}")?;
runner.evaluate("document.getElementById('message').textContent = 'after'")?;
let frame = runner.layout(400.0, 300.0)?;
for node in frame.nodes() {
    println!("{:?}: {}", node.rect(), node.text());
}
# Ok::<(), lavacat_jsrunner::RunnerError>(())
```

Build with `cargo build -p lavacat-jsrunner`. Run native IPC tests with
`cargo test -p lavacat-jsrunner --features process-tests --test process`.
Controller-only builds use `--no-default-features --features controller`.
The executable's stdout is private bootstrap data, not a user CLI.

Recovery/handoff retains DOM checkpoints, **not** JS heap, listener closures or
timers. Scripts are not automatically replayed. Native IPC must be available;
this is trusted-process crash isolation, not an OS security sandbox. Full scope
and remaining issue #4 requirements are in
`../../docs/decisions/2026-11-15-jsrunner-process-prototype.md`.
