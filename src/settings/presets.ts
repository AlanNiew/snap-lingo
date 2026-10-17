/**
 * One-click endpoints for the services people actually use.
 *
 * Keys are never prefilled: half the presets would have to be lies, and a
 * stored key that silently does not work is worse than an empty box.
 */
import type { ProviderPreset } from "../lib/types";

export const PRESETS: ProviderPreset[] = [
  {
    id: "nvidia",
    label: "NVIDIA NIM",
    hint: "免费额度友好，视觉与文本模型都齐全",
    kind: "both",
    baseUrl: "https://integrate.api.nvidia.com/v1",
    visionModel: "meta/llama-3.2-11b-vision-instruct",
    textModel: "nvidia/nemotron-3-ultra-550b-a55b",
  },
  {
    id: "zhipu",
    label: "智谱 GLM",
    hint: "glm-4-flash 便宜快速，glm-4v 支持看图",
    kind: "both",
    baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    visionModel: "glm-4v-flash",
    textModel: "glm-4-flash",
  },
  {
    id: "deepseek",
    label: "DeepSeek",
    hint: "中文与代码翻译性价比高，无视觉模型",
    kind: "translate",
    baseUrl: "https://api.deepseek.com/v1",
    visionModel: "",
    textModel: "deepseek-chat",
  },
  {
    id: "dashscope",
    label: "阿里云百炼（通义）",
    hint: "qwen-vl-max 识别中文排版效果好",
    kind: "both",
    baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    visionModel: "qwen-vl-max",
    textModel: "qwen-plus",
  },
  {
    id: "openai",
    label: "OpenAI",
    hint: "gpt-4o-mini 兼顾质量与价格",
    kind: "both",
    baseUrl: "https://api.openai.com/v1",
    visionModel: "gpt-4o-mini",
    textModel: "gpt-4o-mini",
  },
  {
    id: "siliconflow",
    label: "硅基流动",
    hint: "国内直连，模型选择多",
    kind: "both",
    baseUrl: "https://api.siliconflow.cn/v1",
    visionModel: "Qwen/Qwen2.5-VL-72B-Instruct",
    textModel: "Qwen/Qwen2.5-7B-Instruct",
  },
  {
    id: "moonshot",
    label: "月之暗面 Kimi",
    hint: "长文本与中文语义强",
    kind: "both",
    baseUrl: "https://api.moonshot.cn/v1",
    visionModel: "moonshot-v1-8k-vision-preview",
    textModel: "moonshot-v1-8k",
  },
  {
    id: "openrouter",
    label: "OpenRouter",
    hint: "一个密钥调用多家模型",
    kind: "both",
    baseUrl: "https://openrouter.ai/api/v1",
    visionModel: "openai/gpt-4o-mini",
    textModel: "openai/gpt-4o-mini",
  },
  {
    id: "ollama",
    label: "Ollama（本地）",
    hint: "完全离线运行，无需密钥",
    kind: "both",
    baseUrl: "http://localhost:11434/v1",
    visionModel: "qwen2.5vl:7b",
    textModel: "qwen2.5:7b",
  },
  {
    id: "lmstudio",
    label: "LM Studio（本地）",
    hint: "本地加载任意 gguf 模型",
    kind: "both",
    baseUrl: "http://localhost:1234/v1",
    visionModel: "qwen2-vl-7b-instruct",
    textModel: "qwen2.5-7b-instruct",
  },
];

export function presetFor(id: string): ProviderPreset | undefined {
  return PRESETS.find((preset) => preset.id === id);
}
