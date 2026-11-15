# LavaCat

MusKitty 的**不造轮子**分支——一个用 Rust 构建的下一代浏览器内核。

不同于 Chromium、Firefox（Gecko）从零手写所有核心模块，LavaCat 的设计哲学反过来：
**能复用成熟 crate 就不自己实现，只把精力留给真正没有现成轮子的部分**
（DOM、Cascade、盒级布局中的 inline/文本断行）。

行为 ground truth 是 **WHATWG 规范**与 **WPT 测试套件**；Chromium 源码仅作参考。

## 当前状态

八层 facade crate + 2 个 runner 相关 crate 已就位，管线
`network → html → dom → css → layout → paint → chrome` 端到端连通：

```bash
cargo run -p lavacat-chrome --bin lavacat
```

起窗口后地址栏输入 URL 即可导航，支持滚动与滚动条拖拽（含容器滚动）、链接点击、
悬停光标、地址栏编辑与快捷键、后退/前进/刷新、页面文本选择与复制、favicon、
页面脚本执行（QuickJS）。

WPT tree-construction 比对：**1854/1959 通过**（scripted 档 1860/1959），
剩余 99 条已逐条入基线（88 条语料过时、11 条 html5ever 上游偏差），
比对方式与基线见 [tree_construction.rs](crates/lavacat-html/tests/tree_construction.rs)。

## 架构

一层一个 facade crate，外部依赖只允许出现在对应层内；对外能力先定 trait
（`NetworkFetcher` / `TextShaper` / `Rasterizer` 等），具体后端作为可替换实现。
依赖类型一律不外泄到公共 API。

```
LavaCat/
├── crates/
│   ├── lavacat-dom/          # 自研 arena DOM + 事件模型 + JSRT 接缝
│   ├── lavacat-html/         # html5ever → arena DOM 的 TreeSink 桥接
│   ├── lavacat-css/          # cssparser/selectors 封装 + 自研 cascade
│   ├── lavacat-layout/       # taffy 盒级几何 + 自研 inline/文本断行
│   ├── lavacat-text/         # parley（fontique 选字 + harfrust 整形）
│   ├── lavacat-paint/        # vello_cpu 光栅（GPU 后端 vello_hybrid 待接入）
│   ├── lavacat-network/      # reqwest 封装 + 解码 / 重定向 / HTTP 缓存
│   ├── lavacat-chrome/       # winit 窗口 + 导航管线 + 渲染线程
│   ├── lavacat-jsrt-rquickjs/  # JS runner 适配 crate（QuickJS-NG）
│   └── lavacat-jsrunner/     # runner 进程原型（controller + child runtime）
├── tools/acceptance/         # 实机验收站与页面
└── docs/                     # ADR 决策记录与轮次计划
```

### Crate 选型一览

| 层 | 选型 | 说明 |
| --- | --- | --- |
| HTML 解析 | `html5ever` | WHATWG §13 树构建 |
| DOM | **自研 arena DOM** | 与 JSRT 绑定同处 |
| CSS 解析 / 选择器 | `cssparser` + `selectors` | Selectors 4 |
| Cascade | **自研** | UA/author 双 origin、特异度、computed style |
| 盒级布局 | `taffy` + **自研 inline** | block/flex/grid/absolute/float 交 taffy；断行永久自研 |
| 文本整形 | `parley` | fontique 选字 + harfrust 整形 |
| 断行 | **自研 UAX#14** | 逐条比对官方 19,346 条 LineBreakTest |
| 绘制 / 光栅 | `vello_cpu` | 双后端 facade，渲染在隔离线程 |
| 网络 | `reqwest` | 缓存 / 解码 / 重定向编排 |
| 窗口 / 输入 | `winit` + `wgpu` | Pixmap → surface 呈现 |
| JS 运行时 | `rquickjs`（QuickJS-NG） | 可插拔，换引擎只重写适配 crate |
| 剪贴板 | `arboard` | 仅文本 |

完整理由与边界见 [crate 选型 ADR](docs/decisions/2026-10-01-crate-selection.md)。

### 硬约束

- Rust stable，facade crate 一律 `#![forbid(unsafe_code)]`（runner 适配 crate 允许 FFI unsafe）
- 每个 facade crate 的功能按 Cargo feature 拆分，`default` = 全部已实现能力
- 许可证白名单 MIT / Apache-2.0 / BSD / ISC / MPL-2.0 / LGPL / Zlib / CC0-1.0 /
  Unicode-3.0 / CDLA-Permissive-2.0 / BSL-1.0，禁 GPL / AGPL；`cargo deny` 硬门禁
