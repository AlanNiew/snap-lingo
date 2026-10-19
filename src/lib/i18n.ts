/**
 * Tiny two-language dictionary.
 *
 * `[简体中文, English]`, with `{name}` placeholders. Everything user-visible
 * lives here so the language switch in 设置 → 外观 is honest: no half-English
 * screens, no untranslated leftovers.
 */

export type Lang = "zh" | "en";

const STRINGS = {
  /* shell */
  appName: ["SnapLingo", "SnapLingo"],
  settings: ["设置", "Settings"],

  /* tabs */
  tabGeneral: ["通用", "General"],
  tabOcr: ["文字识别", "Text recognition"],
  tabTranslate: ["翻译", "Translation"],
  tabCapture: ["截图", "Capture"],
  tabAppearance: ["外观", "Appearance"],
  tabHistory: ["历史", "History"],
  tabLogs: ["日志", "Logs"],
  tabAbout: ["关于", "About"],

  /* general */
  shortcutTitle: ["全局快捷键", "Global shortcut"],
  shortcutHint: [
    "在任意程序中按下即可开始框选截图。点击右侧输入框后按下组合键即可更换。",
    "Works in any app. Click the box and press a key combination to change it.",
  ],
  shortcutRecording: ["请按下组合键…（Esc 取消）", "Press a combination… (Esc to cancel)"],
  shortcutRecord: ["录入", "Record"],
  shortcutInvalid: ["无法注册该快捷键，可能是已被其他程序占用。", "Could not register: another app may own it."],
  captureKeyLabel: ["截图快捷键", "Capture key"],
  startupTitle: ["开机自动启动", "Start with Windows"],
  startupHint: ["登录后常驻托盘，按快捷键即可截图。", "Stays in the tray and is ready for the hotkey."],
  captureModeTitle: ["截图范围", "Capture scope"],
  captureAllMonitors: ["所有显示器", "All monitors"],
  captureAllMonitorsHint: [
    "每个显示器都会出现一个框选层，可在任意一块屏幕上选取。",
    "Every monitor gets its own selection layer.",
  ],
  capturePrimaryOnly: ["仅主显示器", "Primary monitor only"],
  trayTitle: ["托盘菜单", "Tray menu"],
  trayHint: [
    "右键托盘图标可以截图、打开设置或退出程序。关闭窗口不会退出程序。",
    "Right-click the tray icon to capture, open settings, or quit. Closing a window keeps the app running.",
  ],
  testCapture: ["立即截图", "Capture now"],

  /* ocr */
  ocrEngineTitle: ["识别引擎", "Recognition engine"],
  ocrWin: ["Windows 离线 OCR", "Windows offline OCR"],
  ocrWinDesc: [
    "使用系统自带的文字识别，完全离线、无需密钥、启动即用。适合常规文档与界面文字。",
    "Built into Windows. Offline, no API key, works immediately. Good for documents and UI text.",
  ],
  ocrVision: ["视觉大模型", "Vision model"],
  ocrVisionDesc: [
    "把图片发给多模态模型，对艺术字、屏幕截图、复杂排版识别更准。需要一个 OpenAI 兼容接口。",
    "Sends the image to a multimodal model. Better on stylised text and complex layouts. Needs an OpenAI-compatible endpoint.",
  ],
  ocrLocal: ["本地 OCR 服务", "Local OCR service"],
  ocrLocalDesc: [
    "连接本机运行的离线识别服务（Umi-OCR 及同类服务）。模型跑在自己电脑上，图片不出本机，体积小、可随时关掉。需先在服务里开启 HTTP 接口。",
    "Talks to an offline recognition service running on this machine (Umi-OCR and compatible servers). The model runs locally, the picture never leaves the machine. Enable its HTTP interface first.",
  ],
  ocrPicture: ["由翻译渠道完成（一步图片翻译）", "Handled by the translation service (one step)"],
  ocrPictureDesc: [
    "把图片直接交给翻译渠道，识别与翻译一次完成，不使用本机 OCR。需要有道 / 百度 / 腾讯的「图片翻译」服务（在各自控制台单独申请）。",
    "Sends the picture straight to the translation service, which reads and translates it in one call. Requires the picture-translation service of Youdao / Baidu / Tencent (applied for separately in their consoles).",
  ],
  ocrLanguage: ["识别语言", "Recognition language"],
  ocrLanguageAuto: ["跟随系统默认", "System default"],
  ocrLanguageHint: [
    "选择与截图文字一致的语言能明显提高识别率。缺失的语言包可在 Windows「语言和区域」中添加「光学字符识别」可选功能。",
    "Matching the screenshot language improves accuracy a lot. Missing packs can be added in Windows 语言和区域 → 可选功能.",
  ],
  ocrInstalled: ["已安装的语言", "Installed languages"],
  probeOcr: ["检测离线引擎", "Test offline engine"],
  probeOcrRunning: ["正在检测…", "Testing…"],
  probeLocal: ["检测本地服务", "Test local service"],
  ocrLocalUrl: ["本地服务地址", "Service address"],
  ocrLocalUrlHint: [
    "Umi-OCR 默认监听 http://127.0.0.1:1224（在其「全局设置 → 接口」里开启并查看端口）。",
    "Umi-OCR listens on http://127.0.0.1:1224 by default (enable it in 全局设置 → 接口).",
  ],
  ocrLocalGet: ["下载 Umi-OCR", "Download Umi-OCR"],
  ocrPictureCurrent: ["当前渠道：{name}", "Current service: {name}"],
  ocrPictureUnavailable: [
    "当前翻译渠道（{name}）不能读取图片。请到「翻译」页改选有道 / 百度 / 腾讯，或把识别引擎换成其它选项。",
    "The selected translation service ({name}) cannot read a picture. Pick Youdao / Baidu / Tencent on the 翻译 page, or choose another recognition engine.",
  ],
  ocrPictureFallback: [
    "该服务未开通图片翻译时，本次截图会自动回退到本机识别 + 文本翻译。",
    "If the picture service is not enabled, the capture falls back to local OCR plus text translation.",
  ],
  ocrPictureGoTranslate: ["去设置翻译渠道", "Open translation settings"],

  /* endpoints */
  endpointBaseUrl: ["接口地址", "Base URL"],
  endpointBaseUrlHint: [
    "OpenAI 兼容接口，需以 /v1 结尾，例如 https://api.openai.com/v1。",
    "OpenAI-compatible endpoint ending in /v1.",
  ],
  endpointKey: ["API 密钥", "API key"],
  endpointKeyHint: [
    "仅保存在本机的配置文件中（明文）。留空表示不需要鉴权。",
    "Stored in plain text in this machine's config file. Leave empty when no auth is needed.",
  ],
  endpointModel: ["模型", "Model"],
  endpointModelHint: ["可手动填写，也可以先拉取列表再选择。", "Type it, or fetch the list and pick."],
  fetchModels: ["拉取模型列表", "Fetch models"],
  fetchingModels: ["拉取中…", "Fetching…"],
  testConnection: ["测试连接", "Test connection"],
  testing: ["测试中…", "Testing…"],
  showKey: ["显示", "Show"],
  hideKey: ["隐藏", "Hide"],

  /* presets */
  presetTitle: ["快速填充", "Presets"],
  presetHint: [
    "一键填入常用服务商的接口地址与模型，密钥仍需自行填写。",
    "Fills the endpoint and model for a known provider. You still supply the key.",
  ],
  applyUrl: ["官方申请地址", "Get credentials"],
  applyUrlOpen: ["打开申请页面", "Open sign-up page"],
  presetCustom: ["自定义 / 已手动填写", "Custom / filled in"],
  channel: ["渠道", "Service"],

  /* translate */
  translateProvider: ["翻译服务", "Translation service"],
  translateNone: ["仅识别不翻译", "Recognise only"],
  translateNoneDesc: [
    "只提取文字，不做翻译。适合只想复制原文的场景。",
    "Extracts text without translating.",
  ],
  translateOpenai: ["OpenAI 兼容接口", "OpenAI-compatible"],
  translateOpenaiDesc: [
    "任何兼容 /chat/completions 的服务：OpenAI、DeepSeek、智谱、通义、NVIDIA NIM、Ollama、LM Studio 等。",
    "Any /chat/completions endpoint: OpenAI, DeepSeek, Zhipu, Qwen, NVIDIA NIM, Ollama, LM Studio…",
  ],
  translateDeepl: ["DeepL", "DeepL"],
  translateDeeplDesc: [
    "翻译质量稳定，适合欧洲语言互译。需要 DeepL 的 API 密钥。",
    "Very good on European languages. Needs a DeepL API key.",
  ],
  deeplPlan: ["账号类型", "Account type"],
  deeplFree: ["免费版 (api-free.deepl.com)", "Free (api-free.deepl.com)"],
  deeplPro: ["专业版 (api.deepl.com)", "Pro (api.deepl.com)"],
  deeplKeyHint: [
    "免费版密钥以 :fx 结尾，可在 DeepL 账号页面获取。",
    "Free keys end with :fx and are on your DeepL account page.",
  ],
  translateMicrosoft: ["微软翻译 (Azure)", "Microsoft Translator (Azure)"],
  translateMicrosoftDesc: [
    "Azure 认知服务翻译，全球可用，有免费额度。需要密钥与区域。",
    "Azure Cognitive Services Translator. Free tier available. Needs a key and a region.",
  ],
  translateGoogle: ["谷歌翻译", "Google Translate"],
  translateGoogleDesc: [
    "Google Cloud Translation v2，语言覆盖最广。密钥随请求以 URL 参数发送，建议在 Google Cloud 中限制该密钥。",
    "Google Cloud Translation v2, widest language coverage. The key travels as a URL parameter, so restrict it in Google Cloud.",
  ],
  translateYoudao: ["有道智云", "Youdao"],
  translateYoudaoDesc: [
    "有道文本翻译 v3，按量计费，中文语料好。",
    "Youdao text translation v3. Pay as you go, strong on Chinese.",
  ],
  translateBaidu: ["百度翻译", "Baidu Translate"],
  translateBaiduDesc: [
    "百度翻译开放平台（通用文本翻译），有免费额度。",
    "Baidu Translate open platform (general text), with a free quota.",
  ],
  translateTencent: ["腾讯云翻译 (TMT)", "Tencent Cloud TMT"],
  translateTencentDesc: [
    "腾讯云机器翻译，每月 500 万字符免费额度。",
    "Tencent Cloud machine translation. 5M characters per month free.",
  ],
  secretYoudao: ["应用密钥 (App Secret)", "App secret"],
  secretBaidu: ["密钥 (Secret Key)", "Secret key"],
  secretTencent: ["Secret Key", "Secret key"],
  secretHint: [
    "与 API Key 一起在有道 / 百度 / 腾讯云控制台获取，用于签名。",
    "Issued next to the API key in the Youdao / Baidu / Tencent console; used for signing.",
  ],
  region: ["区域 (Region)", "Region"],
  regionHint: [
    "Azure 密钥所属区域，例如 eastasia；腾讯云可留空。",
    "Region of the Azure key, e.g. eastasia. Leave empty for Tencent.",
  ],
  imageTranslate: ["一步翻译图片", "Translate the picture in one call"],
  translatePictureNote: [
    "这个渠道支持「一步图片翻译」：可在「识别」页把引擎切换为它，识别与翻译一次完成。",
    "This service can read the picture itself — pick it on the 识别 page to try it.",
  ],
  tierTitle: ["服务版本", "Service tier"],
  tierStandard: ["通用翻译", "Standard"],
  tierLlm: ["大模型翻译", "LLM"],
  tierLite: ["轻量版", "Lite"],
  tierLlmPro: ["大模型 · 子曰 Pro", "LLM · 子曰 Pro"],
  tierLlmLite: ["大模型 · 子曰 Lite", "LLM · 子曰 Lite"],
  tierHint: [
    "大模型版本通常需要在服务商控制台单独开通；开通前可先用通用版本。",
    "LLM tiers usually have to be enabled in the provider console first.",
  ],
  authTitle: ["鉴权方式", "Authentication"],
  authSign: ["签名（APPID + 密钥）", "Signature (APPID + secret)"],
  authKey: ["API Key（Bearer）", "API Key (Bearer)"],
  authHint: [
    "百度大模型翻译两种方式都支持；API Key 在百度控制台的「API Key 管理」里创建。",
    "Baidu's LLM endpoint accepts both; create the API Key under 密钥管理 in the Baidu console.",
  ],
  bearerTitle: ["API Key", "API Key"],
  bearerHint: [
    "百度控制台创建的 API Key，放在请求头里，不需要再算签名。",
    "Created in the Baidu console and sent as a header instead of a signature.",
  ],
  badgeImageTranslate: ["图片翻译", "Picture translation"],
  ocrBypassedNote: [
    "本次识别与翻译由翻译渠道一次完成，不使用本机 OCR 引擎。",
    "Recognition and translation were done by the translation service in one call; the local OCR engine was not used.",
  ],
  targetLang: ["目标语言", "Target language"],
  targetLangHint: ["识别出的文字会被翻译成该语言。", "Recognised text is translated into this language."],
  advanced: ["高级选项", "Advanced"],
  customPrompt: ["自定义提示词", "Custom prompt"],
  customPromptHint: [
    "留空使用内置提示词。可在此约定术语、语气或输出格式。",
    "Leave empty for the built-in prompt. Use it for terminology, tone, or output format.",
  ],
  oneShot: ["一次调用完成识别与翻译", "Recognise and translate in one call"],
  oneShotHint: [
    "视觉模型在同一次请求中直接返回译文，速度更快，但结果不如两步流程稳定。",
    "The vision model returns the translation directly. Faster, slightly less predictable.",
  ],

  /* capture */
  captureTitle: ["图像处理", "Image processing"],
  captureTitleHint: [
    "发送给识别引擎之前，图片会先按这里的规则裁剪与缩放。",
    "Crops are prepared with these rules before they reach the engine.",
  ],
  upscaleSmall: ["放大小图", "Upscale small crops"],
  upscaleSmallHint: [
    "小字号是识别失败的主要原因。放大小区域能显著提升准确率。",
    "Small text is the main cause of bad recognition. Upscaling helps a lot.",
  ],
  maxImageSide: ["最长边上限", "Long-edge limit"],
  maxImageSideHint: [
    "超过该尺寸的图片会被压缩后再上传，用于控制请求体积。",
    "Larger images are downscaled before upload to keep requests small.",
  ],
  jpegQuality: ["JPEG 质量", "JPEG quality"],
  jpegQualityHint: ["仅在视觉模型模式下生效。", "Only used with a vision model."],

  /* appearance */
  resultWindow: ["结果窗口", "Result window"],
  theme: ["主题", "Theme"],
  themeSystem: ["跟随系统", "System"],
  themeLight: ["浅色", "Light"],
  themeDark: ["深色", "Dark"],
  uiLanguage: ["界面语言", "Interface language"],
  resultFontSize: ["结果字号", "Result font size"],
  resultView: ["结果窗口显示", "Result window shows"],
  resultViewHint: [
    "对照：原文与译文同时显示；也可以只看原文或只看译文。",
    "Both shows the original and the translation; pick one side to keep it minimal.",
  ],
  resultViewBoth: ["对照显示", "Source + translation"],
  resultViewSource: ["只看原文", "Original only"],
  resultViewTranslation: ["只看译文", "Translation only"],
  viewSource: ["原文", "Source"],
  viewTranslation: ["译文", "Translation"],
  viewBoth: ["对照", "Both"],
  resultShot: ["截图", "Picture"],
  resultShotToggle: ["显示 / 隐藏截图", "Show or hide the captured picture"],
  resultShowImage: ["显示对照截图", "Show the captured picture"],
  resultShowImageHint: [
    "在结果窗口里显示截取的原图，便于与译文对照；也可以在结果窗口底部临时切换。",
    "Shows the crop in the result window for comparison; it can also be toggled from the window itself.",
  ],
  autoCopy: ["自动复制译文", "Copy the translation automatically"],
  autoCopyHint: ["翻译完成后直接写入剪贴板。", "Puts the result on the clipboard when it is ready."],
  closeOnBlur: ["失焦时自动隐藏结果", "Hide the result when it loses focus"],
  closeOnBlurHint: [
    "点击其他窗口后结果窗口自动隐藏，适合连续截图。",
    "Handy when you capture repeatedly.",
  ],

  /* history */
  historyTitle: ["识别历史", "History"],
  historyEnabled: ["保存历史记录", "Keep history"],
  historyEnabledHint: [
    "仅保存在本机，不包含截图图片。",
    "Stored locally. Screenshots are never saved.",
  ],
  historyLimit: ["最多保存条数", "Maximum entries"],
  historyEmpty: ["还没有记录", "Nothing here yet"],
  historyEmptyHint: ["完成一次截图翻译后，记录会出现在这里。", "Your captures will show up here."],
  historyClear: ["清空历史", "Clear history"],
  historyClearConfirm: ["确定要清空全部历史记录吗？", "Clear the whole history?"],

  /* logs */
  logsLead: [
    "这里记录每次执行走了哪个识别渠道、调用了哪个接口、地址与关键参数，以及失败原因。",
    "Every run logs which engine ran, which endpoint was called, the key parameters and why it failed.",
  ],
  logsRefresh: ["刷新", "Refresh"],
  logsAuto: ["自动刷新", "Auto refresh"],
  logsFilter: ["过滤（渠道 / 地址 / 关键字）", "Filter (engine / host / keyword)"],
  logsLevel: ["日志类型", "Log level"],
  logsLevelError: ["错误", "Error"],
  logsLevelWarn: ["警告", "Warning"],
  logsLevelInfo: ["信息", "Info"],
  logsLevelDebug: ["调试", "Debug"],
  logsClear: ["清空日志", "Clear log"],
  logsClearConfirm: ["确定要清空日志文件吗？", "Clear the log file?"],
  logsReveal: ["打开文件位置", "Show in folder"],
  logsCopy: ["复制全部", "Copy all"],
  logsEmpty: ["还没有日志", "No log entries yet"],
  logsEmptyHint: ["完成一次截图翻译后回来看这里。", "Run a capture and come back."],
  logsTail: ["只显示最后 256 KB，完整内容在日志文件里。", "Showing the last 256 KB; the file holds everything."],
  logsFile: ["日志文件", "Log file"],
  logsNoMatch: ["没有匹配的行", "No matching lines"],
  endpointActual: ["实际调用", "Will call"],
  endpointLocal: ["本机运行，不发网络请求", "Runs locally, no network request"],

  /* result window */
  resultOcr: ["识别中…", "Recognising…"],
  resultTranslate: ["翻译中…", "Translating…"],
  resultDone: ["完成", "Done"],
  resultError: ["出错了", "Something went wrong"],
  copy: ["复制", "Copy"],
  copied: ["已复制", "Copied"],
  copySource: ["复制原文", "Copy original"],
  retryTranslate: ["重新翻译", "Translate again"],
  close: ["关闭", "Close"],
  original: ["原文", "Original"],
  translation: ["译文", "Translation"],
  elapsed: ["耗时 {ms} ms", "{ms} ms"],
  noTranslation: ["未配置翻译服务", "No translation service configured"],

  /* overlay */
  overlayHint: ["拖动鼠标框选要翻译的文字 · Esc 取消 · 右键取消", "Drag to select text · Esc or right-click to cancel"],
  overlaySelecting: ["松开鼠标即可识别", "Release to recognise"],
  overlayTooSmall: ["选区太小", "Selection too small"],
  overlayBusy: ["正在截取屏幕…", "Capturing the screen…"],
  overlayEsc: ["取消", "Cancel"],

  /* feedback */
  saved: ["已保存", "Saved"],
  saveFailed: ["保存失败：{message}", "Could not save: {message}"],
  resetDone: ["已恢复默认设置", "Defaults restored"],
  resetConfirm: ["将恢复全部默认设置，确定继续吗？", "Restore all default settings?"],
  resetDefaults: ["恢复默认设置", "Restore defaults"],
  loading: ["加载中…", "Loading…"],
  notConfigured: ["尚未配置", "Not configured"],
  ready: ["就绪", "Ready"],
  empty: ["—", "—"],
  copyFailed: ["复制失败：{message}", "Copy failed: {message}"],

  /* about */
  aboutTagline: [
    "轻量、常驻后台的截图翻译工具。按下快捷键，框选文字，直接得到译文。",
    "A lightweight, always-on screenshot translator. Press the hotkey, select text, read the translation.",
  ],
  aboutVersion: ["版本", "Version"],
  guideOpen: ["使用手册", "User guide"],
  guideHint: ["中文图文手册：从安装、配置到每个选项的说明", "A Chinese walkthrough: setup, configuration, every option"],
  aboutPaths: ["文件位置", "Files"],
  openConfigDir: ["打开配置目录", "Open config folder"],
  aboutShortcuts: ["操作方式", "How to use"],
  aboutShortcutList: [
    "全局快捷键（默认 Ctrl+Alt+Z）：框选截图并翻译\nEsc / 右键：取消本次截图\n托盘右键：截图、设置、历史、退出\n结果窗口：复制、重新翻译、关闭\n多显示器：每块屏幕都会出现框选层",
    "Global hotkey (Ctrl+Alt+Z by default): capture and translate\nEsc / right-click: cancel the capture\nTray menu: capture, settings, history, quit\nResult window: copy, translate again, close\nMultiple monitors: every screen gets a selection layer",
  ],
  aboutNote: [
    "截图内容只会在本机内存中处理；选择「视觉大模型」或在线翻译时，被裁剪的图片或文字会发送到你填写的接口。",
    "Captures stay in memory on this machine. With a vision model or an online translator, the crop or its text is sent to the endpoint you configured.",
  ],
  monitorsTitle: ["显示器", "Monitors"],
  logFile: ["运行日志", "Log file"],
} satisfies Record<string, readonly [string, string]>;

export type StringKey = keyof typeof STRINGS;

let current: Lang = "zh";

export function setLang(lang: Lang): void {
  current = lang === "en" ? "en" : "zh";
}

export function getLang(): Lang {
  return current;
}

export function t(
  lang: Lang,
  key: StringKey,
  vars?: Record<string, string | number>,
): string {
  let text = STRINGS[key][lang === "en" ? 1 : 0] || STRINGS[key][0];
  if (!text) text = STRINGS[key][0];
  if (vars) {
    for (const [name, value] of Object.entries(vars)) {
      text = text.split(`{${name}}`).join(String(value));
    }
  }
  return text;
}

/** For code outside React (toasts, event handlers). */
export function tr(key: StringKey, vars?: Record<string, string | number>): string {
  return t(current, key, vars);
}
