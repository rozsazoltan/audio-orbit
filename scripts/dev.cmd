@echo off
setlocal
cd /d "%~dp0.."
echo Audio Orbit dev launcher
echo Using project-local dev runner; cargo-watch is not required.
cargo run --bin audio-orbit-dev --
