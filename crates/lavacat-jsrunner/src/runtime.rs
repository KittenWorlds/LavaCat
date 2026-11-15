use crate::protocol::{Bootstrap, DomNode, DomSnapshot, Request, Response};
use crate::snapshot::{LayoutNode, LayoutSnapshot};
use crate::{ABI_VERSION, MAX_SNAPSHOT_BYTES, RunnerError, error};
use ipc_channel::ipc::{IpcOneShotServer, IpcSharedMemory};
use lavacat_chrome::{pipeline, script::ScriptSession};
use lavacat_css::{MediaViewport, Stylesheet};
use lavacat_dom::{
    Attribute, Document, DocumentTypeData, ElementData, Event, Namespace, NodeId, NodeKind,
    ProcessingInstructionData, QuirksMode,
};
use std::{cell::RefCell, io::Write, rc::Rc};

pub(crate) fn serve() -> Result<(), RunnerError> {
    let (server, name) = IpcOneShotServer::<Bootstrap>::new()
        .map_err(|e| error(format!("create bootstrap: {e}")))?;
    println!("{name}");
    std::io::stdout()
        .flush()
        .map_err(|e| error(format!("stdout flush: {e}")))?;
    let (_, bootstrap) = server
        .accept()
        .map_err(|e| error(format!("accept bootstrap: {e}")))?;
    bootstrap
        .replies
        .send(Response::Hello {
            abi: ABI_VERSION,
            engine: "quickjs-ng".into(),
            capabilities: vec!["dom-ir-v1".into(), "layout-ir-v1".into()],
        })
        .map_err(error)?;
    let mut state = State {
        page: None,
        script: None,
        css: String::new(),
        suspended: false,
        sequence: 0,
        shaper: lavacat_text::ParleyTextShaper::new(),
    };
    while let Ok(request) = bootstrap.commands.recv() {
        if matches!(request, Request::Shutdown) {
            break;
        }
        let response = state
            .handle(request)
            .unwrap_or_else(|e| Response::Error(e.0));
        if bootstrap.replies.send(response).is_err() {
            break;
        }
    }
    Ok(())
}

