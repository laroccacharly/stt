"""Shared server loop for stt's local models.

`serve(transcribe)` listens on the Unix socket given as the first argument. Each
connection sends 16 kHz s16le mono PCM and shuts down its write half; the
reply is one JSON line, `{"text": …}` or `{"error": …}`. The server stays up
until killed, so the model is loaded only once.
"""

import json
import os
import socket
import sys


def serve(transcribe):
    path = sys.argv[1]
    with socket.socket(socket.AF_UNIX) as probe:
        if probe.connect_ex(path) == 0:
            sys.exit(f"{path}: already served")
    if os.path.exists(path):
        os.unlink(path)
    server = socket.socket(socket.AF_UNIX)
    # Bind under a temporary name so the socket appears only once listening.
    pending = f"{path}.{os.getpid()}"
    server.bind(pending)
    server.listen()
    os.rename(pending, path)
    print(f"serving on {path}", file=sys.stderr, flush=True)
    while True:
        connection, _ = server.accept()
        with connection:
            pcm = bytearray()
            while chunk := connection.recv(1 << 16):
                pcm += chunk
            try:
                reply = {"text": transcribe(bytes(pcm)).strip() if pcm else ""}
            except Exception as error:
                reply = {"error": f"{type(error).__name__}: {error}"}
            try:
                connection.sendall(json.dumps(reply).encode() + b"\n")
            except OSError as error:
                print(f"client hung up: {error}", file=sys.stderr, flush=True)
