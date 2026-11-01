import { Effect, Option, Schema } from "effect"
import * as JsonStore from "effect-lib/json-store"
import { configPath } from "../paths.ts"
import { DEFAULT_PROVIDER, ProviderName } from "../provider/provider.ts"

// Settings saved by `stt provider set`, in `$XDG_CONFIG_HOME/stt/config.json`.
export const Config = Schema.Struct({ provider: ProviderName })

export const configStore = JsonStore.make(configPath(), Config)

export const currentProvider = configStore.load.pipe(Effect.map(Option.match({ onNone: () => DEFAULT_PROVIDER, onSome: (config) => config.provider })))

export const saveProvider = (provider: ProviderName) => configStore.save({ provider })
