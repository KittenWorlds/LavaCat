//! 渲染隔离线程：光栅化在独立线程执行，panic 由 `catch_unwind` 兜底。
//!
//! 对应 AGENTS.md 硬规则「渲染隔离」：vello 系后端对未支持特性是 panic
//! 而非返回错误，渲染失败不得拖垮主进程。本模块保证：
//! 1. 光栅化只发生在专用线程（主线程只做请求/应答）；
//! 2. 单次渲染 panic 被捕获并转为 [`RenderError::Panicked`]，
//!    线程存活、后续请求照常服务；
//! 3. 渲染线程持有的 Rasterizer 由调用方注入（CPU 后端或未来的 GPU 适配）。

use std::sync::Arc;
use std::thread::JoinHandle;

use lavacat_paint::{Pixmap, RasterError, Rasterizer, Scene, Size};

/// 渲染错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// 光栅后端返回的错误。
    Raster(RasterError),
    /// 光栅过程 panic（payload 已转为字符串），本次渲染失败但线程存活。
    Panicked(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Raster(error) => write!(f, "raster error: {error:?}"),
            RenderError::Panicked(message) => write!(f, "renderer panicked: {message}"),
        }
    }
}

impl std::error::Error for RenderError {}

enum LayerJob {
    Full(Scene),
    Page(Box<PageFrame>),
}

type Job = (
    LayerJob,
    Size,
    std::sync::mpsc::Sender<Result<Pixmap, RenderError>>,
);

/// 渲染线程句柄。Drop 时等待线程退出。
pub struct RenderThread {
    sender: Option<std::sync::mpsc::Sender<Job>>,
    handle: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for RenderThread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderThread").finish()
    }
}

impl RenderThread {
    /// 启动渲染线程。
    ///
    /// `waker` 在每份作业完成后调用一次——异步呈现路径用它唤醒事件循环
    /// 取结果：渲染完成时主线程不得阻塞等待，否则巨页帧（可达数十秒）
    /// 会让窗口进入「无响应」状态，系统随之重发 WM_PAINT / WM_SIZE 形成
    /// 重绘风暴（winit 0.30 把 WM_PAINT 直接映射为 RedrawRequested）。
    ///
    /// # Panics
    ///
    /// 线程启动失败时 panic（主进程随后的渲染请求会得到通道错误）。
    #[must_use]
    pub fn spawn(
        rasterizer: Arc<dyn Rasterizer + Send + Sync>,
        waker: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel::<Job>();
        let handle = std::thread::Builder::new()
            .name("lavacat-render".to_owned())
            .spawn(move || {
                let mut cache = PageRasterCache::default();
                while let Ok((job, size, reply)) = receiver.recv() {
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
                            LayerJob::Full(scene) => rasterizer.rasterize(&scene, size),
                            LayerJob::Page(frame) => {
                                render_page_frame(rasterizer.as_ref(), &mut cache, &frame, size)
                            }
                        }))
                        .map_err(panic_message)
                        .and_then(|result| result.map_err(RenderError::Raster));
                    if result.is_err() {
                        cache = PageRasterCache::default();
                    }
                    let _ = reply.send(result);
                    waker();
                }
            })
            .expect("spawn render thread");
        Self {
            sender: Some(sender),
            handle: Some(handle),
        }
    }

    /// 请求一帧光栅化（异步）：结果经 `reply` 通道交回，调用方不等待。
    ///
    /// 接收 `Scene` **所有权**（M1 零拷贝）。返回 `Err(scene)` 表示渲染
    /// 线程已终止，场景原样还给调用方以便降级处理。
    pub fn request(
        &self,
        scene: Scene,
        size: Size,
        reply: std::sync::mpsc::Sender<Result<Pixmap, RenderError>>,
    ) -> Result<(), Scene> {
        self.sender
            .as_ref()
            .expect("render thread sender")
            .send((LayerJob::Full(scene), size, reply))
            .map_err(|std::sync::mpsc::SendError((job, _, _))| match job {
                LayerJob::Full(scene) => scene,
                LayerJob::Page(_) => unreachable!("full request"),
            })
    }

    /// Request a layered frame with CPU page bitmap reuse. Rasterization and
    /// compositing remain inside the panic-isolated render thread.
    pub fn request_page(
        &self,
        frame: PageFrame,
        size: Size,
        reply: std::sync::mpsc::Sender<Result<Pixmap, RenderError>>,
    ) -> Result<(), Box<PageFrame>> {
        self.sender
            .as_ref()
            .expect("render thread sender")
            .send((LayerJob::Page(Box::new(frame)), size, reply))
            .map_err(|std::sync::mpsc::SendError((job, _, _))| match job {
                LayerJob::Page(frame) => frame,
                LayerJob::Full(_) => unreachable!("page request"),
            })
    }

    /// 请求一帧光栅化，阻塞等待结果（测试与工具路径用；窗口主循环走
    /// [`RenderThread::request`] 异步呈现）。
    ///
    /// # Errors
    ///
    /// 光栅失败或渲染线程 panic 时返回 [`RenderError`]；通道断裂
    /// （线程意外退出）也归入 [`RenderError::Panicked`]。
    pub fn render(&self, scene: Scene, size: Size) -> Result<Pixmap, RenderError> {
        let (reply_sender, reply_receiver) = std::sync::mpsc::channel();
        self.request(scene, size, reply_sender)
            .map_err(|_| RenderError::Panicked("render thread terminated".to_owned()))?;
        reply_receiver
            .recv()
            .map_err(|_| RenderError::Panicked("render thread terminated".to_owned()))?
    }
}

