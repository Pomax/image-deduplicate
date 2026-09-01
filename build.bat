@echo off
setlocal
cd /d "%~dp0"

rem The name here goes first, or the move that follows would be onto the same
rem file cargo hard linked it from, which is refused.
for %%b in (imgindex imgdedupe) do (
    if exist "%%b.exe" del "%%b.exe"
)

cargo build --release --workspace
if errorlevel 1 exit /b 1

rem Cargo hard links each binary to a second name under deps. Dropping that name
rem leaves the one here as the only one for it. Cargo links it again next time,
rem which measured at under a second for a binary that did not change.
for %%b in (imgindex imgdedupe) do (
    move /y "target\release\%%b.exe" "%%b.exe" >nul
    if errorlevel 1 exit /b 1
    if exist "target\release\deps\%%b.exe" del "target\release\deps\%%b.exe"
)

echo imgindex.exe and imgdedupe.exe are in %cd%
