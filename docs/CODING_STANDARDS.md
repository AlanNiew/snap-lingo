# SnapLingo 编码约定

这份文档记录**这个仓库里已经踩过坑的约定**：每一条都对应一个真实的 bug 或一次 code review 结论。新增代码按这里写的做，就不用再重新发现一遍。行号会漂移，引用一律以符号名为准。

范围：`src-tauri/src/*.rs`（Rust 后端）、`src/**`（React 前端）、`docs/` 与 `README.md`。

---

## 1. 硬约束（改错了会出线上事故）

### 1.1 provider 标签是稳定契约，不是文案

`pipeline::provider_labels` 的返回值是 `(tag, model)`：

| tag | 含义 | 展示 |
| --- | --- | --- |
| `winocr` | Windows 内置 OCR | 前端映射为本地化标签 |
| `vision` | 视觉大模型 | 标签 + `model` 徽标 |
| `local` | 本地 OCR（Umi-OCR） | 标签，`model` 位置放 host |
| `image` | 一步图片翻译（厂商直接返回译文） | 标签 |

- **tag 永远不本地化、不拼人话**，它就是 `ResultPayload.ocr_provider` 的取值，前端在 `src/result.tsx` 的 `OCR_TAGS` 里映射到 i18n key。
- 模型名走 `payload.model`，单独渲染成一个徽标；不要在 tag 里拼 `vision:model` 这种东西（历史 bug：换语言后标签炸了，`docs/GUIDE.md` 也解释不了）。
- 新增识别渠道：`provider_labels` 加一个分支 + `result.tsx` 的 `OCR_TAGS` 加一条 + `i18n.ts` 加一条 label key，三处缺一不可。
- 「重试」（`pipeline::retranslate`）也必须走 `provider_labels`，不要复用第一次的旧值，也不要在图片路径里写死字符串。

### 1.2 日志里不许出现密钥或未脱敏的 URL

- 任何打印 URL / 请求 / `reqwest::Error` 的日志，必须过 `providers::scrub_url` 或 `providers::redact_error`。`reqwest::Error` 的 `Display` 自带 URL，直接 `log::warn!("{err}")` 就等于把 `?key=...` 写进 `snaplingo.log`。
- `scrub_url` 会把 `key` / `token` / `sign` / `signature` / `apikey` / `access_token` 这类 query 参数的值替换成 `***`；`q`（翻译文本）不算凭据，不要加进名单——它是有用的调试信息。
- API key 走请求头而不是 URL（Google 用 `X-Goog-Api-Key`，见 `providers::google_url` / `providers::google`）；新增渠道时优先查厂商文档是否支持 header 鉴权。
- 失败时记录响应体是**有意的**（README 承诺过），但响应体也要先脱敏再进日志——厂商在 4xx 里回显请求是常见行为。
- 密钥只允许来自 `AppConfig`，禁止硬编码、禁止写进测试夹具。

### 1.3 坐标只有一种单位：帧物理像素

- `state::SessionEntry` 里的 rect 一律是**冻结帧的物理像素**。
- 缩放只发生在 overlay 侧：`frame.width / innerWidth`（`src/overlay.tsx`）。Rust 侧只做 `frame_rect` 的 clamp，**不要再乘一次 scale**（历史 bug：双重缩放导致截图跑偏）。
- 单屏模式（`ui.capture_all_monitors = false`）截的是**鼠标所在显示器**，不是主显示器；`capture::capture` 用 `Monitor::from_point` 拿指针坐标，失败才回退主屏。`primary` 这种命名已改成 `focus_id`——它表示「要给焦点的覆盖层」，不是「主屏」。
- 传给 xcap 的坐标是物理像素（`Monitor::from_point` 走 Win32 `MonitorFromPoint`）。

### 1.4 每一层网络调用都必须有硬超时

`providers.rs` 顶部常量是唯一来源：

| 常量 | 值 | 说明 |
| --- | --- | --- |
| `CONNECT_TIMEOUT` | 12s | 建连 |
| `REQUEST_TIMEOUT` | 60s | 单次请求 |
| `MAX_ATTEMPTS` | 4 | 即「重试 3 次（共 4 次）」，文档措辞以此为准 |
| `CHUNK_CHARS` | 2400 | 长文本分片 |

- `pipeline.rs` 的阶段超时（`OVERLAY_READY_TIMEOUT` / `CAPTURE_TIMEOUT` / `OCR_TIMEOUT` / `VISION_TIMEOUT` / `TRANSLATE_TIMEOUT` / `IMAGE_TRANSLATE_TIMEOUT` / `JOB_TIMEOUT`）同理，新增阶段必须自带 watchdog，不能让 UI 无限等待。
- 改任何一个数字，**必须同步 `README.md` 与 `docs/GUIDE.md` 的对应段落**（这两个文件是用户可见的承诺）。

### 1.5 配置兼容性只增不减

- 新字段必须 `#[serde(default)]` 或实现 `Default`，老配置文件能被反序列化。
- 取值迁移写进 `config::AppConfig::sanitize`（例如旧值 `youdao-llm` → `youdao`、扁平凭据 → `profiles`）；`sanitize` 是唯一允许「修数据」的地方，命令里不要就地补丁。
- 配置写入必须原子：临时文件 + rename，解析失败保留 `.bak`（`config::ConfigStore`）。
- 凭据折平只允许走 `providers::scoped_ocr` / `providers::scoped_translate`，不要在调用点手写 `let mut cfg = ...` 再逐个覆盖字段（历史问题：三处各写一遍，漏一处就用错厂商）。

