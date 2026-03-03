# Localify

Backend desarrollado con FastAPI para descargar música desde Spotify y YouTube.

---

# 📦 Requisitos

* Python 3.10 o superior
* Git

Comprobar versión instalada:

```
python --version
```

Si en Linux no funciona:

```
python3 --version
```

---

# 📥 1️⃣ Clonar el repositorio

```
git clone https://github.com/TU_USUARIO/TU_REPO.git
cd TU_REPO
```

---

# 🔐 2️⃣ Configuración obligatoria

## 🎵 1. Credenciales de Spotify

Crea un archivo llamado:

```
spotify_keys.txt
```

En la raíz del proyecto con el siguiente formato:

```
CLIENT_ID=tu_client_id
CLIENT_SECRET=tu_client_secret
```

Puedes obtener las claves en:
https://developer.spotify.com/dashboard

---

## 🍪 2. Archivo cookies.txt (YouTube)

Necesitas un archivo `cookies.txt` con una sesión iniciada en YouTube.

Puedes generarlo usando esta extensión de Chrome:

https://chromewebstore.google.com/detail/get-cookiestxt-locally/cclelndahbckbenkjhflpdbgdldlbecc

Pasos:

1. Inicia sesión en YouTube.
2. Instala la extensión.
3. Exporta las cookies.
4. Guarda el archivo como `cookies.txt` en la raíz del proyecto.

---

# 🚀 3️⃣ Ejecutar la aplicación

El proyecto incluye:

* `run.sh` (Linux/macOS)
* `run.bat` (Windows)

Estos scripts:

1. Crean el entorno virtual `venv` si no existe.
2. Activan el entorno.
3. Instalan las dependencias desde `requirements.txt`.
4. Lanzan el servidor:

```
uvicorn back:app --reload --port 8000
```

---

## 🐧 Linux / macOS

Dar permisos (solo la primera vez):

```
chmod +x run.sh
```

Ejecutar:

```
./run.sh
```

---

## 🪟 Windows

Ejecutar:

```
run.bat
```

---

# 🌐 Acceder a la app

Aplicación:
http://127.0.0.1:8000

Documentación interactiva:
http://127.0.0.1:8000/docs


---

Fin.
