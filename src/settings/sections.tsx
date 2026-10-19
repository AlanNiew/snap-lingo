/** The eight settings pages. Each one is a column of cards. */
import { useCallback, useEffect, useMemo, useState } from "react";

import { api, type MonitorInfo, type OcrLanguage } from "../lib/api";
import {
  OCR_ENGINES,
  PROVIDER_KEYS,
  TARGET_LANGS,
  TRANSLATE_ENGINES,
  engineMeta,
  targetOptionLabel,
  withTarget,
  type ProfileField,
} from "../lib/labels";
import {
  EMPTY_PROFILE,
  type AppConfig,
  type ConnectionCheck,
  type EndpointInfo,
  type HistoryItem,
  type LogFile,
  type OcrProvider,
  type ProviderPreset,
  type TranslateProfile,
  type TranslateProvider,
} from "../lib/types";
import { copyText, cx, formatClock, formatDuration, truncate } from "../lib/util";
import { THEMES, THEME_LABEL_KEYS, THEME_SWATCHES } from "../lib/theme";
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

/** Opens a service's console / sign-up page in the default browser. */
function ApplyLink({ url, label }: { url: string; label?: string }) {
  const { t } = useT();
  if (!url) return null;
  return (
    <button className="btn btn-sm btn-ghost" title={url} onClick={() => void api.openExternal(url)}>
      {label ?? t("applyUrlOpen")} ↗
    </button>
  );
}

/**
 * Shows the request a channel will really send, so a typo in the endpoint or a
 * token pasted into the wrong field is visible before a capture is attempted.
 */
function EndpointHint({ config, kind }: { config: AppConfig; kind: "ocr" | "translate" | "picture" }) {
  const { t } = useT();
  const [info, setInfo] = useState<EndpointInfo | null>(null);

  useEffect(() => {
    let alive = true;
    api
      .describeEndpoints(config)
      .then((all) => {
        if (!alive) return;
        setInfo(kind === "ocr" ? all.ocr : kind === "translate" ? all.translate : all.picture);
      })
      .catch(() => {
        if (alive) setInfo(null);
      });
    return () => {
      alive = false;
    };
  }, [config, kind]);

  if (!info || !info.url) return null;
  return (
    <div className="endpoint-hint">
      <span className="endpoint-hint-tag">{t("endpointActual")}</span>
      <code>{`${info.method} ${info.url}`}</code>
      {info.note ? <span className="endpoint-hint-note">{info.note}</span> : null}
    </div>
  );
}

/**
 * Dropdown of known services. Picking one fills the endpoint and model; the
 * matching preset is also how we know which sign-up page to link to.
 */
