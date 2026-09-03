@echo off
setlocal
cd /d "%~dp0"

rem Runs the whole suite. Anything given on the command line is passed on to
rem cargo, so one test can be run by name: test.bat the_name_of_the_test
cargo test --workspace %*
