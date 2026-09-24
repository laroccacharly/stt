import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";

const runtime = join(process.env.XDG_RUNTIME_DIR ?? `/run/user/${process.getuid?.() ?? 1000}`, "stt");
const pidFile = join(runtime, "recording.pid");
const logFile = join(runtime, "stt.log");
const root = import.meta.dir.replace(/\/src$/, "");
const model = "scribe_v2_realtime";

async function notify(title: string, body: string, timeout = 1500): Promise<void> {
  const proc = Bun.spawn(["notify-send", "-a", "stt", "-r", "47511", "-t", String(timeout), title, body], { stdout: "ignore", stderr: "ignore" });
  await proc.exited;
}

async function existingPid(): Promise<number | null> {
  try {
    const pid = Number((await readFile(pidFile, "utf8")).trim());
    if (!Number.isSafeInteger(pid) || pid <= 0) return null;
    process.kill(pid, 0);
    return pid;
  } catch {
    return null;
  }
}

async function key(): Promise<string> {
  const value = process.env.ELEVENLABS_API_KEY || await Bun.secrets.get({ service: "cterm", name: "ELEVENLABS_API_KEY" });
  if (!value) throw new Error("No ElevenLabs key: set ELEVENLABS_API_KEY or run cterm login");
  return value;
}

let typeQueue = Promise.resolve();
function writeText(text: string): void {
  if (!text.trim()) return;
  typeQueue = typeQueue.then(async () => {
    const proc = Bun.spawn(["wtype", text], { stdout: "ignore", stderr: "pipe" });
    const code = await proc.exited;
    if (code !== 0) {
      const error = await new Response(proc.stderr).text();
      console.error(`wtype failed: ${error}`);
      await notify("STT typing failed", error || "Check the focused window", 4000);
    }
  });
}

async function record(): Promise<void> {
  await mkdir(runtime, { recursive: true });
  await writeFile(pidFile, String(process.pid), { flag: "wx" });
  let recorder: ReturnType<typeof Bun.spawn> | undefined;
  let socket: WebSocket | undefined;
  let stopped = false;
  let committed = "";
  let pending: Uint8Array | undefined;
  let finishResolve: (() => void) | undefined;
  const finished = new Promise<void>(resolve => { finishResolve = resolve; });
  const finish = () => finishResolve?.();
  const stop = () => {
    if (stopped) return;
    stopped = true;
    recorder?.kill("SIGTERM");
    if (socket?.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify({ message_type: "input_audio_chunk", audio_base_64: pending ? Buffer.from(pending).toString("base64") : "", commit: true }));
      setTimeout(finish, 4500);
    } else finish();
    void notify("STT", "Finishing transcript…");
  };
  process.on("SIGUSR2", stop);
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);

  try {
    const apiKey = await key();
    const url = new URL("wss://api.elevenlabs.io/v1/speech-to-text/realtime");
    url.searchParams.set("model_id", model);
    url.searchParams.set("audio_format", "pcm_16000");
    url.searchParams.set("commit_strategy", "vad");
    const BunWebSocket = WebSocket as unknown as new (url: URL, options: { headers: Record<string, string> }) => WebSocket;
    socket = new BunWebSocket(url, { headers: { "xi-api-key": apiKey } });
    socket.addEventListener("open", () => {
      console.log("ElevenLabs WebSocket connected");
      recorder = Bun.spawn(["pw-record", "--raw", "--format", "s16", "--rate", "16000", "--channels", "1", "-"], { stdout: "pipe", stderr: "pipe" });
      void (async () => {
        try {
          for await (const chunk of recorder.stdout as ReadableStream<Uint8Array>) {
            if (stopped || socket?.readyState !== WebSocket.OPEN) break;
            if (pending) socket.send(JSON.stringify({ message_type: "input_audio_chunk", audio_base_64: Buffer.from(pending).toString("base64") }));
            pending = chunk;
          }
          if (!stopped) stop();
        } catch (error) { console.error(error); stop(); }
      })();
      void notify("STT recording", "Speak now; press End again to stop", 2500);
    });
    socket.addEventListener("message", event => {
      try {
        const data = JSON.parse(String(event.data)) as { message_type: string; text?: string; error?: string };
        if (data.message_type === "session_started") {
          console.log("ElevenLabs transcription session started");
        } else if (data.message_type === "partial_transcript" && data.text && !stopped) {
          void notify("STT live", `${committed}${data.text}`.slice(-300));
        } else if (data.message_type === "committed_transcript") {
          const text = data.text?.trim();
          if (text) {
            const segment = `${committed ? " " : ""}${text}`;
            committed += segment;
            writeText(segment);
          }
          if (stopped) finish();
        } else if (data.message_type.endsWith("error") || data.message_type === "rate_limited") {
          console.error(`ElevenLabs: ${data.error ?? data.message_type}`);
          void notify("STT error", data.error ?? data.message_type, 5000);
          finish();
        }
      } catch (error) { console.error(error); }
    });
    socket.addEventListener("error", () => { console.error("ElevenLabs WebSocket error"); void notify("STT error", "Connection failed", 5000); finish(); });
    socket.addEventListener("close", () => finish());
    await finished;
    await typeQueue;
    if (committed) await notify("STT complete", committed.slice(-300), 2500);
  } finally {
    recorder?.kill("SIGTERM");
    socket?.close();
    await rm(pidFile, { force: true });
  }
}

async function main(): Promise<void> {
  const command = process.argv[2] ?? "toggle";
  await mkdir(runtime, { recursive: true });
  if (command === "record") { await record(); return; }
  if (command === "status") { console.log((await existingPid()) ? "recording" : "idle"); return; }
  if (command !== "toggle") throw new Error("Usage: stt [toggle|status|record]");
  const pid = await existingPid();
  if (pid) { process.kill(pid, "SIGUSR2"); return; }
  await rm(pidFile, { force: true });
  const child = Bun.spawn([process.execPath, join(root, "src/main.ts"), "record"], {
    cwd: root, stdin: "ignore", stdout: Bun.file(logFile), stderr: Bun.file(logFile),
    env: process.env,
  });
  child.unref();
  for (let i = 0; i < 20; i++) {
    if (await existingPid()) return;
    await Bun.sleep(50);
  }
  throw new Error(`STT did not start; inspect ${logFile}`);
}

await main().catch(async error => {
  console.error(error);
  await notify("STT error", String(error), 5000);
  process.exitCode = 1;
});
