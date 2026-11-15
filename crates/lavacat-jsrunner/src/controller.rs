use crate::protocol::{Bootstrap, DomSnapshot, Request, Response};
use crate::{ABI_VERSION, LayoutFrame, RunnerError, error};
use ipc_channel::ipc::{self, IpcSender};
use std::{
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// One process per page. The controller exchanges versioned commands and owns
/// immutable recovery checkpoints, never a live Document or engine instance.
///
/// Every request has a finite timeout. A timeout kills/reaps the unresponsive
/// process so a late reply can never be mistaken for the next request's reply.
/// Drop also kills and reaps the process. This API blocks; a UI must call it from
/// an orchestration worker and publish frames back to its event loop.
pub struct RunnerController {
    executable: PathBuf,
    timeout: Duration,
    child: Option<Child>,
    commands: IpcSender<Request>,
    replies: mpsc::Receiver<Response>,
    reader: Option<std::thread::JoinHandle<()>>,
    engine: String,
    checkpoint: Option<DomSnapshot>,
    viewport: Option<[f32; 2]>,
    suspended: bool,
}
impl RunnerController {
    /// Spawn one engine distribution. The executable selects the engine; it
    /// cannot be changed inside an instance. ABI is checked before creating DOM.
    pub fn spawn(executable: impl AsRef<Path>, timeout: Duration) -> Result<Self, RunnerError> {
        if timeout.is_zero() || timeout > Duration::from_secs(60) {
            return Err(error("timeout must be in (0, 60s]"));
        }
        let (commands, commands_rx) = ipc::channel::<Request>().map_err(error)?;
        let (replies_tx, replies_rx) = ipc::channel::<Response>().map_err(error)?;
        let mut child = Command::new(executable.as_ref())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(error)?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| error("runner stdout unavailable"))?;
        let (name_tx, name_rx) = mpsc::channel();
        let bootstrap_reader = std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout.take(1025))
                .read_line(&mut line)
                .map(|_| line);
            let _ = name_tx.send(result);
        });
        let name = match name_rx.recv_timeout(timeout) {
            Ok(Ok(name))
                if !name.trim().is_empty() && name.len() <= 1024 && !name.contains('\0') =>
            {
                name
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                // A descendant may retain stdout; do not join an unbounded read.
                drop(bootstrap_reader);
                return Err(error("runner bootstrap failed or timed out"));
            }
        };
        let _ = bootstrap_reader.join();
        let (reply_tx, replies) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            while let Ok(reply) = replies_rx.recv() {
                if reply_tx.send(reply).is_err() {
                    break;
                }
            }
        });
        let mut controller = Self {
            executable: executable.as_ref().to_owned(),
            timeout,
            child: Some(child),
            commands,
            replies,
            reader: Some(reader),
            engine: String::new(),
            checkpoint: None,
            viewport: None,
            suspended: false,
        };
        let remaining = controller.send_with_deadline(move || {
            let bootstrap = IpcSender::<Bootstrap>::connect(name.trim().to_owned())
                .map_err(|e| e.to_string())?;
            bootstrap
                .send(Bootstrap {
                    commands: commands_rx,
                    replies: replies_tx,
                })
                .map_err(|e| e.to_string())
        })?;
        match controller.receive(remaining)? {
            Response::Hello {
                abi,
                engine,
                capabilities,
            } if abi == ABI_VERSION
                && !engine.is_empty()
                && capabilities.iter().any(|s| s == "dom-ir-v1")
                && capabilities.iter().any(|s| s == "layout-ir-v1") =>
            {
                controller.engine = engine
            }
            _ => return Err(error("unsupported runner ABI/hello")),
        }
        Ok(controller)
    }
    /// Distribution engine identifier, validated by the initial handshake.
    pub fn engine(&self) -> &str {
        &self.engine
    }
    /// Operating-system process ID for health diagnostics.
    pub fn process_id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
    /// Terminate and reap an instance. The last committed checkpoint is retained
    /// for explicit recovery; no live DOM is accessed during termination.
    pub fn terminate(&mut self) {
        self.stop();
    }
    /// Probe health and restore the last DOM checkpoint when the process died or
    /// exceeded its deadline. Returns true if a replacement process was started.
    pub fn recover_if_unhealthy(&mut self) -> Result<bool, RunnerError> {
        if self.heartbeat().is_ok() {
            return Ok(false);
        }
        self.recover()?;
        Ok(true)
    }
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    fn receive(&mut self, remaining: Duration) -> Result<Response, RunnerError> {
        match self.replies.recv_timeout(remaining) {
            Ok(Response::Error(message)) => Err(error(message)),
            Ok(response) => Ok(response),
            Err(e) => {
                self.stop();
                Err(error(format!("runner disconnected or timed out: {e}")))
            }
        }
    }
    fn request(&mut self, request: Request) -> Result<Response, RunnerError> {
        if self.child.is_none() {
            return Err(error("runner is not running; recover from a checkpoint"));
        }
        let sender = self.commands.clone();
        let remaining =
            self.send_with_deadline(move || sender.send(request).map_err(|e| e.to_string()))?;
        self.receive(remaining)
    }
    // IPC connect/send can block too. Keep them outside the controlling thread
    // so the same deadline covers backpressure as well as the response wait.
    fn send_with_deadline(
        &mut self,
        operation: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) -> Result<Duration, RunnerError> {
        let started = Instant::now();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
                .unwrap_or_else(|_| Err("runner IPC operation panicked".into()));
            let _ = tx.send(result);
        });
        match rx.recv_timeout(self.timeout) {
            Ok(Ok(())) => Ok(self.timeout.saturating_sub(started.elapsed())),
            Ok(Err(e)) => {
                self.stop();
                Err(error(e))
            }
            Err(e) => {
                self.stop();
                Err(error(format!(
                    "runner IPC send timed out/disconnected: {e}"
                )))
            }
        }
    }

    fn ack(&mut self, request: Request) -> Result<(), RunnerError> {
        match self.request(request)? {
            Response::Ack => Ok(()),
            _ => {
                self.stop();
                Err(error("unexpected runner reply"))
            }
        }
    }
    /// Replace the page context. Scripts are evaluated explicitly, so recovering
    /// a checkpoint never repeats network/user side effects by replaying scripts.
    pub fn create_context(&mut self, html: &str, css: &str) -> Result<(), RunnerError> {
        if html.len() > 4 * 1024 * 1024 || css.len() > 1024 * 1024 {
            return Err(error("context input exceeds budget"));
        }
        self.ack(Request::Create {
            html: html.into(),
            css: css.into(),
        })?;
        self.checkpoint = None;
        self.viewport = None;
        self.checkpoint()
    }
    /// Evaluate in the child, pump microtasks, and checkpoint the resulting DOM.
    /// A script exception is reported without killing a healthy instance.
    pub fn evaluate(&mut self, source: &str) -> Result<(), RunnerError> {
        if source.len() > 1024 * 1024 {
            return Err(error("script exceeds budget"));
        }
        self.ack(Request::Eval(source.into()))?;
        self.checkpoint()
    }
    /// Dispatch a basic bubbling/cancelable Event to a page-local node identity.
    /// Typed pointer/key payloads and renderer hit testing are not yet integrated.
    pub fn dispatch(&mut self, node: u64, kind: &str) -> Result<(), RunnerError> {
        if kind.len() > 256 {
            return Err(error("event type exceeds budget"));
        }
        self.ack(Request::Dispatch {
            node,
            kind: kind.into(),
        })?;
        self.checkpoint()
    }
    /// Generate a complete immutable layout frame; the previous frame remains
    /// valid until its last owner drops it (no shared mutable frame slots).
    pub fn layout(&mut self, width: f32, height: f32) -> Result<LayoutFrame, RunnerError> {
        match self.request(Request::Layout { width, height })? {
            Response::Frame(memory) => {
                let frame = LayoutFrame::new(memory)?;
                self.viewport = Some([width, height]);
                Ok(frame)
            }
            _ => Err(error("expected layout frame")),
        }
    }
    /// Update the engine-neutral recovery checkpoint; JS heap/closures/timers are
    /// excluded from ABI 1 and must be rebuilt explicitly by the caller.
    pub fn checkpoint(&mut self) -> Result<(), RunnerError> {
        match self.request(Request::Export)? {
            Response::Checkpoint(snapshot) => {
                self.checkpoint = Some(snapshot);
                Ok(())
            }
            _ => Err(error("expected DOM checkpoint")),
        }
    }
    /// Suspend script/event execution; heartbeat, layout and export stay usable.
    pub fn suspend(&mut self) -> Result<(), RunnerError> {
        self.ack(Request::Suspend)?;
        self.suspended = true;
        Ok(())
    }
    /// Resume a suspended instance.
    pub fn resume(&mut self) -> Result<(), RunnerError> {
        self.ack(Request::Resume)?;
        self.suspended = false;
        Ok(())
    }
    /// Explicit health probe. An unresponsive child is terminated on timeout.
    pub fn heartbeat(&mut self) -> Result<(), RunnerError> {
        self.ack(Request::Heartbeat)
    }
    /// Restore the most recent committed DOM checkpoint in a fresh process.
    /// This does not restore JS execution state or replay earlier scripts.
    pub fn recover(&mut self) -> Result<(), RunnerError> {
        let snapshot = self
            .checkpoint
            .clone()
            .ok_or_else(|| error("no recovery checkpoint"))?;
        let mut replacement = Self::spawn(&self.executable, self.timeout)?;
        replacement.ack(Request::Import(snapshot.clone()))?;
        if let Some([width, height]) = self.viewport {
            replacement.layout(width, height)?;
        }
        if self.suspended {
            replacement.suspend()?;
        }
        replacement.checkpoint = Some(snapshot);
        std::mem::swap(self, &mut replacement);
        Ok(())
    }
    /// Transactional engine-distribution handoff: freeze/export the old context,
    /// import and lay out in a candidate, then commit. Failure resumes the source.
    /// Old-process teardown happens after candidate readiness, not a fixed delay.
    /// The execution-state limitations of `recover` apply here too.
    pub fn switch_distribution(&mut self, executable: impl AsRef<Path>) -> Result<(), RunnerError> {
        let was_suspended = self.suspended;
        if !was_suspended {
            self.suspend()?;
        }
        let result = (|| {
            self.checkpoint()?;
            let snapshot = self
                .checkpoint
                .clone()
                .ok_or_else(|| error("no handoff snapshot"))?;
            let mut candidate = Self::spawn(executable, self.timeout)?;
            candidate.ack(Request::Import(snapshot.clone()))?;
            if let Some([width, height]) = self.viewport {
                candidate.layout(width, height)?;
            }
            if was_suspended {
                candidate.suspend()?;
            }
            candidate.checkpoint = Some(snapshot);
            Ok(candidate)
        })();
        match result {
            Ok(mut candidate) => {
                std::mem::swap(self, &mut candidate);
                Ok(())
            }
            Err(e) => {
                if !was_suspended {
                    let _ = self.resume();
                }
                // Keep the desired state even when rollback cannot contact a
                // source that died during checkpoint export.
                self.suspended = was_suspended;
                Err(e)
            }
        }
    }
}
impl Drop for RunnerController {
    fn drop(&mut self) {
        self.stop();
        // Dropping the join handle is deliberate: a failed bootstrap may still
        // own the parent's reply sender while unwinding. EOF releases this
        // reader as soon as that sender drops; joining here would deadlock.
        self.reader.take();
    }
}
