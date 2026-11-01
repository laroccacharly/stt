import { expect, test } from "bun:test"
import { level, SAMPLE_RATE, splitAtPauses, wav } from "../src/audio/pcm.ts"

const tone = (seconds: number, amplitude: number) => {
  const samples = new Int16Array(SAMPLE_RATE * seconds).fill(amplitude)
  return new Uint8Array(samples.buffer)
}
const bytesAt = (seconds: number) => SAMPLE_RATE * seconds * 2
const CHUNK = bytesAt(0.1)

test("splits in the pause before the limit, keeping every sample", () => {
  const pcm = tone(30, 8000)
  const pause = bytesAt(27)
  pcm.fill(0, pause, pause + CHUNK)

  const pieces = splitAtPauses(pcm, 28_000)

  expect(pieces).toHaveLength(2)
  expect(pieces[0]?.length).toBeGreaterThanOrEqual(pause)
  expect(pieces[0]?.length).toBeLessThan(pause + CHUNK)
  expect(Buffer.concat(pieces)).toEqual(Buffer.from(pcm))
})

test("splits at the quietest stretch of loud audio", () => {
  const pcm = tone(30, 16000)
  const quieter = bytesAt(27)
  new Int16Array(pcm.buffer, quieter, CHUNK / 2).fill(8000)

  const [first] = splitAtPauses(pcm, 28_000)

  expect(first?.length).toBeGreaterThanOrEqual(quieter)
  expect(first?.length).toBeLessThan(quieter + CHUNK)
})

test("short audio is one piece", () => {
  expect(splitAtPauses(new Uint8Array(640), 30_000)).toHaveLength(1)
})

test("level is zero for silence and clamped at full scale", () => {
  expect(level(new Uint8Array(320))).toBe(0)
  expect(level(new Uint8Array(new Int16Array(160).fill(32767).buffer))).toBe(1)
})

test("the WAV header describes the PCM", () => {
  const out = wav(new Uint8Array([1, 2, 3, 4]))
  const view = new DataView(out.buffer)

  expect(out.length).toBe(48)
  expect(new TextDecoder().decode(out.subarray(0, 4))).toBe("RIFF")
  expect(view.getUint32(4, true)).toBe(40)
  expect(view.getUint32(24, true)).toBe(16_000)
  expect(view.getUint32(40, true)).toBe(4)
  expect([...out.subarray(44)]).toEqual([1, 2, 3, 4])
})