impl Drop for RenderThread {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn render_page_frame(
    rasterizer: &dyn Rasterizer,
    cache: &mut PageRasterCache,
    frame: &PageFrame,
    size: Size,
) -> Result<Pixmap, RasterError> {
    let page = cache.rasterize(
        rasterizer,
        &frame.page,
        frame.generation,
        size,
        frame.origin_y,
    )?;
    let mut result = rasterizer.rasterize(&frame.background, size)?;
    composite(&mut result, &page);
    let overlay = rasterizer.rasterize(&frame.overlay, size)?;
    composite(&mut result, &overlay);
    Ok(result)
}

// Source-over for non-premultiplied RGBA8 (Compositing 1 §5.1 / §10.1).
fn composite(target: &mut Pixmap, source: &Pixmap) {
    assert_eq!(target.size, source.size);
    assert_eq!(target.data.len(), source.data.len());
    for (dst, src) in target
        .data
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(source.data.as_chunks::<4>().0.iter())
    {
        let sa = u32::from(src[3]);
        if sa == 0 {
            continue;
        }
        if sa == 255 {
            dst.copy_from_slice(src);
            continue;
        }
        let da = u32::from(dst[3]);
        let alpha = sa * 255 + da * (255 - sa);
        for c in 0..3 {
            let value = u32::from(src[c]) * sa * 255 + u32::from(dst[c]) * da * (255 - sa);
            dst[c] = ((value + alpha / 2) / alpha) as u8;
        }
        dst[3] = ((alpha + 127) / 255) as u8;
    }
}

/// 从 panic payload 提取可读信息。
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> RenderError {
    let message = if let Some(text) = payload.downcast_ref::<&'static str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic".to_owned()
    };
    RenderError::Panicked(message)
}

// Compute exact outline-based bounds once per immutable display-list revision.
// Clip stack commands are always retained, preserving every nesting transition.
fn command_vertical_bounds(scene: &Scene) -> Vec<Option<(f32, f32)>> {
    use lavacat_paint::Command;
    let mut glyph_bounds = std::collections::HashMap::new();
    scene
        .commands
        .iter()
        .map(|command| {
            let (min, max) = match command {
                Command::PushClip { .. } | Command::PopClip => return None,
                Command::DrawText {
                    glyphs,
                    baseline,
                    fonts,
                    size,
                    ..
                } => {
                    let mut min = f32::INFINITY;
                    let mut max = f32::NEG_INFINITY;
                    for glyph in glyphs {
                        let Some(font) = fonts.get(glyph.font as usize) else {
                            continue;
                        };
                        let key = (font.id(), font.index(), size.to_bits(), glyph.id);
                        let bounds = match glyph_bounds.get(&key) {
                            Some(bounds) => *bounds,
                            None => {
                                let bounds = font.glyph_bounds(glyph.id, *size);
                                if glyph_bounds.len() < 65536 {
                                    glyph_bounds.insert(key, bounds);
                                }
                                bounds
                            }
                        }?;
                        min = min.min(baseline + glyph.y - bounds[3]);
                        max = max.max(baseline + glyph.y - bounds[1]);
                    }
                    (min, max)
                }
                Command::StrokeRect { rect, width, .. } => (
                    rect.y - width.abs() / 2.0,
                    rect.y + rect.height + width.abs() / 2.0,
                ),
                Command::FillRect { rect, .. }
                | Command::FillRoundedRect { rect, .. }
                | Command::StrokeRoundedRect { rect, .. }
                | Command::FillGradient { rect, .. }
                | Command::DrawImage { rect, .. } => (rect.y, rect.y + rect.height),
            };
            (min.is_finite() && max.is_finite()).then_some((min, max))
        })
        .collect()
}

