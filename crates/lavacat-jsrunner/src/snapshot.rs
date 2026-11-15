#![cfg_attr(
    not(all(feature = "quickjs", feature = "controller")),
    allow(dead_code)
)]
use crate::{MAX_SNAPSHOT_BYTES, RunnerError, error};
use ipc_channel::ipc::IpcSharedMemory;
use rkyv::{Archive, Serialize};

#[derive(Archive, Serialize)]
pub(crate) struct LayoutSnapshot {
    pub abi: u32,
    pub sequence: u64,
    pub width: f32,
    pub height: f32,
    pub nodes: Vec<LayoutNode>,
}
#[derive(Archive, Serialize)]
pub(crate) struct LayoutNode {
    pub node: u64,
    pub parent: u32,
    pub rect: [f32; 4],
    pub text: String,
}

/// Owns one immutable, refcounted shared-memory frame. It can outlive the runner;
/// retained frames are never rewritten. No DOM or engine types cross this API.
pub struct LayoutFrame {
    memory: IpcSharedMemory,
}
impl std::fmt::Debug for LayoutFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutFrame")
            .field("bytes", &self.memory.len())
            .finish()
    }
}
impl LayoutFrame {
    pub(crate) fn new(memory: IpcSharedMemory) -> Result<Self, RunnerError> {
        if memory.len() > MAX_SNAPSHOT_BYTES {
            return Err(error("layout frame exceeds budget"));
        }
        let frame = Self { memory };
        let view = frame.view()?;
        if !view.width.to_native().is_finite()
            || !view.height.to_native().is_finite()
            || !(1.0..=16384.0).contains(&view.width.to_native())
            || !(1.0..=16384.0).contains(&view.height.to_native())
        {
            return Err(error("invalid frame viewport"));
        }
        if view.abi.to_native() != crate::ABI_VERSION
            || view.nodes.is_empty()
            || view.nodes.len() > 100_000
        {
            return Err(error("invalid layout ABI or node count"));
        }
        for (index, node) in view.nodes.iter().enumerate() {
            let parent = node.parent.to_native();
            if (parent == u32::MAX) != (index == 0)
                || (parent != u32::MAX && parent as usize >= index)
            {
                return Err(error("invalid layout parent"));
            }
            if node.rect.iter().any(|x| !x.to_native().is_finite())
                || node.rect[2].to_native() < 0.0
                || node.rect[3].to_native() < 0.0
            {
                return Err(error("invalid layout geometry"));
            }
        }
        Ok(frame)
    }
    fn view(&self) -> Result<&ArchivedLayoutSnapshot, RunnerError> {
        rkyv::access::<ArchivedLayoutSnapshot, rkyv::rancor::Error>(&self.memory).map_err(error)
    }
    /// Sequence assigned only after the runner completed layout.
    pub fn sequence(&self) -> u64 {
        self.view().expect("validated frame").sequence.to_native()
    }
    /// Physical CSS-pixel viewport used for this frame.
    pub fn viewport(&self) -> [f32; 2] {
        let v = self.view().expect("validated frame");
        [v.width.to_native(), v.height.to_native()]
    }
    /// Number of flat layout nodes, read directly from shared memory.
    pub fn node_count(&self) -> usize {
        self.view().expect("validated frame").nodes.len()
    }
    /// Iterate in-place node views with one archive validation for the entire
    /// traversal; no tree allocation or per-node validation scan is performed.
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = LayoutNodeView<'_>> {
        self.view()
            .expect("validated frame")
            .nodes
            .iter()
            .map(|node| LayoutNodeView { node })
    }
    /// Borrow one flat layout node without deserialization or rebuilding a tree.
    pub fn node(&self, index: usize) -> Option<LayoutNodeView<'_>> {
        self.view()
            .ok()?
            .nodes
            .get(index)
            .map(|node| LayoutNodeView { node })
    }
}

/// Borrowed view into a validated shared-memory node (CSS box geometry only).
pub struct LayoutNodeView<'a> {
    node: &'a ArchivedLayoutNode,
}
impl LayoutNodeView<'_> {
    /// Stable page-local DOM identity, or None for anonymous layout boxes.
    pub fn identity(&self) -> Option<u64> {
        match self.node.node.to_native() {
            u64::MAX => None,
            id => Some(id),
        }
    }
    /// Parent flat-array index; None denotes the root.
    pub fn parent(&self) -> Option<usize> {
        match self.node.parent.to_native() {
            u32::MAX => None,
            index => Some(index as usize),
        }
    }
    /// Absolute border-box x/y/width/height, CSS px (CSS 2.2 §8.1).
    pub fn rect(&self) -> [f32; 4] {
        self.node.rect.map(|v| v.to_native())
    }
    /// Visible text represented by this box's line runs, borrowed in place.
    pub fn text(&self) -> &str {
        self.node.text.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_archives_and_invalid_parent_are_rejected() {
        assert!(LayoutFrame::new(IpcSharedMemory::from_bytes(&[0; 12])).is_err());
        let snapshot = LayoutSnapshot {
            abi: crate::ABI_VERSION,
            sequence: 1,
            width: 10.0,
            height: 10.0,
            nodes: vec![LayoutNode {
                node: 0,
                parent: 0,
                rect: [0.0; 4],
                text: "bad".into(),
            }],
        };
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&snapshot).unwrap();
        assert!(LayoutFrame::new(IpcSharedMemory::from_bytes(&bytes)).is_err());
    }
}
