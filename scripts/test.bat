@echo off
setlocal
rem The repository, which is where cargo has to be run from. This script lives
rem in a directory of its own under it.
cd /d "%~dp0.."

rem The whole suite, the way CI runs it. Anything given on the command line is
rem passed on to cargo, so one test can be run by name:
rem scripts\test.bat the_name_of_the_test.
rem
rem The source is formatted before anything is compiled, so what the tests run
rem against is what the formatter would leave behind. A formatter that will not
rem run is source nobody can trust, so that stops the run here.
cargo fmt --all
if errorlevel 1 exit /b 1

cargo test --workspace %*
exit /b %errorlevel%