/// A page layer and viewport layers for CPU bitmap reuse (CSS Overflow 3 §2.2).
///
/// `generation` must change whenever page commands/resources change, including
/// highlights and container offsets. Canvas background and UI use viewport
/// coordinates and are composited separately, so they never enter page tiles.
#[derive(Debug)]
pub struct PageFrame {
    /// Immutable page-coordinate display list.
    pub page: Scene,
    /// Display-list revision, unique for this renderer's lifetime.
    pub generation: u64,
    /// Page origin in viewport pixels (toolbar height minus scrollTop).
    pub origin_y: f32,
    /// Canvas background in viewport coordinates.
    pub background: Scene,
    /// Scrollbars and browser chrome in viewport coordinates.
    pub overlay: Scene,
}

/// Bounded CPU page bitmap cache. Integer scrolls copy retained rows; fractional
/// offsets use full rasterization to preserve the backend's antialiasing phase.
///
/// Tiles contain the fully composited page layer, including nested clips,
/// translucent content and images. CSS painting order is unchanged. Compositing
/// this RGBA8 layer onto the canvas can introduce small RGBA8 rounding differences
/// versus a single backend pass (Compositing and Blending 1 §5.1 / §10.1).
#[derive(Debug, Default)]
pub struct PageRasterCache {
    key: Option<(u64, u32)>,
    command_bounds: Vec<Option<(f32, f32)>>,
    tiles: std::collections::HashMap<i32, Pixmap>,
}

impl PageRasterCache {
    /// Rasterize the visible page. The caller must advance `generation` on any
    /// display-list or resource change. Retained tiles use at most 64 MiB.
    /// Backend errors propagate without caching a failed tile.
    pub fn rasterize(
        &mut self,
        rasterizer: &dyn Rasterizer,
        scene: &Scene,
        generation: u64,
        size: Size,
        origin_y: f32,
    ) -> Result<Pixmap, RasterError> {
        const TILE_HEIGHT: i32 = 256;
        const BUDGET: u64 = 64 * 1024 * 1024;
        let tile_bytes = u64::from(size.width) * TILE_HEIGHT as u64 * 4;
        let max_tiles = u64::from(size.height).div_ceil(TILE_HEIGHT as u64) + 1;
        if size.width == 0
            || size.height == 0
            || size.width > 16384
            || size.height > 16384
            || tile_bytes * max_tiles > BUDGET
            || !origin_y.is_finite()
            || origin_y.fract() != 0.0
            || origin_y.abs() > 1e7
            || scene.commands.len() > 1_000_000
        {
            return rasterizer.rasterize(&scene.translated(0.0, origin_y), size);
        }
        if self.key != Some((generation, size.width)) {
            self.tiles.clear();
            self.key = Some((generation, size.width));
            self.command_bounds = command_vertical_bounds(scene);
        }
        let origin = origin_y as i32;
        let first = (-origin).div_euclid(TILE_HEIGHT);
        let last = (size.height as i32 - 1 - origin).div_euclid(TILE_HEIGHT);
        self.tiles
            .retain(|index, _| *index >= first && *index <= last);
        let mut output = Pixmap {
            size,
            data: vec![0; size.width as usize * size.height as usize * 4],
        };
        let stride = size.width as usize * 4;
        for index in first..=last {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.tiles.entry(index) {
                let top = (index * TILE_HEIGHT) as f32;
                let visible = Scene {
                    commands: scene
                        .commands
                        .iter()
                        .zip(&self.command_bounds)
                        .filter(|(_, bounds)| {
                            bounds.is_none_or(|(min, max)| {
                                max >= top - 1.0 && min <= top + TILE_HEIGHT as f32 + 1.0
                            })
                        })
                        .map(|(command, _)| command.clone())
                        .collect(),
                    images: scene.images.clone(),
                };
                let tile = rasterizer.rasterize(
                    &visible.translated(0.0, -top),
                    Size {
                        width: size.width,
                        height: TILE_HEIGHT as u32,
                    },
                )?;
                entry.insert(tile);
            }
            let tile = &self.tiles[&index];
            let target_start = (index * TILE_HEIGHT + origin).max(0) as usize;
            let target_end = ((index + 1) * TILE_HEIGHT + origin).min(size.height as i32) as usize;
            let source_start = (target_start as i32 - origin - index * TILE_HEIGHT) as usize;
            let rows = target_end - target_start;
            output.data[target_start * stride..target_end * stride]
                .copy_from_slice(&tile.data[source_start * stride..(source_start + rows) * stride]);
        }
        Ok(output)
    }
}

