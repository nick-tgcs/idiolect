#!/usr/bin/env python3
"""Minimal dictation client for idiolectd.

Connects to the running daemon, opens the mic, waits for you to talk, then
transcribes on the GPU and prints what you said.

Usage:
    python3 scripts/dictate.py [SOCKET_PATH]

If SOCKET_PATH is omitted it defaults to $XDG_RUNTIME_DIR/idiolect.sock.
Press a key, talk, press Enter to stop and transcribe.
"""
import json
import os
import socket
import sys
import time


def main() -> int:
    sock_path = (
        sys.argv[1]
        if len(sys.argv) > 1
        else os.path.join(
            os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "idiolect.sock"
        )
    )

    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        s.connect(sock_path)
    except OSError as exc:
        print(f"could not connect to daemon at {sock_path}: {exc}", file=sys.stderr)
        print("is idiolectd running?", file=sys.stderr)
        return 1
    f = s.makefile("rwb", buffering=0)

    def send(obj):
        f.write((json.dumps(obj) + "\n").encode())

    def recv():
        line = f.readline()
        return json.loads(line.decode()) if line else None

    send(
        {
            "type": "ClientHello",
            "payload": {
                "client_name": "dictate.py",
                "protocol_version": 1,
                "features": ["preedit", "commit"],
            },
        }
    )
    hello = recv()
    if not hello or hello.get("type") != "ServerHello":
        print(f"handshake failed: {hello}", file=sys.stderr)
        return 1

    input("Press Enter, then SPEAK… ")
    send({"type": "StartRecording"})  # mic opens
    input("…recording. Press Enter when done. ")
    send({"type": "StopRecording"})  # stop + transcribe on GPU

    # The daemon replies with a PreeditUpdate carrying the transcript.
    deadline = time.time() + 30
    while time.time() < deadline:
        msg = recv()
        if msg is None:
            break
        if msg.get("type") == "PreeditUpdate":
            text = msg["payload"]["text"]
            print(f"\nYou said: {text}")
            return 0
        if msg.get("type") == "Error":
            print(f"\ndaemon error: {msg['payload']}", file=sys.stderr)
            return 1
    print("no transcript received", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
