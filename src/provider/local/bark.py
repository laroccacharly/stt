# /// script
# requires-python = "==3.12.*"
# dependencies = ["constriction", "huggingface-hub", "librosa", "numpy", "torch", "transformers>=5.19"]
#
# [tool.uv.sources]
# torch = { index = "pytorch-cpu" }
#
# [[tool.uv.index]]
# name = "pytorch-cpu"
# url = "https://download.pytorch.org/whl/cpu"
# explicit = true
# ///
"""Underdog Bark 3.0 Clean server for stt (see serve.py). `--prepare` only
downloads the model and its dependencies."""

import ctypes
import gc
import importlib.util
import sys

import numpy as np
import torch
from huggingface_hub import hf_hub_download
from transformers import AutoProcessor

# Pinned so a change upstream can't alter what `stt provider set` downloads.
REPO = "ConwayResearch/Underdog-Bark-3.0-Clean"
REVISION = "6d34ed8639b2b9a33e764acf06748519bbb950cf"
PROCESSOR = "nvidia/parakeet-tdt-0.6b-v3"

# bf16 is ~2.5x faster than fp32 on CPUs with native support; fp16 is much
# slower on x86, so only use it where the CPU has the AVX512 BF16 extension.
bf16 = getattr(torch.cpu, "_is_avx512_bf16_supported", lambda: False)()

weights = hf_hub_download(REPO, "model.bark", revision=REVISION)
loader = hf_hub_download(REPO, "bark_loader.py", revision=REVISION)

spec = importlib.util.spec_from_file_location("bark_loader", loader)
bark_loader = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bark_loader)

model = bark_loader.load_model(
    weights, device="cpu", dtype=torch.bfloat16 if bf16 else torch.float32
)
# Loading builds an fp32 copy before converting it, and glibc keeps the freed
# pages; return them to the OS (5.5 GB -> 1.7 GB here).
gc.collect()
ctypes.CDLL("libc.so.6").malloc_trim(0)
# The loader reloads this for every transcription, which costs most of a
# second. Keep it for the life of the server instead.
processor = AutoProcessor.from_pretrained(PROCESSOR)
if "--prepare" in sys.argv:
    sys.exit()

from serve import serve


def transcribe(pcm: bytes) -> str:
    samples = np.frombuffer(pcm, dtype="<i2").astype(np.float32) / 32768
    inputs = processor([samples], sampling_rate=16_000, return_tensors="pt", padding=True)
    dtype = next(model.parameters()).dtype
    inputs = {
        name: value.to(dtype) if value.dtype.is_floating_point else value
        for name, value in inputs.items()
    }
    with torch.inference_mode():
        output = model.generate(**inputs)
    return processor.batch_decode(output.sequences, skip_special_tokens=True)[0].strip()


serve(transcribe)
