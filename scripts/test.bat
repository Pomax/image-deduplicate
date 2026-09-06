@echo off
setlocal
rem The repository, which is where cargo has to be run from. This script lives
rem in a directory of its own under it.
cd /d "%~dp0.."

rem Runs the whole suite. Anything given on the command line is passed on to
rem cargo, so one test can be run by name: scripts\test.bat the_name_of_the_test
cargo test --workspace %*
