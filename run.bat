@echo off
where pythonw >nul 2>&1
if %errorlevel%==0 (
  start "" pythonw -m acecm
  exit /b
)
python -m acecm
