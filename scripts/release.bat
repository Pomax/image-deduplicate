@echo off
setlocal
rem The repository, which is where cargo and git have to be run from. This
rem script lives in a directory of its own under it.
cd /d "%~dp0.."

set "part=%~1"
if /i "%part%"=="major" goto part_given
if /i "%part%"=="minor" goto part_given
if /i "%part%"=="patch" goto part_given
echo usage: scripts\release.bat major^|minor^|patch
exit /b 1
:part_given

rem Releases are cut from main and nowhere else.
set "branch="
for /f "delims=" %%B in ('git rev-parse --abbrev-ref HEAD') do set "branch=%%B"
if not "%branch%"=="main" (
    echo not on main
    exit /b 1
)

rem The version the whole workspace shares. It is the one line in the file that
rem starts with `version = `: the dependency versions further down all sit
rem inside a table on the line, so none of them starts one.
set "current="
for /f "usebackq tokens=2 delims== " %%V in (`findstr /b /c:"version = " Cargo.toml`) do (
    if not defined current set "current=%%~V"
)
if not defined current (
    echo Cargo.toml has no version in its [workspace.package] section
    exit /b 1
)

for /f "tokens=1,2,3 delims=." %%A in ("%current%") do (
    set "major=%%A"
    set "minor=%%B"
    set "patch=%%C"
)

if /i "%part%"=="major" (
    set /a major = major + 1
    set "minor=0"
    set "patch=0"
)
if /i "%part%"=="minor" (
    set /a minor = minor + 1
    set "patch=0"
)
if /i "%part%"=="patch" set /a patch = patch + 1

set "next=%major%.%minor%.%patch%"
echo %current% to %next%

rem Every line back out, with the version line replaced. `findstr /n` numbers
rem them so that the blank ones survive the loop, and the number comes off
rem again on the way out.
if exist "Cargo.toml.next" del "Cargo.toml.next"
> "Cargo.toml.next" (
    for /f "usebackq delims=" %%L in (`findstr /n "^" Cargo.toml`) do (
        set "line=%%L"
        setlocal enabledelayedexpansion
        set "line=!line:*:=!"
        if "!line:~0,10!"=="version = " (
            if defined replaced (
                echo(!line!
                endlocal
            ) else (
                echo version = "%next%"
                endlocal
                set "replaced=1"
            )
        ) else (
            echo(!line!
            endlocal
        )
    )
)
move /y "Cargo.toml.next" "Cargo.toml" >nul
if errorlevel 1 exit /b 1

rem The lock file carries the version of every crate in the workspace, so it has
rem to say the same thing the manifest now says. Only the workspace's own
rem entries are touched: the dependencies stay on what they were locked to.
cargo update --workspace
if errorlevel 1 exit /b 1

git add Cargo.toml Cargo.lock
if errorlevel 1 exit /b 1
git commit -m "bump to v%next%"
if errorlevel 1 exit /b 1
