import { Schema } from "effect"

export const ProviderName = Schema.Literals(["phonon-2.5-en", "qwen3-asr-1.7b", "openrouter"])
export type ProviderName = typeof ProviderName.Type

export interface ProviderInfo {
  readonly label: string
  // The model that transcribes, as the provider names it; saved in the history.
  readonly model: string
}

export const PROVIDERS: Readonly<Record<ProviderName, ProviderInfo>> = {
  "phonon-2.5-en": { label: "Phonon-2.5 English (local)", model: "FermionResearch/Phonon-2.5-English" },
  "qwen3-asr-1.7b": { label: "Qwen3-ASR 1.7B (local GPU)", model: "Qwen/Qwen3-ASR-1.7B" },
  openrouter: { label: "OpenRouter (MAI-Transcribe 2)", model: "microsoft/mai-transcribe-2" },
}

export const DEFAULT_PROVIDER: ProviderName = "phonon-2.5-en"