- 渲染在独立线程执行，panic 由 `catch_unwind` 兜住，不拖垮主进程

详细规则见 [`AGENTS.md`](AGENTS.md)。

## 已落地能力（按批次）

- **批次 0**：HTML→DOM→CSS→布局→绘制→窗口管线；块级流、flex、grid、绝对定位、
  浮动；行内断行（UAX#14）、CSS Text 放置期修正（justify / text-indent /
  letter-spacing / word-spacing / tab-size / pre-wrap）；文本选择与复制
- **性能轮**：整形与断行缓存（H4）、图片零拷贝（M1）、级联规则桶索引（M5）、
  resize 合帧与滚动场景缓存（M6）、工具带整形缓存（M7）；
  基线见 [perf baseline](docs/2026-10-25-perf-baseline.md)
- **批次 1**：K1 DOM 事件模型 / K2 圆角、渐变、text-decoration、z-index 绘制 /
  K3 重定向、字符编码探测、HTTP 缓存 / K4 @font-face Web 字体
- **批次 2**：K5 JSRT 选型与最小绑定（QuickJS，`document`/元素门面、事件派发）/
  K6 容器滚动与裁剪 / K13 图片格式扩展（jpeg/gif/webp/ico）与 favicon
- **进行中**：runner 进程原型（[ADR](docs/decisions/2026-11-15-jsrunner-process-prototype.md)）；
  后续方向：table 布局、视口裁剪/增量光栅、vello_hybrid GPU 后端、表单基线、bidi

## 本地构建与测试

需要 Rust 1.90+（MSRV 基线）。

```bash
cargo check --workspace                        # 必须零 warning
cargo test --workspace                         # 全部测试
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# 依赖门禁与许可证清单（引入/升级任何依赖后必跑）
cargo deny check
cargo about generate about.hbs -o docs/dependencies.html
python tools/licenses/inject-notes.py .workbuddy/about.json docs/dependencies.html

# 实机验收站（浏览器外壳无无头模式，渲染/交互行为由验收页确认）
node tools/acceptance/server.cjs               # http://127.0.0.1:8472
```

## CI / CD

仓库通过 GitHub Actions 进行持续集成与发布，行为基准见 [`AGENTS.md`](AGENTS.md)。

- **CI**（`.github/workflows/ci.yml`）：推送 `main` 或开 PR 时运行
  `cargo fmt --check`、`cargo clippy -D warnings`、三平台（Linux/macOS/Windows）
  `cargo test`、`cargo deny check`、`cargo about` 与 MSRV(1.90) 校验。
- **Release**（`.github/workflows/release.yml`）：推送 `v*` 标签（或手动
  `workflow_dispatch`）时，在原生 runner 上构建 `lavacat` 二进制（Linux x86_64 /
  Windows x86_64），打包为 `tar.gz` 并附 `sha256`，连同 `dependencies.html`
  许可证清单一起发布为 GitHub Release。
- **Bench**（`.github/workflows/bench.yml`）：手动触发，跑管线性能基准并产出
  机器无关回归断言（resize 合帧、滚动零 emit、整形调用计数）。

### 发版步骤

```bash
# 1. 在 main 上完成改动并合并
# 2. 打标签并推送
git tag v0.1.0
git push origin v0.1.0
# 3. 流水线自动构建制品并创建 GitHub Release
```

## 文档

- [AGENTS.md](AGENTS.md) — 硬约束指南（分层、feature 拆分、验证流程）
- [docs/decisions/](docs/decisions/) — 架构决策记录（ADR）：crate 选型、JSRT、
  断行、剪贴板、URL、编码等
- [docs/plans/](docs/plans/) — 阶段计划与执行报告
- [docs/dependencies.html](docs/dependencies.html) — 第三方依赖与许可证清单
- 各 crate 模块文档内附**已知偏差清单**（相对规范的简化档）

## 贡献

见 [`CONTRIBUTING.md`](CONTRIBUTING.md)（环境准备、验证命令、提交格式与 PR 要求）；
参与即表示遵守 [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)。

## 许可

本项目以 [Apache-2.0](LICENSE) 许可发布。第三方依赖的许可证清单见
[docs/dependencies.html](docs/dependencies.html)。
