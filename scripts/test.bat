@echo off
setlocal
rem The repository, which is where cargo has to be run from. This script lives
rem in a directory of its own under it.
cd /d "%~dp0.."

rem Without arguments this is the suite, the way CI runs it: no extra features,
rem and nothing that reads anybody's real folder of photographs.
rem
rem With --all it is that plus the checks in local\, which are compiled by the
rem `local` feature and run against the folder the application is set to. They
rem take minutes and they write to a real index, so they are asked for.
rem
rem Anything else given on the command line is passed on to cargo, so one test
rem can be run by name: scripts\test.bat the_name_of_the_test.
if /i "%~1"=="--all" goto all

cargo test --workspace %*
exit /b %errorlevel%

:all
if not exist "local\" (
    echo there is no local\ here, so there is nothing --all adds
    exit /b 1
)

rem The suite, without them: local:: is the module they are all in, and
rem everything else runs the way it always does, in parallel.
cargo test --workspace --features imgdedupe/local -- --skip local::
if errorlevel 1 exit /b 1

rem Then those, one at a time. Not because they are optional: the feature is what
rem asks for them, and asking for it is asking for them to run. It is that there
rem is one real index and they all open it, so run together they read each
rem other's half-written work and three of the seven fail on it.
cargo test --workspace --features imgdedupe/local local:: -- --nocapture --test-threads=1
