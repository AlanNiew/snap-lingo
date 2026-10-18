/** The seven settings pages. Each one is a column of cards. */
import { useState } from "react";

import { api, type MonitorInfo, type OcrLanguage } from "../lib/api";
import { PROVIDER_KEYS } from "../lib/labels";
import type { AppConfig, ConnectionCheck, HistoryItem, ProviderPreset, TranslateProvider } from "../lib/types";
import { copyText, formatClock, formatDuration, truncate } from "../lib/util";
import { useT } from "./context";
import {
  Card,
  Field,
  HotkeyRecorder,
  OptionCard,
  Row,
  SecretInput,
  Select,
  Slider,
  Switch,
  TextArea,
  TextInput,
} from "./fields";
import { PRESETS } from "./presets";

export interface PatchFn {
  (recipe: (config: AppConfig) => AppConfig, immediate?: boolean): void;
}

export interface NotifyFn {
  (message: string, bad?: boolean): void;
}

interface Base {
  config: AppConfig;
  patch: PatchFn;
  notify: NotifyFn;
}

const TARGET_LANGS = [
  { value: "zh-CN", label: "简体中文" },
  { value: "zh-TW", label: "繁體中文" },
  { value: "en", label: "English" },
  { value: "ja", label: "日本語" },
  { value: "ko", label: "한국어" },
  { value: "fr", label: "Français" },
  { value: "de", label: "Deutsch" },
  { value: "es", label: "Español" },
  { value: "ru", label: "Русский" },
  { value: "pt", label: "Português" },
  { value: "it", label: "Italiano" },
  { value: "ar", label: "العربية" },
  { value: "hi", label: "हिन्दी" },
  { value: "th", label: "ไทย" },
  { value: "vi", label: "Tiếng Việt" },
  { value: "id", label: "Bahasa Indonesia" },
  { value: "tr", label: "Türkçe" },
];

function withCurrent(
  options: Array<{ value: string; label: string }>,
  current: string,
): Array<{ value: string; label: string }> {
  if (!current || options.some((option) => option.value === current)) return options;
  return [{ value: current, label: current }, ...options];
}

/* ================================================================= general */

export function GeneralSection({
  config,
  patch,
  hotkeyError,
  monitors,
}: Base & { hotkeyError: string | null; monitors: MonitorInfo[] }) {
  const { t } = useT();

  return (
    <>
      <h1 className="page-title">{t("tabGeneral")}</h1>
      <p className="page-lead">{t("trayHint")}</p>

      <Card
        title={t("shortcutTitle")}
        note={t("shortcutHint")}
        actions={
          <button className="btn btn-primary btn-sm" onClick={() => api.startCapture()}>
            {t("testCapture")}
          </button>
        }
      >
        <div className="row">
          <div className="row-main">
            <HotkeyRecorder
              value={config.shortcut}
              recordingLabel={t("shortcutRecording")}
              onCommit={(accelerator) =>
                patch((cfg) => ({ ...cfg, shortcut: accelerator }), true)
              }
            />
          </div>
        </div>
        {hotkeyError ? (
          <div className="note note-bad" style={{ marginTop: 10 }}>
            {hotkeyError}
          </div>
        ) : null}
      </Card>

      <Card title={t("startupTitle")} note={t("startupHint")}>
        <Row title={t("startupTitle")}>
          <Switch
            checked={config.startup}
            onChange={(next) => patch((cfg) => ({ ...cfg, startup: next }), true)}
          />
        </Row>
      </Card>

      <Card title={t("trayTitle")} note={t("trayHint")}>
        <Row title={t("monitorsTitle")} hint={t("captureAllMonitorsHint")}>
          <span className="badge">
            {monitors.length} {t("monitorsTitle")}
          </span>
        </Row>
      </Card>
    </>
  );
}

/* ===================================================================== ocr */

