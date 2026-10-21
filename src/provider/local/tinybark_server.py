# /// script
# requires-python = "==3.12.*"
# dependencies = ["tinybark"]
#
# [tool.uv.sources]
# tinybark = { path = "$STT_DIR/../tinybark", editable = true }
# ///
"""Underdog Bark 3.0 Clean on the GPU through tinybark (tinygrad), for stt (see
serve.py). `--prepare` only downloads and converts the model."""

import sys

import tinybark

# Warm up every padded length up to 30 s before listening, about 70 s, so each
# recording is fast from the first. Off, the first recording of each 2.56 s
# length takes about 5 s more.
WARM_UP = False

if "--prepare" in sys.argv:
    tinybark.Bark()
    sys.exit()

bark = tinybark.load() if WARM_UP else tinybark.Bark()

from serve import serve

serve(bark.transcribe_pcm)
