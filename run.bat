@echo off

echo Creando entorno virtual...
python -m venv venv

echo Activando entorno...
call venv\Scripts\activate

echo Actualizando pip...
python -m pip install --upgrade pip

echo Instalando dependencias...
pip install -r requirements.txt

echo.
echo Listo. Para arrancar:
echo venv\Scripts\activate
echo uvicorn back:app --reload --port 8000

pause
