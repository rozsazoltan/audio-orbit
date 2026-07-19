# Third-Party Notices

Audio Orbit remains licensed under AGPL-3.0-or-later. This file records direct third-party DJ-engine components. Full transitive dependency inventory must still be generated and reviewed from `Cargo.lock` before release.

## ebur128

- Package: `ebur128 0.1.10`
- Source: https://github.com/sdroege/ebur128
- License: MIT
- Copyright: 2011 Jan Kokemüller; 2020 Sebastian Dröge

## shine-rs

- Package: `shine-rs 0.1.3`
- Purpose: MP3 encoding
- License: LGPL-2.0

## Models and sidecars

No ML checkpoint, source-separation model, FFmpeg binary, Mixxx code, Liquidsoap binary, Essentia code/model, Beat This checkpoint, Demucs checkpoint, Open-Unmix checkpoint, Rubber Band code, aubio code, or libKeyFinder code is bundled by this change.

## Optional professional DJ tools

Audio Orbit can invoke the following separately installed command-line tools at runtime:

- Essentia Music Extractor: rhythm, beat-grid, confidence, and musical-key analysis
- Rubber Band R3: pitch-preserving time stretching
- Demucs: drums, bass, accompaniment, and vocal stem separation

These tools, their source code, binaries, models, Python environments, and transitive dependencies are not distributed in the Audio Orbit archive. Users install and license them separately under their upstream terms. Audio Orbit communicates with them through temporary WAV/JSON files and command-line process execution. Each integration has an independent built-in fallback.