struct State {
    page: Option<pipeline::Page>,
    script: Option<ScriptSession>,
    css: String,
    suspended: bool,
    sequence: u64,
    shaper: lavacat_text::ParleyTextShaper,
}
impl State {
    fn install(&mut self, page: pipeline::Page, css: String) -> Result<(), RunnerError> {
        let sheets = author_sheets(&page.document.borrow(), &css);
        let session = ScriptSession::install(&page.document, &sheets).map_err(error)?;
        let page = pipeline::build_page(page.document, sheets, page.images);
        self.page = Some(page);
        self.script = Some(session);
        self.css = css;
        Ok(())
    }
    fn handle(&mut self, request: Request) -> Result<Response, RunnerError> {
        match request {
            Request::Create { html, css } => {
                if html.len() > 4 * 1024 * 1024 || css.len() > 1024 * 1024 {
                    return Err(error("context input exceeds budget"));
                }
                if self.suspended {
                    return Err(error("runner is suspended"));
                }
                // Inline script scheduling stays explicit in this prototype. Import
                // and recovery must never replay scripts with external side effects.
                self.install(pipeline::load_page(&html, &css), css)?;
                Ok(Response::Ack)
            }
            Request::Eval(source) => {
                if self.suspended {
                    return Err(error("runner is suspended"));
                }
                if source.len() > 1024 * 1024 {
                    return Err(error("script exceeds budget"));
                }
                let session = self.script.as_mut().ok_or_else(|| error("no context"))?;
                session.eval(&source, "ipc-eval").map_err(error)?;
                session.pump().map_err(error)?;
                Ok(Response::Ack)
            }
            Request::Dispatch { node, kind } => {
                if self.suspended {
                    return Err(error("runner is suspended"));
                }
                if kind.len() > 256 || node > usize::MAX as u64 {
                    return Err(error("invalid event"));
                }
                let page = self.page.as_ref().ok_or_else(|| error("no context"))?;
                let target = NodeId::from_index(node as usize);
                if page.document.borrow().node(target).is_none() {
                    return Err(error("unknown target"));
                }
                let session = self.script.as_mut().ok_or_else(|| error("no context"))?;
                session
                    .dispatch(target, &mut Event::new(kind, true, true))
                    .map_err(error)?;
                session.pump().map_err(error)?;
                Ok(Response::Ack)
            }
            Request::Layout { width, height } => {
                if !width.is_finite()
                    || !height.is_finite()
                    || !(1.0..=16384.0).contains(&width)
                    || !(1.0..=16384.0).contains(&height)
                {
                    return Err(error("invalid viewport"));
                }
                let page = self.page.as_mut().ok_or_else(|| error("no context"))?;
                // Reconcile embedded style DOM before both layout and export/import
                // comparisons; otherwise dynamic style text would change only on restore.
                let sheets = author_sheets(&page.document.borrow(), &self.css);
                if let Some(session) = &mut self.script {
                    session.update_stylesheets(&sheets);
                }
                *page =
                    pipeline::build_page(Rc::clone(&page.document), sheets, page.images.clone());
                pipeline::recascade(page, MediaViewport { width, height });
                let root = pipeline::layout_page(page, &self.shaper, width)
                    .ok_or_else(|| error("no layout root"))?;
                let mut nodes = Vec::new();
                flatten(&root, u32::MAX, [0.0, 0.0], &mut nodes, &mut 0)?;
                self.sequence = self
                    .sequence
                    .checked_add(1)
                    .ok_or_else(|| error("frame sequence exhausted"))?;
                let snapshot = LayoutSnapshot {
                    abi: ABI_VERSION,
                    sequence: self.sequence,
                    width,
                    height,
                    nodes,
                };
                let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&snapshot).map_err(error)?;
                if bytes.len() > MAX_SNAPSHOT_BYTES {
                    return Err(error("layout frame exceeds budget"));
                }
                Ok(Response::Frame(IpcSharedMemory::from_bytes(&bytes)))
            }
            Request::Export => {
                let page = self.page.as_ref().ok_or_else(|| error("no context"))?;
                Ok(Response::Checkpoint(export(
                    &page.document.borrow(),
                    self.css.clone(),
                )?))
            }
            Request::Import(snapshot) => {
                let document = import(&snapshot)?;
                let css = snapshot.css;
                let page = pipeline::build_page(
                    Rc::new(RefCell::new(document)),
                    vec![Stylesheet::parse(&css)],
                    Default::default(),
                );
                self.install(page, css)?;
                Ok(Response::Ack)
            }
            Request::Suspend => {
                self.suspended = true;
                Ok(Response::Ack)
            }
            Request::Resume => {
                self.suspended = false;
                Ok(Response::Ack)
            }
            Request::Heartbeat => Ok(Response::Ack),
            Request::Shutdown => unreachable!(),
        }
    }
}

fn author_sheets(document: &Document, css: &str) -> Vec<Stylesheet> {
    let mut sheets = vec![Stylesheet::parse(css)];
    for resource in lavacat_html::collect_style_resources(document) {
        if let lavacat_html::StyleResource::Inline { css } = resource {
            sheets.push(Stylesheet::parse(&css));
        }
    }
    sheets
}

fn flatten(
    fragment: &lavacat_layout::Fragment,
    parent: u32,
    origin: [f32; 2],
    out: &mut Vec<LayoutNode>,
    bytes: &mut usize,
) -> Result<(), RunnerError> {
    if out.len() >= 100_000 {
        return Err(error("layout node budget exceeded"));
    }
    let x = origin[0] + fragment.border_box.x;
    let y = origin[1] + fragment.border_box.y;
    let index = out.len() as u32;
    let mut text = String::new();
    for line in &fragment.lines {
        for run in &line.runs {
            let visible = run
                .text
                .get(run.text_start as usize..run.text_end as usize)
                .unwrap_or("");
            *bytes = bytes.saturating_add(visible.len() + 64);
            if *bytes > MAX_SNAPSHOT_BYTES {
                return Err(error("layout text exceeds budget"));
            }
            text.push_str(visible);
        }
    }
    out.push(LayoutNode {
        node: if fragment.anonymous {
            u64::MAX
        } else {
            fragment.node.index() as u64
        },
        parent,
        rect: [x, y, fragment.border_box.width, fragment.border_box.height],
        text,
    });
    let content = [
        x + fragment.border.left + fragment.padding.left,
        y + fragment.border.top + fragment.padding.top,
    ];
    let mut line_offset = 0.0;
    for line in &fragment.lines {
        for atomic in &line.boxes {
            flatten(
                atomic,
                index,
                [content[0], content[1] + line_offset],
                out,
                bytes,
            )?;
        }
        line_offset += line.height;
    }
    for child in &fragment.children {
        flatten(child, index, content, out, bytes)?;
    }
    Ok(())
}

