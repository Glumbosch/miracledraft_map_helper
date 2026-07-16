@echo off
cd /d "%~dp0"
cargo run --release
if errorlevel 1 pause
