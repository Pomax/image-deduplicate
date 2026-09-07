@echo off
setlocal
rem The repository, which is where cargo has to be run from. This script lives
rem in a directory of its own under it.
cd /d "%~dp0.."

rem Anything given on the command line is passed on to cargo, so one test can be
rem run by name: scripts\test.bat the_name_of_the_test. A named run is what was
rem asked for and nothing else is added to it.
if not "%~1"=="" (
    cargo test --workspace %*
    exit /b %errorlevel%
)

rem `local\` holds the checks that only mean something against a real folder of
rem photographs. It is not in the repository, so a checkout without it runs the
rem suite exactly as CI does, and a machine that has it runs those as well.
if not exist "local\" (
    cargo test --workspace
    exit /b %errorlevel%
)

echo local\ is here, so the checks against a real folder are included
cargo test --workspace --features imgdedupe/local
if errorlevel 1 exit /b 1

rem They are all marked to be asked for by name, because they take minutes and
rem touch a real index. `local::` is the module they are all in.
cargo test --workspace --features imgdedupe/local local:: -- --ignored --nocapture