fn ns(namespace: &Namespace) -> String {
    match namespace {
        Namespace::None => "".into(),
        Namespace::Html => "http://www.w3.org/1999/xhtml".into(),
        Namespace::Svg => "http://www.w3.org/2000/svg".into(),
        Namespace::MathMl => "http://www.w3.org/1998/Math/MathML".into(),
        Namespace::Other(value) => value.clone(),
    }
}
fn parse_ns(value: &str) -> Namespace {
    match value {
        "" => Namespace::None,
        "http://www.w3.org/1999/xhtml" => Namespace::Html,
        "http://www.w3.org/2000/svg" => Namespace::Svg,
        "http://www.w3.org/1998/Math/MathML" => Namespace::MathMl,
        value => Namespace::Other(value.into()),
    }
}
fn export(document: &Document, css: String) -> Result<DomSnapshot, RunnerError> {
    if document.node_count() > 100_000 {
        return Err(error("DOM node budget exceeded"));
    }
    let mut nodes = Vec::new();
    for index in 0..document.node_count() {
        let id = NodeId::from_index(index);
        let mut node = DomNode {
            kind: 0,
            name: String::new(),
            namespace: String::new(),
            value: String::new(),
            extra: String::new(),
            attributes: vec![],
            children: document.children(id).map(|id| id.index() as u32).collect(),
            template: document.template_contents(id).map(|id| id.index() as u32),
        };
        match document.node(id).ok_or_else(|| error("invalid arena"))? {
            NodeKind::Document => {}
            NodeKind::Doctype(data) => {
                node.kind = 1;
                node.name = data.name.clone();
                node.value = data.public_id.clone();
                node.extra = data.system_id.clone();
            }
            NodeKind::DocumentFragment => node.kind = 2,
            NodeKind::Element(data) => {
                node.kind = 3;
                node.name = data.name.clone();
                node.namespace = ns(&data.namespace);
                node.attributes = data
                    .attributes
                    .iter()
                    .map(|a| (a.name.clone(), ns(&a.namespace), a.value.clone()))
                    .collect();
            }
            NodeKind::Text(text) => {
                node.kind = 4;
                node.value = text.clone();
            }
            NodeKind::Comment(text) => {
                node.kind = 5;
                node.value = text.clone();
            }
            NodeKind::ProcessingInstruction(data) => {
                node.kind = 6;
                node.name = data.target.clone();
                node.value = data.data.clone();
            }
        }
        nodes.push(node);
    }
    let snapshot = DomSnapshot {
        nodes,
        css,
        mode: match document.quirks_mode() {
            QuirksMode::NoQuirks => 0,
            QuirksMode::Quirks => 1,
            QuirksMode::LimitedQuirks => 2,
        },
    };
    validate(&snapshot)?;
    Ok(snapshot)
}

