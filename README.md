# SnapLingo · 截图翻译

按下快捷键 → 框选屏幕上的任意文字 → 自动 OCR → 自动翻译 → 结果卡片。

SnapLingo 是一个 Windows 桌面小工具，常驻托盘，**启动快、内存占用低**（Tauri 2 + WebView2，无 Electron）。
默认走「Windows 内置 OCR（离线、免费、不需要 Key）+ 你自己的 OpenAI 兼容翻译接口」，开箱即可用；
也可以在设置里切换成「视觉大模型 OCR」，用一条请求同时拿到原文和译文。

---

## 功能

| 功能 | 说明 |
| --- | --- |
| 全局快捷键 | 默认 `Ctrl + Alt + Z`，可在设置中重新录制 |
| 多显示器框选 | 触发时**先冻结全部屏幕画面**再弹出选区，因此不会截到选区窗口自身，也不会因为背景刷新而错位 |
| 拖拽即识别 | 松开鼠标即开始识别；`Esc` 或右键取消；选区小于 4px 自动忽略；**再按一次快捷键**或托盘「取消本次截图」同样能取消 |
| 出错处理 | 任何失败都会**弹出结果窗口并同时发一条系统通知**说明原因，结果窗口内置「重试」；网络请求失败自动重试 3 次，绝不静默失败 |
| 防卡死 | 每个耗时步骤都有硬超时（截图 15s、识别 30s、翻译 120s、整体 150s）；框选 3 分钟未确认自动取消；若真的卡住（连续 4 分钟无响应）程序会自行退出，**绝不会让选区窗口把桌面锁死** |
| OCR | ① Windows 内置 OCR（`Windows.Media.Ocr`，离线、免费）② 视觉大模型（OpenAI 兼容 `/chat/completions`，图片以 base64 传入） |
| 翻译 | ① OpenAI 兼容接口 ② DeepL（Free / Pro）③ 关闭（只 OCR 不翻译） |
| 结果窗口 | 无边框置顶卡片，自动按内容调整高度；复制原文/译文、重新翻译、`Esc` 关闭、失焦自动关闭（可关） |
| 历史记录 | 本地保存最近 N 条识别结果，可搜索查看、单条删除或清空 |
| 其它 | 开机自启、深浅色主题、中/英文界面、结果字号、自动复制、自定义 Prompt、API Key 一键探测/模型列表拉取 |

---

## 系统要求