function PresetPicker({
  presets,
  onPick,
  currentUrl,
}: {
  presets: ProviderPreset[];
  onPick: (preset: ProviderPreset) => void;
  /** Endpoint currently in the form, used to reflect the matching preset. */
  currentUrl: string;
}) {
  const { t } = useT();
  const matched = presets.find((preset) => preset.baseUrl === currentUrl);

  return (
    <div className="stack">
      <Row title={t("presetTitle")} hint={t("presetHint")}>
        <Select
          value={matched?.id ?? ""}
          onChange={(value) => {
            const preset = presets.find((item) => item.id === value);
            if (preset) onPick(preset);
          }}
          options={[
            { value: "", label: t("presetCustom") },
            ...presets.map((preset) => ({ value: preset.id, label: preset.label })),
          ]}
        />
      </Row>
      {matched ? (
        <Row title={t("applyUrl")} hint={matched.applyUrl}>
          <ApplyLink url={matched.applyUrl} />
        </Row>
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

/** The selected translator, when it is able to read the picture itself. */
function pictureChannel(config: AppConfig): "youdao" | "baidu" | "tencent" | null {
  const provider = config.translate.provider;
  return provider === "youdao" || provider === "baidu" || provider === "tencent" ? provider : null;
}

export function OcrSection({ config, patch, notify, languages }: Base & { languages: OcrLanguage[] }) {
  const { t } = useT();
  const [probe, setProbe] = useState<ConnectionCheck | null>(null);
  const [probing, setProbing] = useState(false);
  const provider = config.ocr.provider;
  const meta = OCR_ENGINES.find((engine) => engine.id === provider) ?? OCR_ENGINES[0];
  const picture = pictureChannel(config);
  const pictureMeta = picture ? engineMeta(picture) : undefined;

  const setOcr = (patchOcr: Partial<AppConfig["ocr"]>, immediate = false) =>
    patch((cfg) => ({ ...cfg, ocr: { ...cfg.ocr, ...patchOcr } }), immediate);

  const runProbe = async () => {
    setProbing(true);
    try {
      setProbe(
        provider === "local"
          ? await api.testLocalOcrConnection(config.ocr)
          : await api.ocrProbe(config.ocr.language),
      );
    } catch (error) {
      setProbe({ ok: false, message: String(error), elapsedMs: 0, sample: null });
    } finally {
      setProbing(false);
    }
  };

  const probeNote = probe ? (
    <div className={`note ${probe.ok ? "note-ok" : "note-bad"}`} style={{ marginTop: 10 }}>
      <span>
        {probe.message}
        {probe.ok ? ` · ${formatDuration(probe.elapsedMs)}` : ""}
      </span>
    </div>
  ) : null;

  return (
    <>
      <h1 className="page-title">{t("tabOcr")}</h1>
      <p className="page-lead">{t("ocrEngineTitle")}</p>

      <Card title={t("channel")}>
        <Row title={t("channel")} hint={t(meta.descKey)}>
          <Select
            value={provider}
            onChange={(value) => setOcr({ provider: value as OcrProvider }, true)}
            options={OCR_ENGINES.map((engine) => ({
              value: engine.id,
              label: t(engine.titleKey),
            }))}
          />
        </Row>
      </Card>

      {provider === "local" ? (
        <Card title={t("ocrLocal")} note={t("ocrLocalDesc")}>
          <Field label={t("ocrLocalUrl")} hint={t("ocrLocalUrlHint")}>
            <TextInput
              mono
              value={config.ocr.baseUrl}
              onChange={(value) => setOcr({ baseUrl: value })}
              placeholder="http://127.0.0.1:1224"
            />
          </Field>
          <div className="row row-top" style={{ marginTop: 12 }}>
            <div className="row-main">
              <button className="btn" disabled={probing} onClick={() => void runProbe()}>
                {probing ? <span className="spinner" /> : null}
                {probing ? t("probeOcrRunning") : t("probeLocal")}
              </button>
            </div>
            {meta.applyUrl ? <ApplyLink url={meta.applyUrl} label={t("ocrLocalGet")} /> : null}
          </div>
          {probeNote}
          <EndpointHint config={config} kind="ocr" />
        </Card>
      ) : null}

      {provider === "picture" ? (
        <Card title={t("ocrPicture")} note={t("ocrPictureDesc")}>
          {picture ? (
            <Row
              title={t("ocrPictureCurrent", { name: t(PROVIDER_KEYS[picture]) })}
              hint={t("ocrPictureFallback")}
            >
              {pictureMeta ? <ApplyLink url={pictureMeta.applyUrl} /> : null}
            </Row>
          ) : (
            <div className="note note-warn">
              {t("ocrPictureUnavailable", { name: t(PROVIDER_KEYS[config.translate.provider]) })}
            </div>
          )}
          <EndpointHint config={config} kind="picture" />
        </Card>
      ) : null}

      {provider === "vision" ? (
        <>
          <Card title={t("endpointBaseUrl")}>
            <EndpointFields
              kind="ocr"
              baseUrl={config.ocr.baseUrl}
              apiKey={config.ocr.apiKey}
              model={config.ocr.model}
              set={(next) => setOcr(next, false)}
            />
            <EndpointHint config={config} kind="ocr" />
            <div style={{ marginTop: 12, borderTop: "1px solid var(--border)", paddingTop: 12 }}>
              <PresetPicker
                presets={PRESETS.filter((preset) => preset.visionModel)}
                currentUrl={config.ocr.baseUrl}
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
      ) : provider === "winocr" ? (
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
                <button className="btn" disabled={probing} onClick={() => void runProbe()}>
                  {probing ? <span className="spinner" /> : null}
                  {probing ? t("probeOcrRunning") : t("probeOcr")}
                </button>
              </div>
            </div>
            {probeNote}
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
      ) : null}
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
  const meta = engineMeta(provider);
  const profile = config.translate.profiles?.[provider] ?? EMPTY_PROFILE;
  const has = (field: ProfileField) => (meta?.fields ?? []).includes(field);

  const setTranslate = (next: Partial<AppConfig["translate"]>, immediate = false) =>
    patch((cfg) => ({ ...cfg, translate: { ...cfg.translate, ...next } }), immediate);

  /**
   * Writes into the selected engine's own profile. Keeping them apart is what
   * stops a key typed for one service from showing up under another one.
   */
  const setProfile = (next: Partial<TranslateProfile>, immediate = false) =>
    patch((cfg) => {
      const current = cfg.translate.profiles?.[provider] ?? EMPTY_PROFILE;
      return {
        ...cfg,
        translate: {
          ...cfg.translate,
          profiles: { ...cfg.translate.profiles, [provider]: { ...current, ...next } },
        },
      };
    }, immediate);

  const secretLabel =
    provider === "youdao"
      ? t("secretYoudao")
      : provider === "baidu"
        ? t("secretBaidu")
        : t("secretTencent");
  // Baidu's LLM endpoint authenticates either by signature or by a Bearer API
  // Key, so only the credential that is actually used is shown.
  const baiduLlm = provider === "baidu" && (profile.modelTier || "standard") === "llm";
  const keyAuth = baiduLlm && (profile.auth || "sign") === "key";
  const showSecret = has("apiSecret") && !keyAuth;

  return (
    <>
      <h1 className="page-title">{t("tabTranslate")}</h1>
      <p className="page-lead">{t("translateProvider")}</p>

      <Card title={t("channel")} note={meta ? t(meta.descKey) : undefined}>
        <Row title={t("channel")}>
          <Select
            value={provider}
            onChange={(value) => setTranslate({ provider: value as TranslateProvider }, true)}
            options={TRANSLATE_ENGINES.map((engine) => ({
              value: engine.id,
              label: t(engine.titleKey),
            }))}
          />
        </Row>
        {meta && meta.applyUrl ? (
          <Row title={t("applyUrl")} hint={meta.applyUrl}>
            <ApplyLink url={meta.applyUrl} />
          </Row>
        ) : null}
      </Card>

      {provider !== "none" ? (
        <Card title={t("targetLang")} note={t("targetLangHint")}>
          <Row title={t("targetLang")}>
            <Select
              value={config.translate.targetLang}
              onChange={(value) => setTranslate({ targetLang: value }, true)}
              options={withTarget(TARGET_LANGS, config.translate.targetLang).map((option) => ({
                value: option.value,
                label: targetOptionLabel(option.value, t("targetAuto")),
              }))}
            />
          </Row>
        </Card>
      ) : null}

      {provider === "openai" ? (
        <>
          <Card title={t("endpointBaseUrl")}>
            <EndpointFields
              kind="translate"
              baseUrl={profile.baseUrl}
              apiKey={profile.apiKey}
              model={profile.model}
              set={setProfile}
              onTest={() => api.testTranslateConnection(config.translate)}
            />
            <EndpointHint config={config} kind="translate" />
            <div style={{ marginTop: 12, borderTop: "1px solid var(--border)", paddingTop: 12 }}>
              <PresetPicker
                presets={PRESETS.filter((preset) => preset.textModel)}
                currentUrl={profile.baseUrl}
                onPick={(preset) => {
                  setProfile({ baseUrl: preset.baseUrl, model: preset.textModel }, true);
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

      {meta && provider !== "openai" && provider !== "none" ? (
        <Card
          title={t(meta.titleKey)}
          note={has("region") ? t("regionHint") : has("apiSecret") ? t("secretHint") : undefined}
        >
          <div className="stack">
            <Field
              label={t("endpointKey")}
              hint={provider === "deepl" ? t("deeplKeyHint") : t("endpointKeyHint")}
            >
              <SecretInput
                value={profile.apiKey}
                onChange={(value) => setProfile({ apiKey: value })}
                placeholder={meta.keyPlaceholder}
                showLabel={t("showKey")}
                hideLabel={t("hideKey")}
              />
            </Field>
            {showSecret ? (
              <Field label={secretLabel} hint={baiduLlm ? t("authHint") : undefined}>
                <SecretInput
                  value={profile.apiSecret}
                  onChange={(value) => setProfile({ apiSecret: value })}
                  placeholder="…"
                  showLabel={t("showKey")}
                  hideLabel={t("hideKey")}
                />
              </Field>
            ) : null}
            {has("region") ? (
              <Field label={t("region")} hint={t("regionHint")}>
                <TextInput
                  mono
                  value={profile.region}
                  onChange={(value) => setProfile({ region: value })}
                  placeholder={provider === "microsoft" ? "eastasia" : "ap-guangzhou"}
                />
              </Field>
            ) : null}
            {provider === "deepl" ? (
              <Field label={t("deeplPlan")}>
                <Select
                  value={config.translate.deeplPlan}
                  onChange={(value) =>
                    setTranslate({ deeplPlan: value as AppConfig["translate"]["deeplPlan"] }, true)
                  }
                  options={[
                    { value: "free", label: t("deeplFree") },
                    { value: "pro", label: t("deeplPro") },
                  ]}
                />
              </Field>
            ) : null}
            {baiduLlm ? (
              <Field label={t("authTitle")} hint={t("authHint")}>
                <Select
                  value={profile.auth || "sign"}
                  onChange={(value) => setProfile({ auth: value }, true)}
                  options={[
                    { value: "sign", label: t("authSign") },
                    { value: "key", label: t("authKey") },
                  ]}
                />
              </Field>
            ) : null}
            {keyAuth ? (
              <Field label={t("bearerTitle")} hint={t("bearerHint")}>
                <SecretInput
                  value={profile.bearer}
                  onChange={(value) => setProfile({ bearer: value })}
                  placeholder="…"
                  showLabel={t("showKey")}
                  hideLabel={t("hideKey")}
                />
              </Field>
            ) : null}
            {meta.picture ? (
              <div className="note" style={{ marginBottom: 12 }}>
                {t("translatePictureNote")}
              </div>
            ) : null}
            {meta.tiers ? (
              <Field label={t("tierTitle")} hint={t("tierHint")}>
                <Select
                  value={profile.modelTier || meta.tiers[0].value}
                  onChange={(value) => setProfile({ modelTier: value }, true)}
                  options={meta.tiers.map((tier) => ({
                    value: tier.value,
                    label: t(tier.labelKey),
                  }))}
                />
              </Field>
            ) : null}
            <TranslateTest config={config} />
            <EndpointHint config={config} kind="translate" />
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

/** Palette chooser: a card per theme with a live mini preview. */
function ThemePicker({
  value,
  onChange,
}: {
  value: AppConfig["ui"]["theme"];
  onChange: (next: AppConfig["ui"]["theme"]) => void;
}) {
  const { t } = useT();
  return (
    <div className="theme-grid">
      {THEMES.map((id) => {
        const active = value === id;
        const swatch = id === "system" ? null : THEME_SWATCHES[id];
        return (
          <button
            key={id}
            type="button"
            className="theme-card"
            aria-pressed={active}
            onClick={() => onChange(id)}
          >
            <span
              className="theme-preview"
              style={{
                background:
                  swatch?.bg ??
                  "linear-gradient(135deg, #f3f5f9 0 50%, #131519 50% 100%)",
              }}
            >
              <span
                className="theme-swatch-dot"
                style={{ background: swatch?.accent ?? "#4c6ef5" }}
              />
              <span
                className="theme-swatch-bar"
                style={{ background: swatch?.text ?? "#7c96ff" }}
              />
              <span
                className="theme-swatch-dot"
                style={{
                  background: swatch?.surface ?? "#ffffff",
                  boxShadow: "inset 0 0 0 1px rgba(0,0,0,.18)",
                }}
              />
            </span>
            <span className="theme-card-name">
              <span>{t(THEME_LABEL_KEYS[id])}</span>
              {active ? <span className="theme-card-check">✓</span> : null}
            </span>
          </button>
        );
      })}
    </div>
  );
}

export function AppearanceSection({ config, patch }: Base) {
  const { t } = useT();
  const setUi = (next: Partial<AppConfig["ui"]>, immediate = false) =>
    patch((cfg) => ({ ...cfg, ui: { ...cfg.ui, ...next } }), immediate);

  return (
    <>
      <h1 className="page-title">{t("tabAppearance")}</h1>

      <Card title={t("theme")} note={t("themeHint")}>
        <ThemePicker value={config.ui.theme} onChange={(next) => setUi({ theme: next }, true)} />
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
        <Row title={t("resultShowImage")} hint={t("resultShowImageHint")}>
          <Switch
            checked={config.ui.resultShowImage}
            onChange={(next) => setUi({ resultShowImage: next }, true)}
          />
        </Row>
        <Row title={t("autoCopy")} hint={t("autoCopyHint")}>
          <Switch checked={config.ui.autoCopy} onChange={(next) => setUi({ autoCopy: next }, true)} />
        </Row>
        <Row title={t("notifyCard")} hint={t("notifyCardHint")}>
          <Switch checked={config.ui.notify} onChange={(next) => setUi({ notify: next }, true)} />
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

/* ==================================================================== logs */

const LOG_LINE_LIMIT = 1200;

type LogLevel = "ERROR" | "WARN" | "INFO" | "DEBUG";
const LOG_LEVELS: LogLevel[] = ["ERROR", "WARN", "INFO", "DEBUG"];

/**
 * The level a line was written at. Handles both the current
 * `2026-10-18 10:53:55 WARN …` and the older `[02:53:55Z] WARN …` shape.
 */
function lineLevel(line: string): LogLevel | null {
  const match = line.match(
    /^\s*(?:\[[^\]]+\]|\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})\s+(ERROR|WARN|INFO|DEBUG|TRACE)\b/,
  );
  if (!match) return null;
  return match[1] === "TRACE" ? "DEBUG" : (match[1] as LogLevel);
}

function lineTone(line: string): string {
  if (/ERROR|失败|出错|未能|不可用|未授权|error|failed/i.test(line)) return "bad";
  if (/WARN|超时|重试|改用|warn/i.test(line)) return "warn";
  if (/本次任务|接口响应|正在调用|框选层|已冻结/.test(line)) return "info";
  return "";
}

/** The type of a rendered line, highlighted by tone. */
function lineToneOf(line: string): string {
  const level = lineLevel(line);
  if (level === "ERROR") return "bad";
  if (level === "WARN") return "warn";
  if (level) return "info";
  return lineTone(line);
}

const LEVEL_KEYS: Record<LogLevel, "logsLevelError" | "logsLevelWarn" | "logsLevelInfo" | "logsLevelDebug"> = {
  ERROR: "logsLevelError",
  WARN: "logsLevelWarn",
  INFO: "logsLevelInfo",
  DEBUG: "logsLevelDebug",
};

function humanBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
}

/**
 * Reads the tail of the log file so a run can be explained after the fact:
 * which engine ran, which URL was called, what came back, why it failed.
 */
export function LogsSection({ notify }: Base) {
  const { t } = useT();
  const [file, setFile] = useState<LogFile | null>(null);
  const [query, setQuery] = useState("");
  const [levels, setLevels] = useState<Record<LogLevel, boolean>>({
    ERROR: true,
    WARN: true,
    INFO: true,
    DEBUG: true,
  });
  const [auto, setAuto] = useState(true);

  const load = useCallback(async () => {
    try {
      setFile(await api.readLog());
    } catch (error) {
      notify(String(error), true);
    }
  }, [notify]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!auto) return;
    const timer = window.setInterval(() => void load(), 2500);
    return () => window.clearInterval(timer);
  }, [auto, load]);

  const shown = useMemo(() => {
    const lines = (file?.content ?? "").split("\n").filter((line) => line.trim() !== "");
    const needle = query.trim().toLowerCase();
    const kept = lines.filter((line) => {
      const level = lineLevel(line);
      // Lines with no level of their own (continuations, third-party output)
      // are treated as INFO so a level filter never hides them silently.
      if (!levels[level ?? "INFO"]) return false;
      if (needle && !line.toLowerCase().includes(needle)) return false;
      return true;
    });
    // Newest first: the file is append-only, so read it top-down from the end.
    return kept.reverse();
  }, [file, query, levels]);

  const clipped = shown.length > LOG_LINE_LIMIT;
  // Newest first, so the cap keeps the newest lines (at the start of the list).
  const visible = clipped ? shown.slice(0, LOG_LINE_LIMIT) : shown;

  const copyAll = async () => {
    const ok = await copyText(visible.join("\n"));
    notify(ok ? t("copied") : t("copyFailed", { message: "" }), !ok);
  };

  const clear = async () => {
    if (!window.confirm(t("logsClearConfirm"))) return;
    try {
      await api.clearLog();
      await load();
      notify(t("saved"));
    } catch (error) {
      notify(String(error), true);
    }
  };

  const notes = [file ? humanBytes(file.bytes) : "", clipped ? t("logsTail") : ""].filter(Boolean).join(" · ");

  return (
    <>
      <h1 className="page-title">{t("tabLogs")}</h1>
      <p className="page-lead">{t("logsLead")}</p>

      <Card
        title={t("logsFile")}
        note={notes}
        actions={
          <>
            <button className="btn btn-sm btn-ghost" onClick={() => void load()}>
              {t("logsRefresh")}
            </button>
            <button className="btn btn-sm btn-ghost" onClick={() => void api.revealLog()}>
              {t("logsReveal")}
            </button>
            <button
              className="btn btn-sm btn-ghost"
              disabled={shown.length === 0}
              onClick={() => void copyAll()}
            >
              {t("logsCopy")}
            </button>
            <button className="btn btn-sm btn-danger" onClick={() => void clear()}>
              {t("logsClear")}
            </button>
          </>
        }
      >
        <Row title={t("logsFile")} hint={file?.path ?? "…"}>
          <div className="logs-tools">
            <Switch checked={auto} onChange={setAuto} title={t("logsAuto")} />
          </div>
        </Row>
        <div className="logs-levels">
          <span className="field-label">{t("logsLevel")}</span>
          {LOG_LEVELS.map((level) => (
            <button
              key={level}
              className={cx("logs-level", level.toLowerCase(), levels[level] && "on")}
              onClick={() => setLevels((current) => ({ ...current, [level]: !current[level] }))}
            >
              {t(LEVEL_KEYS[level])}
            </button>
          ))}
        </div>
        <div className="logs-filter">
          <TextInput value={query} onChange={setQuery} placeholder={t("logsFilter")} />
        </div>
        <div className="logs-view">
          {visible.length === 0 ? (
            <div className="empty">
              <div style={{ fontWeight: 500, color: "var(--text-soft)" }}>
                {file?.exists && file.bytes > 0 ? t("logsNoMatch") : t("logsEmpty")}
              </div>
              <div style={{ marginTop: 4 }}>{t("logsEmptyHint")}</div>
            </div>
          ) : (
            visible.map((line, index) => (
              <div className={cx("logs-line", lineToneOf(line))} key={index}>
                {line}
              </div>
            ))
          )}
        </div>
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
              {t("aboutVersion")} {__APP_VERSION__} ·{" "}
              {t(OCR_ENGINES.find((engine) => engine.id === config.ocr.provider)?.titleKey ?? "ocrWin")} +{" "}
              {t(PROVIDER_KEYS[config.translate.provider])}
            </div>
          </div>
          <div className="row-control">
            <span className="badge badge-accent">
              {config.shortcut.replace(/Key/, "")}
            </span>
          </div>
        </div>
        <div className="row row-top">
          <div className="row-main">
            <div className="row-title">{t("guideOpen")}</div>
            <div className="row-hint">{t("guideHint")}</div>
          </div>
          <div className="row-control">
            <button className="btn btn-sm" onClick={() => void api.openGuide()}>
              {t("guideOpen")}
            </button>
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
