# /// script
# requires-python = "==3.12.*"
# dependencies = ["fermion-research==0.2.11", "torch"]
#
# [tool.uv.sources]
# torch = { index = "pytorch-cpu" }
#
# [[tool.uv.index]]
# name = "pytorch-cpu"
# url = "https://download.pytorch.org/whl/cpu"
# explicit = true
# ///
"""Phonon-2 server for stt (see serve.py). `--prepare` only downloads the
model and its dependencies."""

import sys

import fermion
import numpy as np

speech = fermion.load_speech("phonon-2")
if "--prepare" in sys.argv:
    sys.exit()

from serve import serve


def transcribe(pcm: bytes) -> str:
    samples = np.frombuffer(pcm, dtype="<i2").astype(np.float32) / 32768
    return speech.transcribe_array(samples)[0]


serve(transcribe)
