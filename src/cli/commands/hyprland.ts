import { Command } from "effect/cli"
import { install, uninstall } from "../../hyprland/hyprland.ts"

export const installCommand = Command.make("install", {}, () => install).pipe(Command.withDescription("Bind End to `stt toggle` in Hyprland"))

export const uninstallCommand = Command.make("uninstall", {}, () => uninstall).pipe(Command.withDescription("Remove the End binding"))
