@echo off
rem Opens Cronista without a console window. Double-click it, or pin a shortcut to it.
start "" "%~dp0.venv\Scripts\pythonw.exe" "%~dp0app.py"