fn validate(snapshot: &DomSnapshot) -> Result<(), RunnerError> {
    let nodes = &snapshot.nodes;
    if nodes.is_empty() || nodes.len() > 100_000 || nodes[0].kind != 0 || snapshot.mode > 2 {
        return Err(error("invalid DOM IR"));
    }
    let mut bytes = snapshot.css.len();
    let mut parents = vec![0_u8; nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        if node.kind > 6 || (index > 0 && node.kind == 0) {
            return Err(error("invalid DOM kind"));
        }
        if !matches!(node.kind, 0 | 2 | 3) && (!node.children.is_empty() || node.template.is_some())
        {
            return Err(error("leaf DOM node has children"));
        }
        bytes = bytes.saturating_add(
            node.name.len()
                + node.namespace.len()
                + node.value.len()
                + node.extra.len()
                + 64
                + node.children.len() * 4,
        );
        for (name, namespace, value) in &node.attributes {
            bytes = bytes.saturating_add(name.len() + namespace.len() + value.len() + 72);
        }
        if bytes > MAX_SNAPSHOT_BYTES {
            return Err(error("DOM checkpoint exceeds budget"));
        }
        if node
            .children
            .iter()
            .any(|child| nodes.get(*child as usize).is_some_and(|n| n.kind == 2))
        {
            return Err(error("a fragment cannot be an ordinary DOM child"));
        }
        for child in node.children.iter().copied().chain(node.template) {
            let child = child as usize;
            if child == 0 || child >= nodes.len() || parents[child] != 0 {
                return Err(error("invalid DOM ownership"));
            }
            parents[child] = 1;
        }
        if node
            .template
            .is_some_and(|i| node.kind != 3 || nodes[i as usize].kind != 2)
        {
            return Err(error("invalid template contents"));
        }
    }
    // Check every disconnected arena tree too; invalid/cyclic imports never reach
    // the DOM's unchecked parser insertion method.
    fn visit(
        index: usize,
        nodes: &[DomNode],
        marks: &mut [u8],
        depth: usize,
    ) -> Result<(), RunnerError> {
        if depth > lavacat_dom::MAX_TREE_DEPTH || marks[index] == 1 {
            return Err(error("cyclic or overdeep DOM IR"));
        }
        if marks[index] == 2 {
            return Ok(());
        }
        marks[index] = 1;
        for child in nodes[index]
            .children
            .iter()
            .copied()
            .chain(nodes[index].template)
        {
            visit(child as usize, nodes, marks, depth + 1)?;
        }
        marks[index] = 2;
        Ok(())
    }
    let mut marks = vec![0; nodes.len()];
    for (i, count) in parents.iter().enumerate() {
        if *count == 0 {
            visit(i, nodes, &mut marks, 0)?;
        }
    }
    if marks.iter().any(|mark| *mark != 2) {
        return Err(error("cyclic DOM IR"));
    }
    Ok(())
}
fn import(snapshot: &DomSnapshot) -> Result<Document, RunnerError> {
    validate(snapshot)?;
    let mut document = Document::new();
    for node in snapshot.nodes.iter().skip(1) {
        let kind = match node.kind {
            1 => NodeKind::Doctype(DocumentTypeData {
                name: node.name.clone(),
                public_id: node.value.clone(),
                system_id: node.extra.clone(),
            }),
            2 => NodeKind::DocumentFragment,
            3 => NodeKind::Element(ElementData {
                name: node.name.clone(),
                namespace: parse_ns(&node.namespace),
                attributes: node
                    .attributes
                    .iter()
                    .map(|(name, ns, value)| Attribute {
                        name: name.clone(),
                        namespace: parse_ns(ns),
                        value: value.clone(),
                    })
                    .collect(),
            }),
            4 => NodeKind::Text(node.value.clone()),
            5 => NodeKind::Comment(node.value.clone()),
            6 => NodeKind::ProcessingInstruction(ProcessingInstructionData {
                target: node.name.clone(),
                data: node.value.clone(),
            }),
            _ => return Err(error("invalid DOM kind")),
        };
        document.create_node(kind);
    }
    document.set_quirks_mode(match snapshot.mode {
        1 => QuirksMode::Quirks,
        2 => QuirksMode::LimitedQuirks,
        _ => QuirksMode::NoQuirks,
    });
    for (index, node) in snapshot.nodes.iter().enumerate() {
        let parent = NodeId::from_index(index);
        for child in &node.children {
            document.insert_node(NodeId::from_index(*child as usize), parent, None);
        }
        if let Some(template) = node.template {
            document.set_template_contents(parent, NodeId::from_index(template as usize));
        }
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dom_ir_roundtrips_templates_detached_nodes_and_attributes() {
        let page = pipeline::load_page(
            "<!doctype html><template><b title='x'>template</b></template><p id='live'>hello</p>",
            "p{color:red}",
        );
        page.document
            .borrow_mut()
            .create_node(NodeKind::Text("detached".into()));
        let snapshot = export(&page.document.borrow(), "p{color:red}".into()).unwrap();
        let restored = import(&snapshot).unwrap();
        assert_eq!(export(&restored, snapshot.css.clone()).unwrap(), snapshot);
    }
    fn state() -> State {
        State {
            page: None,
            script: None,
            css: String::new(),
            suspended: false,
            sequence: 0,
            shaper: lavacat_text::ParleyTextShaper::new(),
        }
    }
    fn frame(state: &mut State) -> crate::LayoutFrame {
        match state
            .handle(Request::Layout {
                width: 400.0,
                height: 300.0,
            })
            .unwrap()
        {
            Response::Frame(memory) => crate::LayoutFrame::new(memory).unwrap(),
            _ => panic!("frame"),
        }
    }
    #[test]
    fn inline_atomic_text_and_embedded_styles_survive_checkpoint_import() {
        let mut source = state();
        source.handle(Request::Create { html: "<style id='dynamic'>.hidden{display:none}</style><p><span style='display:inline-block'>atomic text</span></p><p class=hidden>must hide</p>".into(), css: "p{margin:0}".into() }).unwrap();
        let before = frame(&mut source);
        let text = before
            .nodes()
            .map(|n| n.text().to_owned())
            .collect::<String>();
        assert!(text.contains("atomic text"));
        assert!(!text.contains("must hide"));
        let snapshot = match source.handle(Request::Export).unwrap() {
            Response::Checkpoint(snapshot) => snapshot,
            _ => panic!("checkpoint"),
        };
        let mut restored = state();
        restored.handle(Request::Import(snapshot)).unwrap();
        let after = frame(&mut restored);
        let digest = |frame: &crate::LayoutFrame| {
            frame
                .nodes()
                .map(|n| (n.identity(), n.parent(), n.rect(), n.text().to_owned()))
                .collect::<Vec<_>>()
        };
        assert_eq!(digest(&before), digest(&after));
        source
            .handle(Request::Eval(
                "document.getElementById('dynamic').textContent = 'p{display:none}'".into(),
            ))
            .unwrap();
        assert!(
            !frame(&mut source)
                .nodes()
                .any(|n| n.text().contains("atomic text"))
        );
        let snapshot = match source.handle(Request::Export).unwrap() {
            Response::Checkpoint(s) => s,
            _ => panic!("snapshot"),
        };
        restored.handle(Request::Import(snapshot)).unwrap();
        assert!(
            !frame(&mut restored)
                .nodes()
                .any(|n| n.text().contains("atomic text"))
        );
    }
    #[test]
    fn script_dom_lifecycle_and_error_paths_work_without_a_transport() {
        let mut state = state();
        assert!(state.handle(Request::Eval("1".into())).is_err());
        state
            .handle(Request::Create {
                html: "<p id='live'>before</p>".into(),
                css: String::new(),
            })
            .unwrap();
        state
            .handle(Request::Eval(
                "document.getElementById('live').textContent='after'".into(),
            ))
            .unwrap();
        assert!(
            frame(&mut state)
                .nodes()
                .any(|n| n.text().contains("after"))
        );
        state.handle(Request::Suspend).unwrap();
        assert!(state.handle(Request::Eval("1".into())).is_err());
        state.handle(Request::Heartbeat).unwrap();
        state.handle(Request::Resume).unwrap();
        assert!(
            state
                .handle(Request::Eval("throw new Error('expected')".into()))
                .is_err()
        );
        state
            .handle(Request::Eval(
                "document.getElementById('live').textContent='healthy'".into(),
            ))
            .unwrap();
        assert!(
            frame(&mut state)
                .nodes()
                .any(|n| n.text().contains("healthy"))
        );
    }

    #[test]
    fn invalid_dom_graphs_never_reach_unchecked_insertion() {
        let page = pipeline::load_page("<div><p>safe</p></div>", "");
        let snapshot = export(&page.document.borrow(), String::new()).unwrap();
        let mut invalid = snapshot.clone();
        invalid.nodes[0].children.push(0);
        assert!(import(&invalid).is_err());
        let mut invalid = snapshot.clone();
        invalid.nodes[0].children.push(u32::MAX);
        assert!(import(&invalid).is_err());
        let mut invalid = snapshot.clone();
        invalid.nodes[0]
            .children
            .extend(snapshot.nodes[0].children.iter());
        assert!(import(&invalid).is_err());
        let mut invalid = snapshot;
        invalid.nodes[0].template = Some(u32::MAX);
        assert!(import(&invalid).is_err());
    }
}
