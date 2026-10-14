# /// script
# requires-python = "==3.12.*"
# dependencies = ["moondream==2.6.1", "torch"]
#
# [tool.uv.sources]
# torch = { index = "pytorch-cpu" }
#
# [[tool.uv.index]]
# name = "pytorch-cpu"
# url = "https://download.pytorch.org/whl/cpu"
# explicit = true
# ///
"""Parakeet Redux server for stt (see serve.py). `--prepare` only downloads the
model and its dependencies."""

import sys
import tempfile
import wave

import moondream as md

with md.photon("moondream/parakeet-redux", device="cpu") as speech:
    if "--prepare" in sys.argv:
        sys.exit()

    from serve import serve

    def transcribe(pcm: bytes) -> str:
        with tempfile.NamedTemporaryFile(suffix=".wav") as file:
            with wave.open(file.name, "wb") as recording:
                recording.setnchannels(1)
                recording.setsampwidth(2)
                recording.setframerate(16_000)
                recording.writeframes(pcm)
            return speech.transcribe(audio=file.name)["text"]

    serve(transcribe)
