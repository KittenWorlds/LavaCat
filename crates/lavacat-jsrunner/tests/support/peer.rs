#[path = "../../src/protocol.rs"]
mod protocol;
use ipc_channel::ipc::IpcOneShotServer;
use protocol::{Bootstrap, Response};
use std::io::Write;
pub fn serve(abi: u32) {
    let (server, name) = IpcOneShotServer::<Bootstrap>::new().unwrap();
    println!("{name}");
    std::io::stdout().flush().unwrap();
    let (_, bootstrap) = server.accept().unwrap();
    bootstrap
        .replies
        .send(Response::Hello {
            abi,
            engine: "test-peer".into(),
            capabilities: vec!["dom-ir-v1".into(), "layout-ir-v1".into()],
        })
        .unwrap();
    // Retain both endpoints but deliberately do not read the command channel.
    // This exercises send backpressure, not merely reply timeout.
    std::thread::park_timeout(std::time::Duration::from_secs(30));
    drop(bootstrap);
}