function PresetRow({
  presets,
  onPick,
}: {
  presets: ProviderPreset[];
  onPick: (preset: ProviderPreset) => void;
}) {
  const { t } = useT();
  const [open, setOpen] = useState(false);

  return (
    <div>
      <Row
        title={t("presetTitle")}
        hint={t("presetHint")}
      >
        <button className="btn btn-sm" onClick={() => setOpen((value) => !value)}>
          {open ? t("hideKey") : t("showKey")}
        </button>
      </Row>
      {open ? (
        <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginTop: 8 }}>
          {presets.map((preset) => (
            <button
              key={preset.id}
              className="btn btn-sm"
              title={preset.hint}
              onClick={() => onPick(preset)}
            >
              {preset.label}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function EndpointFields({
  kind,
  baseUrl,
  apiKey,
  model,
  set,
  onTest,
}: {
  kind: "ocr" | "translate";
  baseUrl: string;
  apiKey: string;
  model: string;
  set: (patch: Partial<{ baseUrl: string; apiKey: string; model: string }>) => void;
  /** Overrides the built-in OpenAI-compatible check (used by other engines). */
  onTest?: () => Promise<ConnectionCheck>;
}) {
  const { t } = useT();
  const [models, setModels] = useState<string[]>([]);
  const [busy, setBusy] = useState<"models" | "test" | null>(null);
  const [check, setCheck] = useState<ConnectionCheck | null>(null);

  const loadModels = async () => {
    if (busy) return;
    setBusy("models");
    try {
      const list = await api.listModels(baseUrl, apiKey);
      setModels(list);
      if (list.length === 0) setCheck({ ok: false, message: "接口没有返回任何模型。", elapsedMs: 0, sample: null });
    } catch (error) {
      setCheck({ ok: false, message: String(error), elapsedMs: 0, sample: null });
    } finally {
      setBusy(null);
    }
  };

  const test = async () => {
    if (busy) return;
    setBusy("test");
    try {
      setCheck(onTest ? await onTest() : await api.testOcrConnection(baseUrl, apiKey, model));
    } catch (error) {
      setCheck({ ok: false, message: String(error), elapsedMs: 0, sample: null });
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="stack">
      <Field label={t("endpointBaseUrl")} hint={t("endpointBaseUrlHint")}>
        <TextInput mono value={baseUrl} onChange={(value) => set({ baseUrl: value })} placeholder="https://api.example.com/v1" />
      </Field>

      <Field label={t("endpointKey")} hint={t("endpointKeyHint")}>
        <SecretInput
          value={apiKey}
          onChange={(value) => set({ apiKey: value })}
          placeholder="sk-…"
          showLabel={t("showKey")}
          hideLabel={t("hideKey")}
        />
      </Field>

      <Field label={t("endpointModel")} hint={t("endpointModelHint")}>
        <div className="input-group">
          <TextInput
            mono
            value={model}
            onChange={(value) => set({ model: value })}
            list={`models-${kind}`}
            placeholder={kind === "ocr" ? "qwen-vl-max" : "gpt-4o-mini"}
          />
          <datalist id={`models-${kind}`}>
            {models.map((name) => (
              <option key={name} value={name} />
            ))}
          </datalist>
          <button className="btn" disabled={busy !== null} onClick={loadModels}>
            {busy === "models" ? <span className="spinner" /> : null}
            {busy === "models" ? t("fetchingModels") : t("fetchModels")}
          </button>
        </div>
      </Field>

      <Row title={t("testConnection")}>
        <button className="btn" disabled={busy !== null || !model} onClick={test}>
          {busy === "test" ? <span className="spinner" /> : null}
          {busy === "test" ? t("testing") : t("testConnection")}
        </button>
      </Row>

      {check ? (
        <div className={`note ${check.ok ? "note-ok" : "note-bad"}`}>
          <span>
            {check.message}
            {check.ok ? ` · ${formatDuration(check.elapsedMs)}` : ""}
            {check.ok && check.sample ? ` · ${truncate(check.sample, 60)}` : ""}
          </span>
        </div>
      ) : null}
    </div>
  );
}

export function OcrSection({ config, patch, notify, languages }: Base & { languages: OcrLanguage[] }) {
  const { t } = useT();
  const [probe, setProbe] = useState<ConnectionCheck | null>(null);
  const [probing, setProbing] = useState(false);
  const isVision = config.ocr.provider === "vision";

  const setOcr = (patchOcr: Partial<AppConfig["ocr"]>, immediate = false) =>
    patch((cfg) => ({ ...cfg, ocr: { ...cfg.ocr, ...patchOcr } }), immediate);

  const runProbe = async () => {
    setProbing(true);
    try {
      setProbe(await api.ocrProbe(config.ocr.language));
    } catch (error) {
      setProbe({ ok: false, message: String(error), elapsedMs: 0, sample: null });
    } finally {
      setProbing(false);
    }
  };

  return (
    <>
      <h1 className="page-title">{t("tabOcr")}</h1>
      <p className="page-lead">{t("ocrEngineTitle")}</p>

      <Card>
        <div role="radiogroup">
          <OptionCard
            checked={!isVision}
            onSelect={() => setOcr({ provider: "winocr" }, true)}
            title={
              <>
                {t("ocrWin")} <span className="badge badge-accent">{t("ready")}</span>
              </>
            }
            desc={t("ocrWinDesc")}
          />
          <OptionCard
            checked={isVision}
            onSelect={() => setOcr({ provider: "vision" }, true)}
            title={t("ocrVision")}
            desc={t("ocrVisionDesc")}
          />
        </div>
      </Card>

      {isVision ? (
        <>
          <Card title={t("endpointBaseUrl")}>
            <EndpointFields
              kind="ocr"
              baseUrl={config.ocr.baseUrl}
              apiKey={config.ocr.apiKey}
              model={config.ocr.model}
              set={(next) => setOcr(next, false)}
            />
            <div style={{ marginTop: 12, borderTop: "1px solid var(--border)", paddingTop: 12 }}>
              <PresetRow
                presets={PRESETS.filter((preset) => preset.visionModel)}
                onPick={(preset) => {
                  setOcr({ baseUrl: preset.baseUrl, model: preset.visionModel }, true);
                  notify(`${preset.label} · ${preset.visionModel}`, false);
                }}
              />
            </div>
          </Card>

          <Card title={t("advanced")}>
            <Row title={t("oneShot")} hint={t("oneShotHint")}>
              <Switch checked={config.ocr.oneShot} onChange={(next) => setOcr({ oneShot: next }, true)} />
            </Row>
            <div style={{ marginTop: 10 }}>
              <Field label={t("customPrompt")} hint={t("customPromptHint")}>
                <TextArea
                  rows={3}
                  value={config.ocr.prompt}
                  onChange={(value) => setOcr({ prompt: value })}
                  placeholder="只输出文字，不要任何解释；保留换行。"
                />
              </Field>
            </div>
          </Card>
        </>
      ) : (
        <>
          <Card title={t("ocrLanguage")} note={t("ocrLanguageHint")}>
            <Row title={t("ocrLanguage")}>
              <Select
                value={config.ocr.language}
                onChange={(value) => setOcr({ language: value }, true)}
                options={[
                  { value: "", label: t("ocrLanguageAuto") },
                  ...languages.map((language) => ({
                    value: language.tag,
                    label: `${language.name}（${language.tag}）`,
                  })),
                ]}
              />
            </Row>
            <div className="row" style={{ marginTop: 10 }}>
              <div className="row-main">
                <button className="btn" disabled={probing} onClick={runProbe}>
                  {probing ? <span className="spinner" /> : null}
                  {probing ? t("probeOcrRunning") : t("probeOcr")}
                </button>
              </div>
            </div>
            {probe ? (
              <div className={`note ${probe.ok ? "note-ok" : "note-bad"}`} style={{ marginTop: 10 }}>
                <span>
                  {probe.message}
                  {probe.ok ? ` · ${formatDuration(probe.elapsedMs)}` : ""}
                </span>
              </div>
            ) : null}
          </Card>

          <Card title={t("ocrInstalled")}>
            {languages.length === 0 ? (
              <div className="note note-warn">{t("probeOcr")}</div>
            ) : (
              <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
                {languages.map((language) => (
                  <span key={language.tag} className="badge">
                    {language.name}
                  </span>
                ))}
              </div>
            )}
          </Card>
        </>
      )}
    </>
  );
}

/* =============================================================== translate */

/** Connection check for the engines that are not plain OpenAI-compatible. */
function TranslateTest({ config }: { config: AppConfig }) {
  const { t } = useT();
  const [busy, setBusy] = useState(false);
  const [check, setCheck] = useState<ConnectionCheck | null>(null);

  const run = async () => {
    if (busy) return;
    setBusy(true);
    try {
      setCheck(await api.testTranslateConnection(config.translate));
    } catch (error) {
      setCheck({ ok: false, message: String(error), elapsedMs: 0, sample: null });
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <Row title={t("testConnection")}>
        <button className="btn" disabled={busy} onClick={run}>
          {busy ? <span className="spinner" /> : null}
          {busy ? t("testing") : t("testConnection")}
        </button>
      </Row>
      {check ? (
        <div className={`note ${check.ok ? "note-ok" : "note-bad"}`}>
          <span>
            {check.message}
            {check.ok ? ` · ${formatDuration(check.elapsedMs)}` : ""}
            {check.ok && check.sample ? ` · ${truncate(check.sample, 60)}` : ""}
          </span>
        </div>
      ) : null}
    </>
  );
}

export function TranslateSection({ config, patch, notify }: Base) {
  const { t } = useT();
  const provider = config.translate.provider;
  const isOpenai = provider === "openai";
  const isDeepl = provider === "deepl";
  const isMicrosoft = provider === "microsoft";
  const needsSecret = provider === "youdao" || provider === "baidu" || provider === "tencent";
  const needsRegion = isMicrosoft || provider === "tencent";
  const secretLabel =
    provider === "youdao" ? t("secretYoudao") : provider === "baidu" ? t("secretBaidu") : t("secretTencent");

  const setTranslate = (next: Partial<AppConfig["translate"]>, immediate = false) =>
    patch((cfg) => ({ ...cfg, translate: { ...cfg.translate, ...next } }), immediate);

  const engines: Array<{ id: TranslateProvider; title: string; desc: string }> = [
    { id: "openai", title: t("translateOpenai"), desc: t("translateOpenaiDesc") },
    { id: "deepl", title: t("translateDeepl"), desc: t("translateDeeplDesc") },
    { id: "microsoft", title: t("translateMicrosoft"), desc: t("translateMicrosoftDesc") },
    { id: "google", title: t("translateGoogle"), desc: t("translateGoogleDesc") },
    { id: "youdao", title: t("translateYoudao"), desc: t("translateYoudaoDesc") },
    { id: "baidu", title: t("translateBaidu"), desc: t("translateBaiduDesc") },
    { id: "tencent", title: t("translateTencent"), desc: t("translateTencentDesc") },
    { id: "none", title: t("translateNone"), desc: t("translateNoneDesc") },
  ];

  return (
    <>
      <h1 className="page-title">{t("tabTranslate")}</h1>
      <p className="page-lead">{t("translateProvider")}</p>

      <Card>
        <div role="radiogroup">
          {engines.map((engine) => (
            <OptionCard
              key={engine.id}
              checked={provider === engine.id}
              onSelect={() => setTranslate({ provider: engine.id }, true)}
              title={engine.title}
              desc={engine.desc}
            />
          ))}
        </div>
      </Card>

      {provider !== "none" ? (
        <Card title={t("targetLang")} note={t("targetLangHint")}>
          <Row title={t("targetLang")}>
            <Select
              value={config.translate.targetLang}
              onChange={(value) => setTranslate({ targetLang: value }, true)}
              options={withCurrent(TARGET_LANGS, config.translate.targetLang)}
            />
          </Row>
        </Card>
      ) : null}

      {isOpenai ? (
        <>
          <Card title={t("endpointBaseUrl")}>
            <EndpointFields
              kind="translate"
              baseUrl={config.translate.baseUrl}
              apiKey={config.translate.apiKey}
              model={config.translate.model}
              set={(next) => setTranslate(next, false)}
              onTest={() => api.testTranslateConnection(config.translate)}
            />
            <div style={{ marginTop: 12, borderTop: "1px solid var(--border)", paddingTop: 12 }}>
              <PresetRow
                presets={PRESETS.filter((preset) => preset.textModel)}
                onPick={(preset) => {
                  setTranslate({ baseUrl: preset.baseUrl, model: preset.textModel }, true);
                  notify(`${preset.label} · ${preset.textModel}`, false);
                }}
              />
            </div>
          </Card>

          <Card title={t("advanced")}>
            <Field label={t("customPrompt")} hint={t("customPromptHint")}>
              <TextArea
                rows={3}
                value={config.translate.prompt}
                onChange={(value) => setTranslate({ prompt: value })}
                placeholder="把下面内容翻译成{目标语言}，只输出译文，保留换行与列表结构。"
              />
            </Field>
          </Card>
        </>
      ) : null}

      {isDeepl ? (
        <Card title="DeepL" note={t("deeplKeyHint")}>
          <div className="stack">
            <Field label={t("endpointKey")} hint={t("deeplKeyHint")}>
              <SecretInput
                value={config.translate.apiKey}
                onChange={(value) => setTranslate({ apiKey: value })}
                placeholder="xxxxxxxx-xxxx-…:fx"
                showLabel={t("showKey")}
                hideLabel={t("hideKey")}
              />
            </Field>
            <Field label={t("deeplPlan")}>
              <Select
                value={config.translate.deeplPlan}
                onChange={(value) => setTranslate({ deeplPlan: value as AppConfig["translate"]["deeplPlan"] }, true)}
                options={[
                  { value: "free", label: t("deeplFree") },
                  { value: "pro", label: t("deeplPro") },
                ]}
              />
            </Field>
            <TranslateTest config={config} />
          </div>
        </Card>
      ) : null}

      {!isOpenai && !isDeepl && provider !== "none" ? (
        <Card title={t("endpointKey")} note={t("secretHint")}>
          <div className="stack">
            <Field label={t("endpointKey")} hint={t("endpointKeyHint")}>
              <SecretInput
                value={config.translate.apiKey}
                onChange={(value) => setTranslate({ apiKey: value })}
                placeholder="…"
                showLabel={t("showKey")}
                hideLabel={t("hideKey")}
              />
            </Field>
            {needsSecret ? (
              <Field label={secretLabel}>
                <SecretInput
                  value={config.translate.apiSecret}
                  onChange={(value) => setTranslate({ apiSecret: value })}
                  placeholder="…"
                  showLabel={t("showKey")}
                  hideLabel={t("hideKey")}
                />
              </Field>
            ) : null}
            {needsRegion ? (
              <Field label={t("region")} hint={t("regionHint")}>
                <TextInput
                  mono
                  value={config.translate.region}
                  onChange={(value) => setTranslate({ region: value })}
                  placeholder={isMicrosoft ? "eastasia" : "ap-guangzhou"}
                />
              </Field>
            ) : null}
            <TranslateTest config={config} />
          </div>
        </Card>
      ) : null}
    </>
  );
}

/* ================================================================= capture */

export function CaptureSection({ config, patch }: Base) {
  const { t } = useT();
  const setCapture = (next: Partial<AppConfig["capture"]>, immediate = false) =>
    patch((cfg) => ({ ...cfg, capture: { ...cfg.capture, ...next } }), immediate);

  return (
    <>
      <h1 className="page-title">{t("tabCapture")}</h1>
      <p className="page-lead">{t("captureTitleHint")}</p>

      <Card title={t("captureModeTitle")}>
        <div role="radiogroup">
          <OptionCard
            checked={config.capture.allMonitors}
            onSelect={() => setCapture({ allMonitors: true }, true)}
            title={t("captureAllMonitors")}
            desc={t("captureAllMonitorsHint")}
          />
          <OptionCard
            checked={!config.capture.allMonitors}
            onSelect={() => setCapture({ allMonitors: false }, true)}
            title={t("capturePrimaryOnly")}
          />
        </div>
      </Card>

      <Card title={t("captureTitle")}>
        <Row title={t("upscaleSmall")} hint={t("upscaleSmallHint")}>
          <Switch checked={config.capture.upscaleSmall} onChange={(next) => setCapture({ upscaleSmall: next }, true)} />
        </Row>
        <Row title={t("maxImageSide")} hint={t("maxImageSideHint")}>
          <Slider
            min={640}
            max={4096}
            step={32}
            value={config.capture.maxImageSide}
            onChange={(value) => setCapture({ maxImageSide: value })}
            format={(value) => `${value} px`}
          />
        </Row>
        <Row
          title={t("jpegQuality")}
          hint={config.ocr.provider === "vision" ? t("jpegQualityHint") : t("ocrWinDesc")}
        >
          <Slider
            min={40}
            max={100}
            value={config.capture.jpegQuality}
            onChange={(value) => setCapture({ jpegQuality: value })}
            format={(value) => `${value}`}
            disabled={config.ocr.provider !== "vision"}
          />
        </Row>
      </Card>
    </>
  );
}

/* ============================================================== appearance */

export function AppearanceSection({ config, patch }: Base) {
  const { t } = useT();
  const setUi = (next: Partial<AppConfig["ui"]>, immediate = false) =>
    patch((cfg) => ({ ...cfg, ui: { ...cfg.ui, ...next } }), immediate);

  return (
    <>
      <h1 className="page-title">{t("tabAppearance")}</h1>

      <Card title={t("theme")}>
        <Row title={t("theme")}>
          <div className="segmented">
            {[
              { value: "system", label: t("themeSystem") },
              { value: "light", label: t("themeLight") },
              { value: "dark", label: t("themeDark") },
            ].map((option) => (
              <button
                key={option.value}
                aria-pressed={config.ui.theme === option.value}
                onClick={() => setUi({ theme: option.value as AppConfig["ui"]["theme"] }, true)}
              >
                {option.label}
              </button>
            ))}
          </div>
        </Row>
        <Row title={t("uiLanguage")}>
          <div className="segmented">
            {[
              { value: "zh", label: "简体中文" },
              { value: "en", label: "English" },
            ].map((option) => (
              <button
                key={option.value}
                aria-pressed={config.ui.language === option.value}
                onClick={() => setUi({ language: option.value as AppConfig["ui"]["language"] }, true)}
              >
                {option.label}
              </button>
            ))}
          </div>
        </Row>
      </Card>

      <Card title={t("resultWindow")} note={t("resultFontSize")}>
        <Row title={t("resultFontSize")}>
          <Slider
            min={11}
            max={32}
            value={config.ui.resultFontSize}
            onChange={(value) => setUi({ resultFontSize: value })}
            format={(value) => `${value} px`}
          />
        </Row>
        <Row title={t("resultView")} hint={t("resultViewHint")}>
          <Select
            value={config.ui.resultView}
            onChange={(value) => setUi({ resultView: value as AppConfig["ui"]["resultView"] }, true)}
            options={[
              { value: "both", label: t("resultViewBoth") },
              { value: "source", label: t("resultViewSource") },
              { value: "translation", label: t("resultViewTranslation") },
            ]}
          />
        </Row>
        <Row title={t("autoCopy")} hint={t("autoCopyHint")}>
          <Switch checked={config.ui.autoCopy} onChange={(next) => setUi({ autoCopy: next }, true)} />
        </Row>
        <Row title={t("closeOnBlur")} hint={t("closeOnBlurHint")}>
          <Switch checked={config.ui.closeOnBlur} onChange={(next) => setUi({ closeOnBlur: next }, true)} />
        </Row>
      </Card>
    </>
  );
}

/* ================================================================= history */

export function HistorySection({
  config,
  patch,
  notify,
  history,
  reloadHistory,
}: Base & { history: HistoryItem[]; reloadHistory: () => void }) {
  const { t } = useT();
  const setHistory = (next: Partial<AppConfig["history"]>, immediate = false) =>
    patch((cfg) => ({ ...cfg, history: { ...cfg.history, ...next } }), immediate);

  const copy = async (item: HistoryItem) => {
    const ok = await copyText(item.translatedText || item.sourceText);
    notify(ok ? t("copied") : t("copyFailed", { message: "" }), !ok);
  };

  const remove = async (id: string) => {
    try {
      await api.deleteHistoryItem(id);
      reloadHistory();
    } catch (error) {
      notify(String(error), true);
    }
  };

  return (
    <>
      <h1 className="page-title">{t("tabHistory")}</h1>

      <Card
        title={t("historyTitle")}
        note={t("historyEnabledHint")}
        actions={
          <button
            className="btn btn-sm btn-danger"
            disabled={history.length === 0}
            onClick={async () => {
              if (!window.confirm(t("historyClearConfirm"))) return;
              await api.clearHistory();
              reloadHistory();
              notify(t("saved"));
            }}
          >
            {t("historyClear")}
          </button>
        }
      >
        <Row title={t("historyEnabled")}>
          <Switch checked={config.history.enabled} onChange={(next) => setHistory({ enabled: next }, true)} />
        </Row>
        <Row title={t("historyLimit")}>
          <Slider
            min={0}
            max={500}
            step={10}
            value={config.history.limit}
            onChange={(value) => setHistory({ limit: value })}
            disabled={!config.history.enabled}
          />
        </Row>
      </Card>

      <Card>
        {history.length === 0 ? (
          <div className="empty">
            <div style={{ fontWeight: 500, color: "var(--text-soft)" }}>{t("historyEmpty")}</div>
            <div style={{ marginTop: 4 }}>{t("historyEmptyHint")}</div>
          </div>
        ) : (
          <div>
            {history.map((item) => (
              <div className="history-item" key={item.id}>
                <div className="history-text">
                  <div className="history-src">{item.sourceText}</div>
                  {item.translatedText ? <div className="history-dst">{item.translatedText}</div> : null}
                </div>
                <div className="history-meta">
                  <span className="badge">{formatClock(item.createdAt)}</span>
                  <div className="history-actions">
                    <button className="btn btn-sm btn-ghost" onClick={() => copy(item)}>
                      {t("copy")}
                    </button>
                    <button className="btn btn-sm btn-ghost" onClick={() => remove(item.id)}>
                      ✕
                    </button>
                  </div>
                </div>
              </div>
            ))}
          </div>
        )}
      </Card>
    </>
  );
}

/* =================================================================== about */

export function AboutSection({
  config,
  monitors,
}: {
  config: AppConfig;
  monitors: MonitorInfo[];
}) {
  const { t } = useT();

  return (
    <>
      <h1 className="page-title">{t("tabAbout")}</h1>
      <p className="page-lead">{t("aboutTagline")}</p>

      <Card>
        <div className="row">
          <div className="row-main">
            <div className="row-title" style={{ fontSize: 15 }}>
              {t("appName")}
            </div>
            <div className="row-hint">
              {t("aboutVersion")} {__APP_VERSION__} · {config.ocr.provider === "winocr" ? t("ocrWin") : t("ocrVision")} +{" "}
              {t(PROVIDER_KEYS[config.translate.provider])}
            </div>
          </div>
          <div className="row-control">
            <span className="badge badge-accent">
              {config.shortcut.replace(/Key/, "")}
            </span>
          </div>
        </div>
      </Card>

      <Card title={t("aboutShortcuts")}>
        <div style={{ whiteSpace: "pre-wrap" }}>{t("aboutShortcutList")}</div>
      </Card>

      <Card title={t("monitorsTitle")}>
        <div className="mono-list">
          {monitors.map((monitor) => (
            <div key={monitor.id}>
              #{monitor.id} · {monitor.width}×{monitor.height} @ {monitor.scaleFactor.toFixed(2)}x · {monitor.name}
            </div>
          ))}
        </div>
      </Card>

      <Card title={t("aboutPaths")}>
        <Row title={t("openConfigDir")} hint={t("logFile") + " · snaplingo.log"}>
          <button className="btn btn-sm" onClick={() => api.revealConfigDir()}>
            {t("openConfigDir")}
          </button>
        </Row>
      </Card>

      <Card title={t("aboutTagline")}>
        <div className="note">{t("aboutNote")}</div>
      </Card>
    </>
  );
}
