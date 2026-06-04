# Whisper Fixture

Primary model:

- File: `ggml-tiny.en.bin`
- Pinned URL: `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin`
- SHA-256: `921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f`
- License: MIT
- Expected transcript for `tests/fixtures/audio/restart_traffic_16khz_mono.wav`: `restart traffic`

Fallback if `ggml-tiny.en.bin` misses both words on the speech fixture:

- File: `ggml-base.en.bin`
- Pinned URL: `https://huggingface.co/ggerganov/whisper.cpp/resolve/refs%2Fpr%2F8/ggml-base.en.bin`
- SHA-256: `a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002`
- License: MIT

The real-media full-stack E2E suite uses `ggml-tiny.en.bin` with the deterministic restart-traffic audio fixture; required CI must not depend on a live microphone.
