## 概要

<!-- 改了什么、为什么改。一段话即可，评审细节放下面的小节 -->

## 关联

<!-- Closes #123 / 相关 ADR、路线图条目 -->

## 改动类型

- [ ] 修 bug
- [ ] 新特性 / 规范能力补齐
- [ ] 性能
- [ ] 重构（行为不变）
- [ ] 文档 / 工程（CI、依赖、模板）

## 影响面

- [ ] 公共 API 变化（在下方列出签名变更）
- [ ] feature 拆分变化（已同步 AGENTS.md 的 Feature 映射表）
- [ ] 新增依赖（`docs/decisions/` 已加 ADR，`deny.toml` 许可证白名单已核对）
- [ ] 改逻辑的 crate 已把 `Cargo.toml` 的 `version` patch +1
- [ ] 无

## 验证

- [ ] `cargo check --workspace`
- [ ] `cargo test --workspace`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo deny check`（动过依赖时必跑）
- [ ] `cargo check -p <crate> --no-default-features --features <...>`（动过 feature 时）
- [ ] 语义比对（WPT 输出 / 规范条款逐条对照）—— 由架构师执行后勾选

## 规范依据

<!-- 条款链接、WPT 用例名、必要的其它内核对照。没有规范依据的行为改动请说明理由 -->

## 实机验收（外壳 / 渲染 / 输入 / 脚本改动必填）

浏览器外壳没有无头模式，上屏结果只能实机验收：

- [ ] 已新增或更新 `tools/acceptance/` 页面，并在三处登记
      （`pages/index.html`、`README.md` 回归清单、必要时 `server.cjs`）
- 验收路径与判据：

## 已知偏差 / 遗留

<!-- 本次改动引入或保留的偏差、未覆盖的边界、后续任务。没有就写「无」 -->
