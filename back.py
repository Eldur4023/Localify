#!/usr/bin/env python3
"""
Backend FastAPI para descargar música de Spotify/YouTube.
Instalar: pip install fastapi uvicorn spotipy yt-dlp python-multipart mutagen requests
Ejecutar: uvicorn back:app --reload --port 8000
"""

import os
import re
import sys
import uuid
import shutil
import asyncio
import threading
import requests as http_requests
from pathlib import Path
from typing import Optional

from fastapi import FastAPI, HTTPException
from fastapi.staticfiles import StaticFiles
from fastapi.responses import FileResponse, StreamingResponse
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel

# ─── CREDENCIALES SPOTIFY ─────────────────────────────────────────────────────
CLIENT_ID     = "xxxxx"
CLIENT_SECRET = "xxxxx"
# ──────────────────────────────────────────────────────────────────────────────


COOKIES_FILE  = os.getenv("COOKIES_FILE", "cookies.txt")   # ruta al cookies.txt
DOWNLOAD_DIR  = Path("descargas")
DOWNLOAD_DIR.mkdir(exist_ok=True)

app = FastAPI(title="Music Downloader")

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)

# jobs: { job_id: { status, total, done, failed, log, zip_path } }
jobs: dict = {}



# ─── MODELOS ──────────────────────────────────────────────────────────────────

class PlaylistRequest(BaseModel):
    url: str
    formato: str = "m4a"
    workers: int = 3

class TrackRequest(BaseModel):
    url: str            # URL de canción de Spotify
    formato: str = "m4a"


# ─── SPOTIFY ──────────────────────────────────────────────────────────────────