### 1.6 主题白名单四处同步

主题不是数据，是四处写死的清单，改一处必须改四处：

1. `src-tauri/src/config.rs` 里对 `ui.theme` 的合法性判断
2. `src/lib/theme.ts` 的 `THEMES`
3. `src/styles.css` 的 `[data-theme="..."]` 段
4. `src/lib/i18n.ts` 的主题名文案（zh / en 各一条）

### 1.7 窗口生命周期：关闭 = 隐藏

- 除「退出应用」外，所有窗口的 `CloseRequested` 只 `hide()`，不销毁；`lib.rs` 已统一处理，新增窗口要接到同一分支。
- 显示窗口前先 `show()` 之后再 `set_focus`，避免 WebView 首帧未就绪导致的闪白/空白（历史 bug）。
- 结果窗口尺寸：`state.result_size` 只记录**用户拖拽**的尺寸，`ui.auto_fit` 开启时由 `fit_result_window` 改高度，**不得**写入记忆值。
- 新窗口一律通过 `windows.rs` 的 helper 创建（label 常量集中在那里），不要散落 `WebviewWindowBuilder`。

---

## 2. 前端约定

- **所有用户可见字符串都在 `src/lib/i18n.ts` 的 `STRINGS` 里**（`zh` / `en` 两份，`StringKey = keyof typeof STRINGS` 保证类型安全）。组件里禁止硬编码中文或英文文案。
- 后端返回的错误提示目前**只有中文短句**（这是现状，不是理想态）：前端原样展示即可，不要尝试翻译；新增后端错误请写中文、不带 markdown、不拼英文。
- `invoke` 只能通过 `src/lib/api.ts` 的包装调用，不要在组件里写原始命令名；事件名集中在 `EVENTS`。
- 数据驱动优先：识别引擎字段、服务商预设这类结构化内容放 `src/lib/labels.ts` / `presets.ts`，不要写成组件里的 `if/else` 分支。
- 能用 CSS 变量/`data-theme` 解决的样式不要加 React state。
- 自适应内容（`ui.auto_fit`）与用户拖拽是互斥的两种尺寸来源，改动前先读 `windows::place_result` 与 `commands::fit_result_window`。

---

## 3. 命令契约

后端 `#[tauri::command]` 与前端 `api.ts` 的 `invoke<T>` **必须一一对应**（当前 42 / 42）。新增命令三步：

1. `src-tauri/src/commands.rs` 写命令（`State<'_, AppState>` + 返回 `Result<T, String>`）；
2. `src-tauri/src/lib.rs` 的 `generate_handler!` 注册；
3. `src/lib/api.ts` 加带泛型的包装。

改完数一下两边数量是否仍然相等。

---

## 4. 文档同步清单

改动以下内容时，`README.md` 与 `docs/GUIDE.md` 必须跟着改（用户把它们当合同）：

- 默认值（快捷键、识别引擎、翻译渠道、目标语言）
- 渠道 / 引擎 / 预设数量，后端命令数量
- 所有超时、重试次数、日志上限（512 KB / 256 KB）、图片尺寸上限
- 窗口行为（尺寸记忆、自适应、关闭即隐藏）
- 鉴权方式（key 走 header 还是 URL）

GUIDE 里描述的行为必须能在代码里指到具体函数；写不出来就说明文档过时了。

---

## 5. 提交与验证

提交信息格式：

```
type(scope): 中文一句话

正文写「为什么」，不要复述 diff；一个提交只做一件事，且必须能独立编译。
```

`type` 取 `fix` / `feat` / `refactor` / `perf` / `docs` / `chore`。

提交前必须全绿：

```powershell
cargo check --manifest-path src-tauri/Cargo.toml
cargo test  --manifest-path src-tauri/Cargo.toml
npx tsc --noEmit
# 涉及界面/打包再跑
npm run build
```

涉及发版时额外确认 `Cargo.lock` 里的版本号与 `Cargo.toml` / `package.json` / `tauri.conf.json` 一致（否则 `cargo build --locked` 会失败）。

**提交前自审清单**（就是这次 code review 的检查项）：

- [ ] provider tag 还是稳定 tag 吗？模型名是否走 `model`？
- [ ] 新增日志是否经过 `scrub_url` / `redact_error`？
- [ ] 超时/重试/命令数/渠道数的文档是否同步？
- [ ] 主题、引擎、渠道白名单是否四处同步？
- [ ] 注释是否与代码一致（尤其坐标单位、窗口行为）？
- [ ] 新增的纯逻辑是否有单测？

---

## 6. 测试策略

- **Rust**：单测放在同文件 `#[cfg(test)] mod tests`，优先覆盖纯逻辑——配置迁移（`sanitize`）、坐标 clamp、provider 标签、URL 脱敏、分片、预设完整性。`cargo test` 必须保持全绿。
- **Tauri / 窗口 / 网络 / 多屏 DPI**：不做单测，靠真机验证，结论写进提交信息或 PR 描述。
- **前端**：目前没有测试框架，因此把可测逻辑推到 `src/lib/` 下的纯函数；真要补测试再单独引入 vitest，不要为了测试把 UI 拆碎。

---

## 7. 明确延后 / 需要单独 PR 的事

- **provider 全链路裸 `String` → enum（Primitive Obsession）**：要动配置序列化、22 个预设、前后端契约，收益大但风险高，必须单开 PR 并用测试先行的方式做。
- **后端用户可见文案 i18n**：几十条 Rust 错误需要抽 key，属于独立工程。
- **新增第三方依赖**：先讨论，理由与替代方案写进 PR 描述。