- **Windows 10 1809+ / Windows 11（x64）**
- **WebView2 运行时**：Windows 11 自带；Windows 10 一般由 Edge 提供。若提示缺少，安装 [Evergreen WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。
- **OCR 语言包**（仅当使用 Windows 内置 OCR 时）：
  「设置 → 时间和语言 → 语言和区域 → 添加语言」，在对应语言的“可选语言功能”里勾选**光学字符识别**。
  中文简体通常随系统一起安装。设置页里的「检测 OCR 环境」按钮会列出当前系统已安装的识别语言。
- **构建环境**：Node.js 20+/22+、Rust stable（MSVC 工具链）、Visual Studio Build Tools（“使用 C++ 的桌面开发”）。

---

## 开发与构建

```bash
cd snap-lingo

# 安装依赖（npm 12 会拦截 postinstall，需放行 esbuild 的安装脚本）
npm install
npm install-scripts approve esbuild     # 仅 npm 12+ 需要；旧版本可跳过

# 开发模式（热更新，首次编译 Rust 需要几分钟）
npm run tauri dev

# 仅前端类型检查 + 构建
npm run build

# 打包成安装包（NSIS，输出在 src-tauri/target/release/bundle/nsis/）
npm run tauri build
```

> Rust 侧依赖（`xcap`、`image`、`reqwest`、若干 Tauri 插件）首次 `cargo` 编译需要联网下载。

---

## 使用

1. 启动后程序只存在于**系统托盘**（没有主窗口）。
2. 右键托盘图标 → **设置**，至少配置一项：
   - 只想离线使用：`OCR = Windows 内置 OCR`，`翻译 = OpenAI 兼容`（填入你的接口地址、Key、模型）。
   - 想一条请求搞定：`OCR = 视觉大模型`，填好 OpenAI 兼容的地址/Key/模型；若开启 **一次调用**，则同一次请求同时返回原文与译文。
   - 完全离线：`翻译 = 关闭`，只做 OCR。
3. 用 `Ctrl + Alt + Z`、或托盘菜单「截图翻译」、或托盘图标左键单击触发截图。
4. 拖拽框选文字区域，松开鼠标开始识别；`Esc` / 右键取消，再按一次快捷键也能取消。
5. 结果窗口出现后可：复制、重新翻译、`Esc` 关闭。识别或翻译失败时，窗口会显示原因和「重试」按钮（重试会复用上一次的截图，不必重新框选）。
6. 托盘菜单里的「取消本次截图」和「退出 SnapLingo」在**任何时刻**都可用；退出时若 2.5 秒内没有正常结束，进程会被强制结束，不会留下关不掉的窗口。

---

## 配置说明

配置文件位于 **`%APPDATA%\SnapLingo\config.json`**（设置页底部「打开配置目录」可直接打开）。

```jsonc
{
  "shortcut": "Ctrl+Alt+Z",
  "ocr": {
    "provider": "winocr",            // winocr | vision
    "baseUrl": "https://integrate.api.nvidia.com/v1",
    "apiKey": "",
    "model": "",
    "language": "",                  // 空 = 跟随系统；如 "zh-Hans"、"en-US"
    "oneShot": false,                // vision：一次调用同时返回原文和译文
    "prompt": ""
  },
  "translate": {
    "provider": "openai",            // openai | deepl | none
    "baseUrl": "https://integrate.api.nvidia.com/v1",
    "apiKey": "",
    "model": "nvidia/nemotron-3-ultra-550b-a55b",
    "targetLang": "zh-CN",
    "deeplPlan": "free",             // free | pro
    "prompt": ""
  },
  "capture": {
    "allMonitors": true,             // false = 只截鼠标所在显示器
    "upscaleSmall": true,            // 小图自动放大，提高识别率
    "maxImageSide": 1568,            // 送入模型的图片长边上限
    "jpegQuality": 88
  },
  "ui": {
    "autoCopy": false,               // 出结果自动复制译文
    "showOriginal": true,            // 结果卡片里显示原文
    "closeOnBlur": false,            // 点别处自动关闭结果窗口
    "resultFontSize": 16,
    "theme": "system",               // system | light | dark
    "language": "zh"                 // zh | en
  },
  "startup": false,                  // 开机自启
  "history": { "enabled": true, "limit": 100 }
}
```

内置预设服务商（设置页可一键填充）：NVIDIA NIM、智谱 GLM、DeepSeek、阿里云百炼、OpenAI、硅基流动、Kimi、OpenRouter、Ollama、LM Studio —— 全部走 OpenAI 兼容协议。

### 关于 API Key 的安全说明

API Key **以明文**保存在上述 `config.json` 中，仅依赖当前用户目录的访问控制（ACL）保护。
这不是加密存储：能读取当前用户文件的程序都可以读到它。请勿在共享电脑上保存长期密钥。

其它文件（同一目录）：

- `SnapLingo/snaplingo.log` —— 运行日志，超过 512KB 会在下次启动时清空。
- `SnapLingo/history.json` —— 历史记录；设置里可清空或关闭。
- `SnapLingo/config.json.bak` —— 当 `config.json` 解析失败时自动备份的坏文件。

---

## 技术链路

```
全局快捷键 / 托盘
      │
      ▼
capture（xcap + Windows Graphics Capture）
  先抓取全部显示器的完整帧，缓存在内存
      │
      ▼
为每块显示器创建一个无边框置顶 overlay 窗口，
把**冻结帧**铺满显示，前端只负责画蒙版和取框
      │
      ▼  user release → finish_selection(物理像素坐标)
从缓存帧裁切选区 → 图像预处理
  · 小图放大（长边目标 1400px，最多 3×，Lanczos3）
  · 大图降采样（长边上限 maxImageSide，CatmullRom）
  · 转 PNG（超 3.5MB 转 JPEG）→ base64
      │
      ▼
OCR：Windows.Media.Ocr（BGRA8 位图，离线）
     或 视觉大模型（image_url 传入，可一次返回译文）
      │
      ▼
翻译：OpenAI 兼容 / DeepL（长文本按 ~2400 字符分块，
每个请求最多重试 3 次，自动剥离模型的思考过程与代码块）
      │
      ▼
结果窗口（订阅 pipeline:stage / result:update 事件）
```

两条贯穿全链路的设计约束：

- **主线程永不阻塞**：截图、编码、OCR、网络全部跑在阻塞线程池或异步运行时；涉及窗口的命令都是 `async`。
- **失败必须被看见**：任何一步出错都会变成一张带原因的结果卡片 + 一条系统通知，并写进日志 —— 后台程序静默失败和坏掉是无法区分的。

启动后结果窗口与主显示器的选区窗口会被**预先创建并复用**，因此真正截图时窗口只需显示/隐藏/移动，不再创建 WebView（创建 WebView 是唯一可能让主线程卡住的昂贵操作）。

## 项目结构

```
snap-lingo/
├─ index.html / overlay.html / result.html   三个页面入口（Vite 多页）
├─ src/
│  ├─ main.tsx                  设置页入口
│  ├─ overlay.tsx               框选层（每显示器一个窗口）
│  ├─ result.tsx                结果卡片
│  ├─ styles.css                全部样式（含深浅色变量）
│  ├─ lib/
│  │  ├─ api.ts                 invoke / event 封装，与 Rust 命令一一对应
│  │  ├─ types.ts               与 Rust 结构体对齐的 TS 类型
│  │  ├─ i18n.ts                中英文字符串表
│  │  └─ util.ts                小工具（快捷键解析、复制、格式化…）
│  └─ settings/                 设置界面（分区组件、表单控件、预设）
└─ src-tauri/
   ├─ Cargo.toml
   ├─ tauri.conf.json           Tauri 配置（无默认窗口、NSIS 打包）
   ├─ capabilities/default.json 前端权限（core:default + 窗口拖动）
   ├─ icons/                    应用与托盘图标
   └─ src/
      ├─ lib.rs                 插件注册、托盘、快捷键、窗口事件、命令表
      ├─ main.rs                入口
      ├─ config.rs              配置结构与读写（原子写入）
      ├─ model.rs               SelectionRect / ResultPayload / Stage …
      ├─ capture.rs             xcap 截图与显示器信息
      ├─ imageops.rs            裁切、缩放、编码、预览
      ├─ ocr_win.rs             Windows.Media.Ocr 封装
      ├─ providers.rs           OpenAI 兼容 / DeepL 客户端与重试
      ├─ pipeline.rs            截图 → 识别 → 翻译 → 发布的编排
      ├─ commands.rs            暴露给前端的 25 个命令
      ├─ windows.rs             各窗口的创建与显隐
      ├─ tray.rs                托盘图标与菜单
      └─ hotkey.rs              全局快捷键注册
```

前端只使用 `core:default` + `core:window:allow-start-dragging` 权限（`invoke`、事件监听、拖动无边框结果窗口），所有网络与系统调用都在 Rust 侧完成。

---

## 常见问题

**按下快捷键没反应**
快捷键被其它程序占用了。打开设置 → 通用 → 点击快捷键录制框重新录制一个组合；保存失败时会自动回滚并提示。

**识别结果是空的 / 乱码**
框选范围太小或文字太细。勾选「小图自动放大」，并把区域框得稍大一些（包含完整行）。中文竖排或艺术字识别率会下降。

**提示缺少 OCR 语言包**
系统没有安装对应语言的识别功能，按上文「系统要求」里的步骤添加语言并勾选光学字符识别。

**翻译报错 401 / 403**
接口地址或 Key 不对。设置页里点「测试连接」可快速定位；注意接口地址需要包含 `/v1` 这类版本前缀（如 `https://xxx/v1`）。

**翻译特别慢或超时**
单个请求超时 60 秒、最多重试 3 次（429 / 5xx / 网络错误），整体任务超过 150 秒会中止并提示「重试」。长文本会自动分块，代理/中转服务建议换成响应更快的模型。

**程序好像卡住了 / 选区窗口关不掉**
SnapLingo 有多层兜底，正常情况下不会卡住：
1. 再按一次快捷键，或托盘菜单「取消本次截图」；
2. 什么都不做，3 分钟后选区会自行取消；
3. 若主线程真的无响应（连上面两步都失效），程序会在 4 分钟后自动退出，请在托盘重新启动。

每次失败都会写入 `%APPDATA%\SnapLingo\snaplingo.log`，反馈问题时附上该文件即可。

**多显示器下选区错位**
SnapLingo 按显示器的物理像素创建窗口与换算坐标，支持不同的缩放比例（DPI）混合环境。如遇错位，请在设置里把窗口模式改为「只截鼠标所在显示器」并反馈日志。

---

## License

MIT
