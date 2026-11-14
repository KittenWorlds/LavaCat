# 贡献指南

LavaCat 是用 Rust 从零构建的浏览器内核（解析 → DOM → 级联 → 布局 → 绘制 → 外壳）。
**本仓库的硬约束以 [`AGENTS.md`](AGENTS.md) 为准**，本文件是给外部贡献者的入口版：
动手之前先读 AGENTS.md 的「Hard Rules」，再回来看这里的流程。

## 动手之前

- **行为基准**：WHATWG 规范 > WPT 测试套件 > 成熟 crate > 自行实现。讨论「Chromium
  是这样」时请同时给出规范条款或 WPT 用例，Chromium 源码只作参考。
- **不造轮子优先**：已有成熟 crate 能覆盖的能力直接依赖；自研范围仅限选型 ADR 已确定的
  DOM / Cascade / 盒级布局三层（见 [`docs/decisions/`](docs/decisions/)）。
- **引入新依赖**：先确认许可证在 `deny.toml` 白名单内（含传递依赖），并在
  `docs/decisions/` 补一条 ADR，写清「为什么用它、为什么不是别的、边界在哪」。
- **渲染 / 交互改动**：外壳层没有无头模式，必须能在 `tools/acceptance/` 里复现并验收，
  单测证明不了上屏结果。

不确定的选择先开 issue 讨论，不要直接写一大坨再解释。

## 环境

- Rust stable，MSRV 见根 `Cargo.toml` 的 `rust-version`（当前 1.90）。
- Linux 需要 winit / wgpu 的系统库，完整清单见
  [`.github/workflows/ci.yml`](.github/workflows/ci.yml) 的 `apt-get install` 段。
- 排版回归需要 CJK 字体（CI 用 `fonts-noto-cjk`，本地一般系统自带）。

## 构建与验证

workspace 根目录：

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# 动过依赖时必跑（许可证白名单 + advisory）
cargo deny check
```

单个 crate 目录下（例如 `crates/lavacat-css/`）：

```bash
cargo check
cargo test --lib          # 只跑单元测试
cargo test --tests        # 只跑集成测试
```

动过 feature 拆分时，额外验证「逐个能力关闭」也能零 warning 编译：

```bash
cargo check -p lavacat-layout --no-default-features --features block
```

跑起来看效果：

```bash
cargo run -p lavacat-chrome --bin lavacat
node tools/acceptance/server.cjs        # 验收站，默认 http://127.0.0.1:8472
```

## 代码规则（摘要）

完整条款见 AGENTS.md「Hard Rules」，以下是最容易被 PR 卡住的几条：

- **unsafe**：facade crate（dom/html/css/layout/text/paint/network/chrome）一律
  `#![forbid(unsafe_code)]`，无例外；只有 runner 适配 crate（`lavacat-jsrt-*`）允许
  与引擎 C API 的 FFI 适配面，且每处附 SAFETY 注释。
- **依赖类型不得外泄**：html5ever / cssparser / taffy / parley / vello_cpu / reqwest 等
  外部类型不得出现在任何 `pub` 导出里（含 pub 字段、trait 方法签名、`pub use`），先包一层
  自己的类型。
- **feature 拆分**：一个 feature = 一个模块 = 一项能力，kebab-case 命名；`default` 包含
  全部已实现能力；模块声明与 `pub use` 必须挂 `#[cfg(feature = "...")]`。新增或删除模块时
  同步更新 AGENTS.md 的 Feature 映射表。
- **输入边界防护**：解析 / 布局 / 解码 / 网络都要有明确的体量或深度上限（例如树深
  `MAX_TREE_DEPTH`、长度 `MAX_CSS_LENGTH`、像素面积预检、响应体预算）。分配失败是 abort，
  `catch_unwind` 兜不住。
- **公共 API** 必须有 doc comment 并引用对应规范条款；模块文档写清已知偏差。
- **测试覆盖率** ≥ 80%，新增行为优先配失败用例再修。

## 提交

- 一个子任务一个 commit；`check` / `test` / `fmt` 通过后再提交。
- Message 格式：`[module] what + why`，例如
  `[tokenizer] adopt html5ever tokenizer, matches WHATWG §13.2.5.1`。
- 用 `git add <具体文件>` 提交，**禁止** `git commit -a`；**禁止** `git rebase -i` 压缩
  已完成的 commit。
- 改了逻辑（修 bug / 加特性 / 换行为）的 crate，把该 crate `Cargo.toml` 的 `version`
  patch +1；纯文档或注释改动不动版本号。

## 文档与记录

- 新依赖 → `docs/decisions/` 一条 ADR。
- 新增 / 删除模块 → 更新 AGENTS.md 的 Feature 映射表与架构树。
- `docs/` 下既有文件是历史记录，不改写内容；需要补充就新增文件并在 PR 里说明。
- 验收页面三处登记：`tools/acceptance/pages/index.html`（路由与判据）、
  `tools/acceptance/README.md`（回归清单表格），需要特殊响应头时再改 `server.cjs`。

## PR 流程

1. Fork 后从 `main` 开分支，PR 目标为 `main`。
2. PR 标题沿用 commit 格式，正文按
   [PR 模板](.github/PULL_REQUEST_TEMPLATE.md) 填写。
3. 行为改动请附：规范条款链接或 WPT 用例名、最小复现页面、必要时前后对照截图。
4. CI（fmt / clippy / test / deny / about / MSRV）全绿后，由维护者做语义比对再合并。

## 安全问题

不要在公开 issue 里贴可利用细节。请通过 GitHub 私信维护者
（[@Ink-dark](https://github.com/Ink-dark)）说明，修复并发布后再公开讨论。

## 许可证与行为准则

- 本仓库以 **Apache-2.0** 发布，向本仓库提交贡献即表示你同意该贡献以同一许可证授权。
- 参与即表示遵守 [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)。
