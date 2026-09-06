@echo off
setlocal
rem The repository, which is where cargo has to be run from and where the built
rem executable belongs. This script lives in a directory of its own under it.
cd /d "%~dp0.."

rem With --test the binary understands --log and writes a run log. Without it
rem none of that code is compiled in: the flag, the file, and every line the
rem program would have written are all behind the feature.
set "features="
set "note="
if not "%~1"=="" (
    if /i "%~1"=="--test" (
        set "features=--features imgdedupe/logging"
        set "note= --log build"
    ) else (
        echo the only argument is --test
        exit /b 1
    )
)

cargo build --release --workspace %features%
if errorlevel 1 exit /b 1

rem Whether the program is running is asked of the task list rather than of the
rem file, so the one that is running is never deleted or written over. One that
rem is merely sitting there is replaced, which is what a build is for; one that
rem is open is left alone and the build goes beside it under its own name.
set "out=imgdedupe.exe"
tasklist /fi "imagename eq imgdedupe.exe" /nh 2>nul | find /i "imgdedupe.exe" >nul
if errorlevel 1 (
    if exist "imgdedupe.exe" del "imgdedupe.exe"
) else (
    set "out=imgdedupe-new.exe"
)
if exist "%out%" del "%out%"
if exist "%out%" (
    echo %out% is in use and this build has nowhere to go
    exit /b 1
)

rem Cargo hard links the binary to a second name under deps. Dropping that name
rem leaves the one at the root as the only one for it. Cargo links it again next
rem time, which measured at under a second for a binary that did not change.
move /y "target\release\imgdedupe.exe" "%out%" >nul
if errorlevel 1 exit /b 1
if exist "target\release\deps\imgdedupe.exe" del "target\release\deps\imgdedupe.exe"

rem Packed, which is about 60 percent off the linked size. Without upx on the
rem path the build still produces a working binary, just a larger one.
where upx >nul 2>&1
if not errorlevel 1 (
    upx --best --lzma "%out%"
    if errorlevel 1 exit /b 1
)

if "%out%"=="imgdedupe.exe" (
    echo imgdedupe.exe%note% is in %cd%
) else (
    echo imgdedupe.exe is running, so this build%note% is %out% in %cd%
    echo rename it over that one once it is closed
)
