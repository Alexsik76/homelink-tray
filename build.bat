@echo off
cd /d %~dp0
if not exist .venv\Scripts\python.exe py -m venv .venv
.venv\Scripts\python -m pip install --upgrade -e ".[build]"
.venv\Scripts\python -m PyInstaller --noconfirm --onefile --noconsole --name HomeLink ^
  --icon src\homelink\assets\app.ico --uac-admin --paths src ^
  --add-data "src\homelink\assets;homelink\assets" ^
  --add-data "src\homelink\locales;homelink\locales" ^
  --add-data "src\homelink\config.example.json;homelink" ^
  run_homelink.pyw
echo.
echo Done: dist\HomeLink.exe
pause
