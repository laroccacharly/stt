# /// script
# requires-python = "==3.12.*"
# dependencies = ["tinybark[export]"]
#
# [tool.uv.sources]
# tinybark = { path = "$STT_DIR/../tinybark", editable = true }
# torch = { index = "pytorch-cpu" }
#
# [[tool.uv.index]]
# name = "pytorch-cpu"
# url = "https://download.pytorch.org/whl/cpu"
# explicit = true
# ///
"""Underdog Bark 3.0 Clean through tinybark's ONNX Runtime backend (encoder on
the GPU through WebGPU), for stt (see serve.py). `--prepare` only downloads the
model and exports its ONNX graphs, about a minute the first time."""

import sys

from tinybark import ort

bark = ort.load()
if "--prepare" in sys.argv:
    sys.exit()

from serve import serve

serve(bark.transcribe_pcm)
