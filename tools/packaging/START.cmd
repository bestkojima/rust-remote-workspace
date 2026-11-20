@echo off
setlocal DisableDelayedExpansion
set "PATH=%~dp0;%PATH%"
"%~dp0rust-remote.exe"
exit /b %ERRORLEVEL%