#[cfg(test)]
mod bitmap_tests {
    use super::*;
    use lavacat_paint::{Color, Command, Rect, VelloCpuRasterizer};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingRasterizer(AtomicUsize);
    impl Rasterizer for CountingRasterizer {
        fn rasterize(&self, scene: &Scene, size: Size) -> Result<Pixmap, RasterError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            VelloCpuRasterizer::new().rasterize(scene, size)
        }
    }

    fn scene() -> Scene {
        Scene {
            commands: vec![
                Command::FillRect {
                    rect: Rect {
                        x: 2.25,
                        y: 0.5,
                        width: 23.5,
                        height: 900.0,
                    },
                    color: Color::opaque(123, 45, 67),
                },
                Command::PushClip {
                    rect: Rect {
                        x: 0.0,
                        y: 245.5,
                        width: 32.0,
                        height: 290.0,
                    },
                },
                Command::FillRect {
                    rect: Rect {
                        x: 0.0,
                        y: 240.0,
                        width: 32.0,
                        height: 300.0,
                    },
                    color: Color {
                        r: 40,
                        g: 160,
                        b: 20,
                        a: 128,
                    },
                },
                Command::PopClip,
            ],
            ..Scene::default()
        }
    }

    #[test]
    fn newly_exposed_tiles_send_only_intersecting_paint_commands() {
        struct VisibleOnly;
        impl Rasterizer for VisibleOnly {
            fn rasterize(&self, scene: &Scene, size: Size) -> Result<Pixmap, RasterError> {
                assert!(
                    scene.commands.len() < 10,
                    "offscreen commands reached backend: {}",
                    scene.commands.len()
                );
                Ok(Pixmap {
                    size,
                    data: vec![0; size.width as usize * size.height as usize * 4],
                })
            }
        }
        let scene = Scene {
            commands: (0..10000)
                .map(|i| Command::FillRect {
                    rect: Rect {
                        x: 0.0,
                        y: i as f32 * 100.0,
                        width: 10.0,
                        height: 10.0,
                    },
                    color: Color::opaque(0, 0, 0),
                })
                .collect(),
            ..Scene::default()
        };
        let mut cache = PageRasterCache::default();
        cache
            .rasterize(
                &VisibleOnly,
                &scene,
                1,
                Size {
                    width: 32,
                    height: 200,
                },
                -300.0,
            )
            .unwrap();
    }

    #[test]
    fn integer_scroll_reuses_tiles_and_matches_full_raster() {
        let backend = CountingRasterizer(AtomicUsize::new(0));
        let mut cache = PageRasterCache::default();
        let scene = scene();
        let size = Size {
            width: 32,
            height: 200,
        };
        for offset in [0.0, -20.0, -40.0, -260.0, -270.0, -10.0, 20.0] {
            let actual = cache.rasterize(&backend, &scene, 1, size, offset).unwrap();
            let expected = VelloCpuRasterizer::new()
                .rasterize(&scene.translated(0.0, offset), size)
                .unwrap();
            assert_eq!(actual, expected, "offset {offset}");
        }
        assert!(
            backend.0.load(Ordering::Relaxed) < 7,
            "retained rows must avoid full rasterization"
        );
        let before = backend.0.load(Ordering::Relaxed);
        cache.rasterize(&backend, &scene, 1, size, 20.0).unwrap();
        assert_eq!(
            backend.0.load(Ordering::Relaxed),
            before,
            "same frame uses bitmap only"
        );
    }

    #[test]
    fn generation_width_and_fractional_phase_do_not_reuse_stale_pixels() {
        let mut cache = PageRasterCache::default();
        let backend = VelloCpuRasterizer::new();
        for (generation, width, offset) in [
            (1, 32, 0.0),
            (2, 32, -25.0),
            (2, 40, -25.0),
            (2, 40, -25.25),
        ] {
            let mut scene = scene();
            if generation == 2 {
                scene.commands.remove(0);
            }
            let size = Size { width, height: 200 };
            let actual = cache
                .rasterize(&backend, &scene, generation, size, offset)
                .unwrap();
            let expected = backend
                .rasterize(&scene.translated(0.0, offset), size)
                .unwrap();
            assert_eq!(actual, expected);
        }
    }
}
