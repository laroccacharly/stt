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

import importlib.util
import sys

import numpy as np
from huggingface_hub import hf_hub_download

# Pinned so a change upstream can't alter what `stt provider set` downloads.
REPO = "ConwayResearch/Underdog-Bark-3.0-Clean"
REVISION = "6d34ed8639b2b9a33e764acf06748519bbb950cf"

weights = hf_hub_download(REPO, "model.bark", revision=REVISION)
loader = hf_hub_download(REPO, "bark_loader.py", revision=REVISION)

spec = importlib.util.spec_from_file_location("bark_loader", loader)
bark_loader = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bark_loader)

model = bark_loader.load_model(weights, device="cpu")
if "--prepare" in sys.argv:
    # The loader fetches this lazily on the first transcription.
    from transformers import AutoProcessor

    AutoProcessor.from_pretrained("nvidia/parakeet-tdt-0.6b-v3")
    sys.exit()

from serve import serve


def transcribe(pcm: bytes) -> str:
    samples = np.frombuffer(pcm, dtype="<i2").astype(np.float32) / 32768
    return bark_loader.transcribe(model, samples, sample_rate=16_000)


serve(transcribe)
