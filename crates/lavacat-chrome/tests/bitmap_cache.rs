use lavacat_chrome::{
    pipeline,
    render::{PageFrame, PageRasterCache, RenderThread},
};
use lavacat_paint::{Color, Command, Image, Rasterizer, Rect, Scene, Size, VelloCpuRasterizer};
use lavacat_text::ParleyTextShaper;
use std::sync::Arc;
#[test]
fn rich_bitmap_and_layered_frames_match_direct_raster_with_rgba8_tolerance() {
    let html = "<body><div>hello cached fonts 汉字</div><p>second line</p><div style='height:300px'></div><div>tile crossing text</div><p>more text</p><div style='height:300px'></div><p>end text</p></body>";
    let css = "body {margin:0;background:#678} div {background:linear-gradient(90deg,red,blue);border-radius:7px} p {height:25px;overflow:hidden;color:rgba(50,100,200,0.5);background:rgba(90,180,20,0.25)}";
    let page = pipeline::load_page(html, css);
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 400.0).unwrap();
    let mut scene = pipeline::build_scene(&page, &root);
    scene.images.push(Arc::new(Image::new(
        2,
        2,
        vec![
            255, 0, 0, 128, 0, 255, 0, 255, 0, 0, 255, 80, 100, 100, 100, 0,
        ],
    )));
    scene.commands.push(Command::DrawImage {
        rect: Rect {
            x: 20.0,
            y: 250.0,
            width: 10.0,
            height: 10.0,
        },
        image: 0,
    });
    let backend = VelloCpuRasterizer::new();
    let mut cache = PageRasterCache::default();
    let size = Size {
        width: 400,
        height: 640,
    };
    let renderer = RenderThread::spawn(Arc::new(VelloCpuRasterizer::new()), Arc::new(|| {}));
    for offset in [0.0, -20.0, -250.0, -257.0, 48.0, -250.25] {
        let actual = cache.rasterize(&backend, &scene, 1, size, offset).unwrap();
        let direct = backend
            .rasterize(&scene.translated(0.0, offset), size)
            .unwrap();
        let diffs = actual
            .data
            .iter()
            .zip(&direct.data)
            .filter(|(a, b)| a != b)
            .count();
        let maxdiff = actual
            .data
            .iter()
            .zip(&direct.data)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        println!("tiles offset={offset} diffs={diffs} maxdiff={maxdiff}");
        assert!(maxdiff <= 1, "tile maxdiff={maxdiff} at {offset}");
        let background = Scene {
            commands: pipeline::canvas_background_commands(&page, 400.0, 640.0),
            ..Scene::default()
        };
        let overlay = Scene {
            commands: vec![Command::FillRect {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 400.0,
                    height: 48.0,
                },
                color: Color::opaque(200, 200, 200),
            }],
            ..Scene::default()
        };
        let mut merged = scene.translated(0.0, offset);
        for (i, c) in background.commands.iter().cloned().enumerate() {
            merged.commands.insert(i, c)
        }
        merged.commands.extend(overlay.commands.clone());
        let expected = backend.rasterize(&merged, size).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        renderer
            .request_page(
                PageFrame {
                    page: scene.clone(),
                    generation: 1,
                    origin_y: offset,
                    background,
                    overlay,
                },
                size,
                tx,
            )
            .unwrap();
        let actual = rx.recv().unwrap().unwrap();
        let diffs = actual
            .data
            .iter()
            .zip(&expected.data)
            .filter(|(a, b)| a != b)
            .count();
        let maxdiff = actual
            .data
            .iter()
            .zip(&expected.data)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        println!("layered offset={offset} diffs={diffs} maxdiff={maxdiff}");
        assert!(maxdiff <= 2, "composite maxdiff={maxdiff} at {offset}");
    }
}
