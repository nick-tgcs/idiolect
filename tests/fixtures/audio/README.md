# Audio fixtures

## `restart_traffic_16khz_mono.wav`

Short spoken "restart traffic" clip (16 kHz, mono, PCM s16le). Expected Whisper
transcript: `restart traffic`. Used by the deterministic full-stack streaming
tests.

## `librispeech_8555_292519_16khz_mono.wav`

A **real** ~2 min 11 s recording of continuous human speech — the 16 utterances
of LibriSpeech `test-clean` speaker `8555`, chapter `292519`, concatenated in
order into one 16 kHz mono PCM s16le WAV. This is the take the streaming finalize
re-decode is exercised against with the real Whisper adapter (a genuinely long
take, several 30 s Whisper windows wide), so the guarantee is proven on real
audio rather than a synthesised or repeated clip.

- Duration: 130.995 s
- SHA-256: `c39c685e7e860a6bc872e336c9a02b80a4f8b003d6b5adc92006f1159a928689`
- Reference transcript: `librispeech_8555_292519.txt` (LibriSpeech's own aligned
  ground truth for these utterances, upper-cased as distributed).

### Provenance & licence

Derived from **LibriSpeech** (`test-clean` subset), which is itself read from
public-domain LibriVox audiobooks.

- Source: <https://www.openslr.org/12> (`test-clean.tar.gz`)
- Corpus: V. Panayotov, G. Chen, D. Povey, S. Khudanpur, "Librispeech: an ASR
  corpus based on public domain audio books", ICASSP 2015.
- Licence: **CC BY 4.0** (<https://creativecommons.org/licenses/by/4.0/>).

The concatenation is a lossless join of the corpus's own FLAC utterances (already
16 kHz mono); no other modification was made.
