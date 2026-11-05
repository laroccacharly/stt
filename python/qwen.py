# /// script
# requires-python = "==3.12.*"
# dependencies = ["huggingface-hub==2.4.0", "onnxruntime-gpu[cuda,cudnn]==1.31.1", "numpy", "tokenizers"]
# ///
"""Qwen3-ASR ONNX server for stt on an NVIDIA GPU (see serve.py)."""

from concurrent.futures import ThreadPoolExecutor

import json
import sys
from pathlib import Path

import numpy as np
import onnxruntime as ort
from huggingface_hub import snapshot_download
from tokenizers import Tokenizer

MODEL = "jinhwan000/qwen3-asr-1.7b-onnx"
REVISION = "0829891c64b05118d40107beed30880583c3c980"
FILES = ["config.json", "hann_400.f32", "mel_filters.f32", "encoder.fp32.onnx", "encoder.fp32.onnx.data", "decoder_init.int4.onnx", "decoder_step.int4.onnx", "decoder_weights.int4.data", "embed_tokens.bin", "tokenizer.json"]
model_dir = Path(snapshot_download(MODEL, revision=REVISION, local_dir=Path(__file__).parent / "qwen-model", allow_patterns=FILES))
if "--prepare" in sys.argv:
    sys.exit()

from serve import serve

# The CUDA and cuDNN libraries come from the nvidia wheels the [cuda,cudnn] extras install.
ort.preload_dlls()
# The default arena doubles each time it grows, which runs an 8 GB card out of memory.
providers = [("CUDAExecutionProvider", {"arena_extend_strategy": "kSameAsRequested"}), "CPUExecutionProvider"]
encoder = ort.InferenceSession(str(model_dir / "encoder.fp32.onnx"), providers=["CPUExecutionProvider"])
init = ort.InferenceSession(str(model_dir / "decoder_init.int4.onnx"), providers=["CPUExecutionProvider"])
session_options = ort.SessionOptions()
session_options.enable_mem_pattern = False
step = ort.InferenceSession(str(model_dir / "decoder_step.int4.onnx"), sess_options=session_options, providers=providers)
config = json.loads((model_dir / "config.json").read_text())
embeddings = np.fromfile(model_dir / "embed_tokens.bin", np.float16).reshape(-1, config["decoder"]["hidden_size"])
tokenizer = Tokenizer.from_file(str(model_dir / "tokenizer.json"))
hann = np.fromfile(model_dir / "hann_400.f32", np.float32)
filters = np.fromfile(model_dir / "mel_filters.f32", np.float32).reshape(128, 201)


def log_mel(audio):
    audio = np.pad(audio, 200, mode="reflect")
    frames = np.stack([audio[i:i + 400] * hann for i in range(0, len(audio) - 399, 160)])
    spectrum = np.fft.rfft(frames, n=400, axis=1)
    mel = np.log10(np.maximum((spectrum.real ** 2 + spectrum.imag ** 2) @ filters.T, 1e-10))
    mel = np.maximum(mel, mel.max() - 8.0)
    return ((mel[:-1].T + 4.0) / 4.0).astype(np.float32)[None]


def transcribe(pcm):
    # Bound decoder context and CUDA allocations for long recordings.
    if len(pcm) > 256000:
        with ThreadPoolExecutor(max_workers=4) as pool:
            return " ".join(pool.map(transcribe, (pcm[i:i + 256000] for i in range(0, len(pcm), 256000))))
    audio = np.frombuffer(pcm, dtype="<i2").astype(np.float32) / 32768
    if len(audio) < 400:
        return ""
    mel = log_mel(audio)
    features = np.concatenate([encoder.run(["audio_features"], {"mel": mel[:, :, i:i + 800]})[0] for i in range(0, mel.shape[2], 800)], axis=1)
    prompt = [151644, 8948, 198, 151645, 198, 151644, 872, 198, 151669] + [151676] * features.shape[1] + [151670, 151645, 198, 151644, 77091, 198]
    position = len(prompt)
    logits, keys, values = init.run(None, {"input_ids": np.array([prompt], np.int64), "position_ids": np.arange(position, dtype=np.int64)[None], "audio_features": features, "audio_offset": np.array([prompt.index(151676)], np.int64)})
    token = int(logits[0, -1].argmax())
    output = []
    budget = max(16, int(len(audio) / 16000 * 13))
    while token not in (151643, 151645) and len(output) < budget:
        output.append(token)
        logits, keys, values = step.run(None, {"input_embeds": embeddings[token].astype(np.float32)[None, None], "position_ids": np.array([[position]], np.int64), "past_keys": keys, "past_values": values})
        token = int(logits[0, -1].argmax())
        position += 1
    if token not in (151643, 151645):
        raise RuntimeError("Qwen transcript exceeded its token budget")
    return tokenizer.decode(output).split("<asr_text>")[-1].strip()


# The first run on the GPU is slow, so it happens before the socket accepts a dictation.
transcribe(bytes(32000))
serve(transcribe)