def cargar_claves_spotify(path: str = "spotify_keys.txt"):
    if not Path(path).exists():
        raise RuntimeError(
            "No se encontró spotify_keys.txt. "
            "Crea el archivo con CLIENT_ID y CLIENT_SECRET."
        )

    claves = {}
    with open(path, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            if "=" in line:
                k, v = line.split("=", 1)
                claves[k.strip()] = v.strip()

    cid = claves.get("CLIENT_ID")
    cs  = claves.get("CLIENT_SECRET")

    if not cid or not cs:
        raise RuntimeError(
            "spotify_keys.txt inválido. Debe contener CLIENT_ID y CLIENT_SECRET."
        )

    return cid, cs
CLIENT_ID, CLIENT_SECRET = cargar_claves_spotify()

def conectar_spotify():
    import spotipy
    from spotipy.oauth2 import SpotifyClientCredentials
    cid = CLIENT_ID or os.getenv("SPOTIPY_CLIENT_ID", "")
    cs  = CLIENT_SECRET or os.getenv("SPOTIPY_CLIENT_SECRET", "")
    if not cid or not cs:
        raise RuntimeError("Faltan credenciales de Spotify")
    return spotipy.Spotify(auth_manager=SpotifyClientCredentials(client_id=cid, client_secret=cs))


def obtener_info_cancion(url: str) -> dict:
    """
    Dado un enlace de canción de Spotify, devuelve un dict con:
      nombre, artistas, album, cover_url
    """
    sp = conectar_spotify()
    match = re.search(r"track[:/]([A-Za-z0-9]+)", url)
    if not match:
        raise ValueError("URL de canción de Spotify inválida. Debe ser del tipo: https://open.spotify.com/track/...")
    tid = match.group(1)
    track = sp.track(tid)

    nombre   = track["name"]
    artistas = ", ".join(a["name"] for a in track["artists"])
    album    = track["album"]["name"]
    images   = track["album"].get("images", [])
    cover_url = images[0]["url"] if images else None

    return {
        "nombre":    nombre,
        "artistas":  artistas,
        "album":     album,
        "cover_url": cover_url,
    }


def obtener_canciones_playlist(url: str):
    sp = conectar_spotify()
    match = re.search(r"playlist[:/]([A-Za-z0-9]+)", url)
    if not match:
        raise ValueError("URL de playlist inválida")
    pid = match.group(1)

    info = sp.playlist(pid, fields="name,tracks.total")
    nombre = info["name"]
    canciones = []
    offset = 0
    while True:
        res = sp.playlist_items(pid, offset=offset, limit=100,
                                fields="items(track(name,artists(name),album(name,images))),next,total")
        items = res.get("items", [])
        total = res.get("total", 0)
        for item in items:
            t = item.get("track")
            if not t:
                continue
            images    = t.get("album", {}).get("images", [])
            cover_url = images[0]["url"] if images else None
            canciones.append({
                "nombre":    t.get("name", "Desconocido"),
                "artistas":  ", ".join(a["name"] for a in t.get("artists", [])),
                "album":     t.get("album", {}).get("name", ""),
                "cover_url": cover_url,
            })
        offset += len(items)
        if offset >= total or not items:
            break
    return nombre, canciones


# ─── METADATOS ────────────────────────────────────────────────────────────────

def _descargar_portada(url: str) -> Optional[bytes]:
    """Descarga la imagen de portada y devuelve los bytes, o None si falla."""
    if not url:
        return None
    try:
        r = http_requests.get(url, timeout=10)
        r.raise_for_status()
        return r.content
    except Exception:
        return None


def escribir_metadatos(archivo: Path, nombre: str, artistas: str, album: str, cover_bytes: Optional[bytes]):
    """
    Escribe título, artista, álbum y portada al archivo de audio usando mutagen.
    Soporta MP3, M4A/MP4, FLAC, OGG/Vorbis, OPUS.
    """
    try:
        from mutagen import File as MutagenFile
        from mutagen.id3 import ID3NoHeaderError
    except ImportError:
        return  # mutagen no instalado, omitir

    suffix = archivo.suffix.lower()

    try:
        if suffix == ".mp3":
            _escribir_id3(archivo, nombre, artistas, album, cover_bytes)
        elif suffix in (".m4a", ".mp4", ".aac"):
            _escribir_mp4(archivo, nombre, artistas, album, cover_bytes)
        elif suffix == ".flac":
            _escribir_flac(archivo, nombre, artistas, album, cover_bytes)
        elif suffix in (".ogg", ".oga"):
            _escribir_ogg(archivo, nombre, artistas, album, cover_bytes)
        elif suffix == ".opus":
            _escribir_opus(archivo, nombre, artistas, album, cover_bytes)
        else:
            # Intento genérico con mutagen
            audio = MutagenFile(str(archivo), easy=True)
            if audio is not None:
                audio["title"]  = nombre
                audio["artist"] = artistas
                audio["album"]  = album
                audio.save()
    except Exception:
        pass  # No bloquear la descarga si los metadatos fallan


def _escribir_id3(archivo: Path, nombre, artistas, album, cover_bytes):
    from mutagen.id3 import ID3, TIT2, TPE1, TALB, APIC, ID3NoHeaderError
    try:
        tags = ID3(str(archivo))
    except ID3NoHeaderError:
        tags = ID3()

    tags["TIT2"] = TIT2(encoding=3, text=nombre)
    tags["TPE1"] = TPE1(encoding=3, text=artistas)
    tags["TALB"] = TALB(encoding=3, text=album)
    if cover_bytes:
        tags["APIC"] = APIC(
            encoding=3,
            mime="image/jpeg",
            type=3,   # Cover (front)
            desc="Cover",
            data=cover_bytes,
        )
    tags.save(str(archivo))


def _escribir_mp4(archivo: Path, nombre, artistas, album, cover_bytes):
    from mutagen.mp4 import MP4, MP4Cover
    audio = MP4(str(archivo))
    audio["\xa9nam"] = [nombre]
    audio["\xa9ART"] = [artistas]
    audio["\xa9alb"] = [album]
    if cover_bytes:
        audio["covr"] = [MP4Cover(cover_bytes, imageformat=MP4Cover.FORMAT_JPEG)]
    audio.save()


def _escribir_flac(archivo: Path, nombre, artistas, album, cover_bytes):
    from mutagen.flac import FLAC, Picture
    audio = FLAC(str(archivo))
    audio["title"]  = [nombre]
    audio["artist"] = [artistas]
    audio["album"]  = [album]
    if cover_bytes:
        pic = Picture()
        pic.type = 3
        pic.mime = "image/jpeg"
        pic.desc = "Cover"
        pic.data = cover_bytes
        audio.clear_pictures()
        audio.add_picture(pic)
    audio.save()


def _escribir_ogg(archivo: Path, nombre, artistas, album, cover_bytes):
    from mutagen.oggvorbis import OggVorbis
    import base64
    from mutagen.flac import Picture
    audio = OggVorbis(str(archivo))
    audio["title"]  = [nombre]
    audio["artist"] = [artistas]
    audio["album"]  = [album]
    if cover_bytes:
        pic = Picture()
        pic.type = 3
        pic.mime = "image/jpeg"
        pic.desc = "Cover"
        pic.data = cover_bytes
        audio["metadata_block_picture"] = [
            base64.b64encode(pic.write()).decode("ascii")
        ]
    audio.save()


def _escribir_opus(archivo: Path, nombre, artistas, album, cover_bytes):
    from mutagen.oggopus import OggOpus
    import base64
    from mutagen.flac import Picture
    audio = OggOpus(str(archivo))
    audio["title"]  = [nombre]
    audio["artist"] = [artistas]
    audio["album"]  = [album]
    if cover_bytes:
        pic = Picture()
        pic.type = 3
        pic.mime = "image/jpeg"
        pic.desc = "Cover"
        pic.data = cover_bytes
        audio["metadata_block_picture"] = [
            base64.b64encode(pic.write()).decode("ascii")
        ]
    audio.save()


# ─── YOUTUBE ──────────────────────────────────────────────────────────────────

def descargar_yt(query: str, carpeta: Path, formato: str,
                 meta: Optional[dict] = None) -> tuple[bool, str]:
    """
    Descarga audio de YouTube y, si se proporcionan metadatos Spotify (meta),
    escribe título, artista, álbum y portada al archivo resultante.

    meta = { nombre, artistas, album, cover_url }  (todos opcionales)
    """
    import yt_dlp

    ydl_opts = {
        "format": "bestaudio/best",
        "outtmpl": str(carpeta / "%(title)s.%(ext)s"),
        "quiet": True,
        "no_warnings": True,
        "nooverwrites": True,
        # tv_embedded no requiere PO Token — funciona desde servidores sin navegador
        "extractor_args": {
            "youtube": {
                "player_client": ["tv_embedded", "ios"],
            }
        },
        "postprocessors": [{
            "key": "FFmpegExtractAudio",
            "preferredcodec": formato,
            "preferredquality": "192",
        }],
    }

    # ── Autenticación ─────────────────────────────────────────────────────────
    cookies = os.getenv("COOKIES_FILE", COOKIES_FILE)
    if cookies and Path(cookies).exists():
        ydl_opts["cookiefile"] = str(Path(cookies).resolve())
        print(f"[yt] Usando cookies: {ydl_opts['cookiefile']}")
    else:
        print(f"[yt] AVISO: cookies.txt no encontrado en '{cookies}' — YouTube puede bloquear la descarga")

    if re.match(r"https?://(www\.)?(youtube\.com|youtu\.be)/", query):
        search = query
    else:
        search = f"ytsearch1:{query}"

    try:
        with yt_dlp.YoutubeDL(ydl_opts) as ydl:
            info = ydl.extract_info(search, download=True)
            if info and "entries" in info and info["entries"]:
                titulo = info["entries"][0].get("title", query)
            elif info:
                titulo = info.get("title", query)
            else:
                titulo = query
    except Exception as e:
        return False, str(e)

    # ── Escribir metadatos si se pasaron ──────────────────────────────────────
    if meta:
        EXTS_AUDIO = [formato, "m4a", "mp4", "mp3", "opus", "webm", "flac", "wav", "ogg"]
        archivo = None
        for ext in EXTS_AUDIO:
            encontrados = [f for f in carpeta.iterdir() if f.is_file() and f.suffix.lower() == f".{ext}"]
            if encontrados:
                archivo = encontrados[0]
                break
        if archivo is None:
            todos = [f for f in carpeta.iterdir() if f.is_file()]
            archivo = todos[0] if todos else None

        if archivo:
            cover_bytes = _descargar_portada(meta.get("cover_url"))
            escribir_metadatos(
                archivo,
                nombre   = meta.get("nombre", titulo),
                artistas = meta.get("artistas", ""),
                album    = meta.get("album", ""),
                cover_bytes = cover_bytes,
            )

    return True, titulo


# ─── JOBS (descargas en background) ───────────────────────────────────────────

def run_playlist_job(job_id: str, canciones: list, carpeta: Path, formato: str, workers: int):
    import concurrent.futures
    job = jobs[job_id]
    job["total"] = len(canciones)
    lock = threading.Lock()

    def tarea(cancion):
        query = f"{cancion['nombre']} {cancion['artistas']}"
        label = f"{cancion['nombre']} — {cancion['artistas']}"
        meta  = {
            "nombre":    cancion["nombre"],
            "artistas":  cancion["artistas"],
            "album":     cancion.get("album", ""),
            "cover_url": cancion.get("cover_url"),
        }
        ok, info = descargar_yt(query, carpeta, formato, meta=meta)
        with lock:
            if ok:
                job["done"] += 1
                job["log"].append({"ok": True,  "label": label})
            else:
                job["failed"] += 1
                job["log"].append({"ok": False, "label": label, "error": info})

    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as ex:
        list(ex.map(tarea, canciones))

    # empaquetar zip
    zip_path = Path("descargas") / f"{job_id}.zip"
    shutil.make_archive(str(zip_path.with_suffix("")), "zip", str(carpeta))
    shutil.rmtree(carpeta, ignore_errors=True)
    job["zip_path"] = str(zip_path)
    job["status"] = "done"


# ─── ENDPOINTS ────────────────────────────────────────────────────────────────

@app.post("/api/playlist")
async def iniciar_playlist(req: PlaylistRequest):
    """Inicia la descarga de una playlist. Devuelve un job_id."""
    try:
        nombre, canciones = obtener_canciones_playlist(req.url)
    except Exception as e:
        raise HTTPException(400, str(e))

    job_id = str(uuid.uuid4())
    carpeta = DOWNLOAD_DIR / job_id
    carpeta.mkdir(parents=True, exist_ok=True)

    jobs[job_id] = {
        "status": "running",
        "nombre": nombre,
        "total": len(canciones),
        "done": 0,
        "failed": 0,
        "log": [],
        "zip_path": None,
    }

    workers = max(1, min(req.workers, 8))
    t = threading.Thread(
        target=run_playlist_job,
        args=(job_id, canciones, carpeta, req.formato, workers),
        daemon=True,
    )
    t.start()

    return {"job_id": job_id, "nombre": nombre, "total": len(canciones)}


@app.get("/api/progreso/{job_id}")
async def progreso(job_id: str):
    """Devuelve el estado actual del job."""
    job = jobs.get(job_id)
    if not job:
        raise HTTPException(404, "Job no encontrado")
    return {
        "status":  job["status"],
        "nombre":  job["nombre"],
        "total":   job["total"],
        "done":    job["done"],
        "failed":  job["failed"],
        "log":     job["log"],
    }


@app.get("/api/descargar/{job_id}")
async def descargar_zip(job_id: str):
    """Descarga el ZIP cuando el job ha terminado."""
    job = jobs.get(job_id)
    if not job:
        raise HTTPException(404, "Job no encontrado")
    if job["status"] != "done":
        raise HTTPException(400, "La descarga aún no ha terminado")
    zip_path = Path(job["zip_path"])
    if not zip_path.exists():
        raise HTTPException(404, "Archivo no encontrado")

    nombre_safe = re.sub(r'[^\w\s-]', '', job["nombre"]).strip()
    return FileResponse(
        path=str(zip_path),
        media_type="application/zip",
        filename=f"{nombre_safe}.zip",
    )


@app.post("/api/cancion")
async def descargar_cancion_individual(req: TrackRequest):
    """Resuelve la URL de Spotify, busca en YouTube, embebe metadatos y devuelve el archivo."""
    try:
        info_sp = obtener_info_cancion(req.url)
    except Exception as e:
        raise HTTPException(400, str(e))

    nombre   = info_sp["nombre"]
    artistas = info_sp["artistas"]
    query    = f"{nombre} {artistas}"

    job_id = str(uuid.uuid4())
    carpeta = DOWNLOAD_DIR / job_id
    carpeta.mkdir(parents=True, exist_ok=True)

    loop = asyncio.get_event_loop()
    ok, info = await loop.run_in_executor(
        None, descargar_yt, query, carpeta, req.formato, info_sp
    )

    if not ok:
        shutil.rmtree(carpeta, ignore_errors=True)
        raise HTTPException(500, f"Error al descargar: {info}")

    EXTS_AUDIO = [req.formato, "m4a", "mp4", "mp3", "opus", "webm", "flac", "wav", "ogg"]
    archivos = []
    for ext in EXTS_AUDIO:
        encontrados = [f for f in carpeta.iterdir() if f.is_file() and f.suffix.lower() == f".{ext}"]
        if encontrados:
            archivos = encontrados
            break
    if not archivos:
        archivos = [f for f in carpeta.iterdir() if f.is_file()]

    if not archivos:
        shutil.rmtree(carpeta, ignore_errors=True)
        raise HTTPException(500, "No se encontró el archivo descargado")

    archivo = archivos[0]
    nombre_archivo = archivo.name

    def iter_y_limpiar():
        try:
            with open(archivo, "rb") as f:
                while chunk := f.read(1024 * 64):
                    yield chunk
        finally:
            shutil.rmtree(carpeta, ignore_errors=True)

    media_types = {
        "mp3": "audio/mpeg",
        "m4a": "audio/mp4",
        "flac": "audio/flac",
        "wav": "audio/wav",
        "ogg": "audio/ogg",
    }
    media_type = media_types.get(req.formato, "application/octet-stream")

    return StreamingResponse(
        iter_y_limpiar(),
        media_type=media_type,
        headers={"Content-Disposition": f'attachment; filename="{nombre_archivo}"'},
    )


@app.delete("/api/job/{job_id}")
async def limpiar_job(job_id: str):
    """Limpia los archivos de un job terminado."""
    job = jobs.pop(job_id, None)
    if job and job.get("zip_path"):
        Path(job["zip_path"]).unlink(missing_ok=True)
    return {"ok": True}


# Servir el frontend estático
app.mount("/", StaticFiles(directory="static", html=True), name="static")
