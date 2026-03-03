#!/bin/bash

set -e

echo "Creando entorno virtual..."
python3 -m venv venv

echo "Activando entorno..."
source venv/bin/activate

echo "Actualizando pip..."
pip install --upgrade pip

echo "Instalando dependencias..."
pip install -r requirements.txt

uvicorn back:app --reload --port 8000