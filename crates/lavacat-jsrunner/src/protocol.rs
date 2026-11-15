use ipc_channel::ipc::{IpcReceiver, IpcSender, IpcSharedMemory};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(crate) struct Bootstrap {
    pub commands: IpcReceiver<Request>,
    pub replies: IpcSender<Response>,
}
#[derive(Serialize, Deserialize)]
pub(crate) enum Request {
    Create { html: String, css: String },
    Eval(String),
    Dispatch { node: u64, kind: String },
    Layout { width: f32, height: f32 },
    Export,
    Import(DomSnapshot),
    Suspend,
    Resume,
    Heartbeat,
    Shutdown,
}
#[derive(Serialize, Deserialize)]
pub(crate) enum Response {
    Hello {
        abi: u32,
        engine: String,
        capabilities: Vec<String>,
    },
    Ack,
    Frame(IpcSharedMemory),
    Checkpoint(DomSnapshot),
    Error(String),
}

// A bounded, engine-neutral DOM IR. Arena indices and sibling order are retained,
// including detached nodes and template content fragments. JS heap/listeners and
// timers are deliberately outside this first checkpoint ABI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DomSnapshot {
    pub nodes: Vec<DomNode>,
    pub mode: u8,
    pub css: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DomNode {
    pub kind: u8,
    pub name: String,
    pub namespace: String,
    pub value: String,
    pub extra: String,
    pub attributes: Vec<(String, String, String)>,
    pub children: Vec<u32>,
    pub template: Option<u32>,
}
