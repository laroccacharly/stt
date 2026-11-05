// Everything records and transcribes 16-bit little-endian mono PCM at this rate.
export const SAMPLE_RATE = 16_000

const bytesIn = (milliseconds: number) => Math.floor((milliseconds * SAMPLE_RATE) / 1000) * 2

const FRAME_BYTES = bytesIn(20)
const PAUSE_SEARCH_MS = 5000

const samplesOf = (pcm: Uint8Array) => new Int16Array(pcm.buffer.slice(pcm.byteOffset, pcm.byteOffset + (pcm.byteLength & ~1)))

// Sum of squared samples: unlike level, it does not clip loud audio.
const energy = (frame: Uint8Array) => samplesOf(frame).reduce((sum, sample) => sum + sample * sample, 0)

// Perceptual loudness of a chunk, mapped to 0..1 for the overlay.
export const level = (chunk: Uint8Array): number => {
  const samples = samplesOf(chunk)
  const sum = samples.reduce((total, sample) => total + (sample / 32768) ** 2, 0)
  const rms = Math.sqrt(sum / Math.max(samples.length, 1))
  return Math.min(Math.sqrt(rms) * 2.2, 1)
}

const quietestFrameStart = (window: Uint8Array) => {
  let quietest = { start: 0, energy: Number.POSITIVE_INFINITY }
  for (let start = 0; start + FRAME_BYTES <= window.length; start += FRAME_BYTES) {
    const frameEnergy = energy(window.subarray(start, start + FRAME_BYTES))
    if (frameEnergy < quietest.energy) {
      quietest = { start, energy: frameEnergy }
    }
  }
  return quietest.start
}

const pauseCut = (pcm: Uint8Array, maxBytes: number, searchBytes: number) => {
  const windowStart = maxBytes - searchBytes
  return (windowStart + quietestFrameStart(pcm.subarray(windowStart, maxBytes)) + FRAME_BYTES / 2) & ~1
}

// Cutting at a quiet frame helps avoid splitting words.
export const splitAtPauses = (pcm: Uint8Array, maxMilliseconds: number): ReadonlyArray<Uint8Array> => {
  const maxBytes = Math.max(bytesIn(maxMilliseconds), FRAME_BYTES)
  const searchBytes = Math.min(bytesIn(PAUSE_SEARCH_MS), Math.floor(maxBytes / 2))
  const pieces: Uint8Array[] = []
  let rest = pcm
  while (rest.length > maxBytes) {
    const cut = pauseCut(rest, maxBytes, searchBytes)
    pieces.push(rest.subarray(0, cut))
    rest = rest.subarray(cut)
  }
  if (rest.length > 0) {
    pieces.push(rest)
  }
  return pieces
}

export const wav = (pcm: Uint8Array): Uint8Array<ArrayBuffer> => {
  const out = new Uint8Array(44 + pcm.length)
  const view = new DataView(out.buffer)
  const ascii = (offset: number, text: string) => out.set(new TextEncoder().encode(text), offset)
  ascii(0, "RIFF")
  view.setUint32(4, 36 + pcm.length, true)
  ascii(8, "WAVEfmt ")
  view.setUint32(16, 16, true)
  view.setUint16(20, 1, true)
  view.setUint16(22, 1, true)
  view.setUint32(24, SAMPLE_RATE, true)
  view.setUint32(28, SAMPLE_RATE * 2, true)
  view.setUint16(32, 2, true)
  view.setUint16(34, 16, true)
  ascii(36, "data")
  view.setUint32(40, pcm.length, true)
  out.set(pcm, 44)
  return out
}

// Bytes before the samples in a WAV that `wav` wrote.
export const WAV_HEADER_BYTES = 44

// Whether `header` starts a WAV that `wav` wrote for a file of `size` bytes, whose samples are then read as is.
export const isOwnWav = (header: Uint8Array, size: number): boolean => {
  if (header.length < WAV_HEADER_BYTES) {
    return false
  }
  const view = new DataView(header.buffer, header.byteOffset, header.byteLength)
  const ascii = (offset: number, length: number) => new TextDecoder().decode(header.subarray(offset, offset + length))
  return (
    ascii(0, 4) === "RIFF" &&
    ascii(8, 8) === "WAVEfmt " &&
    view.getUint32(16, true) === 16 &&
    view.getUint16(20, true) === 1 &&
    view.getUint16(22, true) === 1 &&
    view.getUint32(24, true) === SAMPLE_RATE &&
    view.getUint16(34, true) === 16 &&
    ascii(36, 4) === "data" &&
    view.getUint32(40, true) === size - WAV_HEADER_BYTES
  )
}

export const concatenate = (chunks: ReadonlyArray<Uint8Array>): Uint8Array => {
  const out = new Uint8Array(chunks.reduce((total, chunk) => total + chunk.length, 0))
  let offset = 0
  for (const chunk of chunks) {
    out.set(chunk, offset)
    offset += chunk.length
  }
  return out
}
