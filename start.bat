@echo off
chcp 65001 > nul
setlocal

:: --- 0. Validate installation path ---
setlocal enabledelayedexpansion
set "LTBOX_CHECK_PATH=%~dp0"
set "LTBOX_CHECK_NOSPACE=!LTBOX_CHECK_PATH: =!"
if not "!LTBOX_CHECK_PATH!"=="!LTBOX_CHECK_NOSPACE!" (
    echo.
    echo [!] Error: The installation path contains spaces.
    echo     Path: !LTBOX_CHECK_PATH!
    echo.
    echo [!] Please move LTBox to a path without spaces.
    echo     Example: D:\LTBox
    echo.
    pause
    endlocal
    goto :eof
)
endlocal

set "SKIP_ADB=0"
set "SKIP_ADB_STATE=OFF"

:: --- 1. Set Python and Main Script Paths ---
set "PYTHON_EXE=%~dp0bin\python3\python.exe"
set "MAIN_PY=%~dp0bin\run.py"

if not exist "%PYTHON_EXE%" (
    echo [!] Python not found at: %PYTHON_EXE%
    echo [!] Please re-download or re-extract the LTBox release package.
    pause
    goto :eof
)
if not exist "%MAIN_PY%" (
    echo [!] Main script not found at: %MAIN_PY%
    pause
    goto :eof
)

:: --- 3. Check Admin Privileges ---
net session >nul 2>&1
if %errorlevel% == 0 goto :run

:: --- 3a. Re-launch elevated in Windows Terminal ---
set "LTBOX_DIR=%~dp0."
set "LTBOX_SELF=%~f0"
where wt.exe >nul 2>&1
if %errorlevel% == 0 (
    powershell -Command "Start-Process wt -Verb RunAs -ArgumentList ('cmd /k cd /d ' + [char]34 + $env:LTBOX_DIR + [char]34 + ' && ' + [char]34 + $env:LTBOX_SELF + [char]34)"
) else (
    powershell -Command "Start-Process cmd -Verb RunAs -ArgumentList ('/k cd /d ' + [char]34 + $env:LTBOX_DIR + [char]34 + ' && ' + [char]34 + $env:LTBOX_SELF + [char]34)"
)
goto :eof

:: --- 4. Run Main Python Script ---
:run
"%PYTHON_EXE%" "%MAIN_PY%"
goto :eof

:: --- 6. Exit ---
:cleanup
endlocal
echo.
echo Exiting LTBox.
goto :eof
