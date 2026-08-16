@echo off
rem ⚠ cd to THIS folder first. `python -m acecm` resolves the package from the
rem current directory, so launching this .bat from anywhere else - a desktop or
rem Start Menu shortcut, another drive, a terminal in a different folder - left
rem it unable to import acecm. Under pythonw there is no console, so that
rem failure printed nowhere and double-clicking simply did nothing.
cd /d "%~dp0"

where pythonw >nul 2>&1
if %errorlevel%==0 (
  start "" pythonw -m acecm
  exit /b
)

where python >nul 2>&1
if not %errorlevel%==0 (
  echo Python was not found on PATH.
  echo Install Python 3, or launch ACECM.exe instead.
  pause
  exit /b 1
)

rem No pythonw: run in this console so the reason for any failure is visible,
rem and hold the window open if it exits immediately.
python -m acecm
if not %errorlevel%==0 pause
