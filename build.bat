@echo off
setlocal
cd /d "%~dp0"

set "out=dist\windows"
if not exist "%out%" mkdir "%out%"

rem The name there goes first, or the move that follows would be onto the same
rem file cargo hard linked it from, which is refused.
if exist "%out%\imgdedupe.exe" del "%out%\imgdedupe.exe"

cargo build --release --workspace
if errorlevel 1 exit /b 1

rem Cargo hard links the binary to a second name under deps. Dropping that name
rem leaves the one in dist as the only one for it. Cargo links it again next
rem time, which measured at under a second for a binary that did not change.
move /y "target\release\imgdedupe.exe" "%out%\imgdedupe.exe" >nul
if errorlevel 1 exit /b 1
if exist "target\release\deps\imgdedupe.exe" del "target\release\deps\imgdedupe.exe"

rem Packed, which is about 60 percent off the linked size. Without upx on the
rem path the build still produces a working binary, just a larger one.
where upx >nul 2>&1
if not errorlevel 1 (
    upx --best --lzma "%out%\imgdedupe.exe"
    if errorlevel 1 exit /b 1
)

rem A copy at the root as well, so the one to run is where it has always been.
copy /y "%out%\imgdedupe.exe" "imgdedupe.exe" >nul
if errorlevel 1 exit /b 1

echo imgdedupe.exe is in %cd%\%out% and %cd%
