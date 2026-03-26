#!/usr/bin/env python3
"""
Backend FastAPI para descargar música de Spotify/YouTube.
Instalar: pip install fastapi uvicorn spotipy yt-dlp python-multipart mutagen requests
Ejecutar: uvicorn back:app --reload --port 8000
"""

import os
import re
import sys
import json
import uuid
import base64
import hashlib
import secrets
import shutil
import asyncio
import threading
import urllib.parse
import requests as http_requests
from pathlib import Path
from typing import Optional

from fastapi import FastAPI, HTTPException
from fastapi.staticfiles import StaticFiles
from fastapi.responses import FileResponse, StreamingResponse, RedirectResponse, HTMLResponse
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel

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

# ─── OAUTH SPOTIFY ────────────────────────────────────────────────────────────
# access_token, refresh_token, expiry (epoch), pkce_verifier
_spotify_oauth: dict = {}

SPOTIFY_REDIRECT_URI = os.getenv("SPOTIFY_REDIRECT_URI", "http://localhost:8000/api/spotify/callback")
SPOTIFY_SCOPES       = "playlist-read-private playlist-read-collaborative"


def _spotify_client_id() -> Optional[str]:
    try:
        with open("spotify_keys.txt", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if "=" in line and not line.startswith("#"):
                    k, v = line.split("=", 1)
                    if k.strip() == "CLIENT_ID":
                        return v.strip()
    except FileNotFoundError:
        pass
    return None


def _spotify_token_valido() -> Optional[str]:
    """Devuelve el access token si existe y no ha expirado; lo renueva si es necesario."""
    import time
    if not _spotify_oauth.get("access_token"):
        return None
    if time.time() < _spotify_oauth.get("expiry", 0) - 60:
        return _spotify_oauth["access_token"]
    # Renovar con refresh_token
    refresh = _spotify_oauth.get("refresh_token")
    cid     = _spotify_client_id()
    if not refresh or not cid:
        return None
    r = http_requests.post(
        "https://accounts.spotify.com/api/token",
        data={"grant_type": "refresh_token", "refresh_token": refresh, "client_id": cid},
    )
    if r.ok:
        d = r.json()
        _spotify_oauth["access_token"]  = d["access_token"]
        _spotify_oauth["expiry"]        = time.time() + d.get("expires_in", 3600)
        if "refresh_token" in d:
            _spotify_oauth["refresh_token"] = d["refresh_token"]
        return _spotify_oauth["access_token"]
    return None


# ─── MODELOS ──────────────────────────────────────────────────────────────────

class PlaylistRequest(BaseModel):
    url: str
    formato: str = "m4a"
    workers: int = 3

class TrackRequest(BaseModel):
    url: str            # URL de canción de Spotify
    formato: str = "m4a"


# ─── SPOTIFY (scraping — sin API key) ────────────────────────────────────────

_SPOTIFY_HEADERS = {
    "User-Agent": (
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
        "AppleWebKit/537.36 Chrome/124.0.0.0 Safari/537.36"
    ),
    "Accept-Language": "en-US,en;q=0.9",
}


def _spotify_next_data(url: str) -> dict:
    """Descarga una página de Spotify y extrae el JSON de __NEXT_DATA__."""
    r = http_requests.get(url, headers=_SPOTIFY_HEADERS, timeout=15)
    r.raise_for_status()
    m = re.search(r'<script id="__NEXT_DATA__"[^>]*>(.+?)</script>', r.text, re.DOTALL)
    if not m:
        raise ValueError("No se encontró __NEXT_DATA__ en la página de Spotify")
    return json.loads(m.group(1))


def obtener_info_cancion(url: str) -> dict:
    """Dado un enlace de canción de Spotify, devuelve nombre, artistas, album, cover_url."""
    match = re.search(r"track[:/]([A-Za-z0-9]+)", url)
    if not match:
        raise ValueError(
            "URL de canción de Spotify inválida. "
            "Debe ser del tipo: https://open.spotify.com/track/..."
        )
    tid = match.group(1)

    oembed_cover: Optional[str] = None

    # 1. oEmbed — siempre disponible, da título y thumbnail de forma fiable
    try:
        oe = http_requests.get(
            f"https://open.spotify.com/oembed?url=https://open.spotify.com/track/{tid}",
            timeout=10,
        ).json()
        oembed_cover = oe.get("thumbnail_url")
    except Exception:
        pass

    # 2. Embed page — __NEXT_DATA__ con artista, álbum y portada en alta res
    try:
        data = _spotify_next_data(f"https://open.spotify.com/embed/track/{tid}")
        entity = (
            data.get("props", {})
                .get("pageProps", {})
                .get("state", {})
                .get("data", {})
                .get("entity", {})
        )
        print(f"[spotify] claves entity: {list(entity.keys()) if entity else 'None'}")
        if entity and entity.get("name"):
            artistas = ", ".join(a.get("name", "") for a in entity.get("artists", []))
            album_data = entity.get("albumOfTrack", {})
            print(f"[spotify] claves albumOfTrack: {list(album_data.keys()) if album_data else 'None'}")
            sources = album_data.get("coverArt", {}).get("sources", [])
            cover_url = sources[-1]["url"] if sources else oembed_cover
            print(f"[spotify] cover_url={cover_url}")
            return {
                "nombre":    entity["name"],
                "artistas":  artistas,
                "album":     album_data.get("name", ""),
                "cover_url": cover_url,
            }
    except Exception as e:
        print(f"[spotify] embed falló: {e}")

    # 3. Fallback completo: solo oEmbed
    if oembed_cover is not None:
        return {
            "nombre":    oe.get("title", ""),
            "artistas":  "",
            "album":     "",
            "cover_url": oembed_cover,
        }

    raise ValueError("No se pudo obtener información de la canción desde Spotify")




def obtener_canciones_playlist(url: str):
    """
    Extrae la lista de canciones de una playlist de Spotify.
    Intenta primero con JSON-LD (navegador normal) y luego con
    Open Graph meta tags usando el User-Agent de Facebook crawler.
    """
    match = re.search(r"playlist[:/]([A-Za-z0-9]+)", url)
    if not match:
        raise ValueError("URL de playlist inválida")
    pid = match.group(1)

    # ── Método 1: JSON-LD con User-Agent de navegador ─────────────────────────
    r = http_requests.get(
        f"https://open.spotify.com/playlist/{pid}",
        headers=_SPOTIFY_HEADERS,
        timeout=15,
    )
    r.raise_for_status()
    html_browser = r.text

    ld_blocks = re.findall(
        r'<script type="application/ld\+json">(.+?)</script>', html_browser, re.DOTALL
    )
    print(f"[playlist] {len(ld_blocks)} bloque(s) JSON-LD encontrados")
    for i, ld_raw in enumerate(ld_blocks):
        try:
            data = json.loads(ld_raw)
        except Exception as e:
            print(f"[playlist] bloque {i}: JSON inválido — {e}")
            continue
        print(f"[playlist] bloque {i}: @type={data.get('@type')} claves={list(data.keys())[:8]}")
        if data.get("@type") != "MusicPlaylist":
            continue

        nombre     = data.get("name", "Playlist")
        raw_tracks = data.get("track", [])
        canciones  = []

        for item in raw_tracks:
            # JSON-LD puede envolver cada pista en ListItem
            if isinstance(item, dict) and item.get("@type") == "ListItem":
                item = item.get("item", {})
            if not isinstance(item, dict):
                continue

            nombre_cancion = item.get("name", "")
            if not nombre_cancion:
                continue

            artista = item.get("byArtist", {})
            if isinstance(artista, list):
                artistas = ", ".join(a.get("name", "") for a in artista if a.get("name"))
            else:
                artistas = artista.get("name", "") if isinstance(artista, dict) else ""

            album_obj = item.get("inAlbum", {})
            album     = album_obj.get("name", "") if isinstance(album_obj, dict) else ""

            canciones.append({
                "nombre":      nombre_cancion,
                "artistas":    artistas,
                "album":       album,
                "cover_url":   None,                # se rellena en tarea()
                "spotify_url": item.get("url", ""), # para obtener_info_cancion
            })

        if canciones:
            print(f"[playlist] JSON-LD encontrado: {len(canciones)} canciones en '{nombre}'")
            return nombre, canciones

    # ── Método 2: Open Graph music:song con User-Agent de Facebook crawler ─────
    print("[playlist] JSON-LD vacío — intentando con facebookexternalhit UA")
    r2 = http_requests.get(
        f"https://open.spotify.com/playlist/{pid}",
        headers={"User-Agent": "facebookexternalhit/1.1"},
        timeout=15,
    )
    r2.raise_for_status()
    html_fb = r2.text

    track_urls = re.findall(
        r'<meta\s+(?:property|name)=["\']music:song["\']\s+content=["\'](https://open\.spotify\.com/track/[^"\']+)["\']',
        html_fb,
    )
    print(f"[playlist] facebookexternalhit: {len(track_urls)} tracks encontrados")

    if track_urls:
        # Extraer nombre de la playlist desde og:title
        m_title = re.search(r'<meta\s+property=["\']og:title["\']\s+content=["\'](.*?)["\']', html_fb)
        nombre = m_title.group(1) if m_title else "Playlist"
        # Limpiar " | Spotify" del título si está presente
        nombre = re.sub(r'\s*[|·]\s*Spotify\s*$', '', nombre).strip() or "Playlist"

        canciones = [
            {
                "nombre":      "",          # tarea() lo rellena con obtener_info_cancion
                "artistas":    "",
                "album":       "",
                "cover_url":   None,
                "spotify_url": t_url,
            }
            for t_url in track_urls
        ]
        print(f"[playlist] OG meta: {len(canciones)} canciones en '{nombre}'")
        return nombre, canciones

    raise ValueError(
        "No se encontraron canciones en la playlist. "
        "La playlist puede ser privada o requerir autenticación."
    )


# ─── METADATOS ────────────────────────────────────────────────────────────────

def _descargar_portada(url: str) -> Optional[bytes]:
    """Descarga la imagen de portada y devuelve los bytes, o None si falla."""
    if not url:
        print("[cover] cover_url es None — sin portada")
        return None
    try:
        r = http_requests.get(url, timeout=10, headers={"User-Agent": "Mozilla/5.0"})
        r.raise_for_status()
        ct = r.headers.get("content-type", "?")
        print(f"[cover] OK — {len(r.content)} bytes, tipo: {ct}")
        return r.content
    except Exception as e:
        print(f"[cover] ERROR descargando portada: {e}")
        return None


def _imagen_mime_y_formato(data: bytes):
    """Detecta MIME y constante MP4Cover según los magic bytes."""
    from mutagen.mp4 import MP4Cover
    if data[:8] == b"\x89PNG\r\n\x1a\n":
        return "image/png", MP4Cover.FORMAT_PNG
    return "image/jpeg", MP4Cover.FORMAT_JPEG


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

    print(f"[meta] archivo={archivo.name}, portada={'sí' if cover_bytes else 'NO'}")
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
            audio = MutagenFile(str(archivo), easy=True)
            if audio is not None:
                audio["title"]  = nombre
                audio["artist"] = artistas
                audio["album"]  = album
                audio.save()
        print(f"[meta] OK — metadatos escritos en {archivo.name}")
    except Exception as e:
        print(f"[meta] ERROR escribiendo metadatos en {archivo.name}: {e}")


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
        mime, _ = _imagen_mime_y_formato(cover_bytes)
        tags["APIC"] = APIC(
            encoding=3,
            mime=mime,
            type=3,
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
        _, fmt = _imagen_mime_y_formato(cover_bytes)
        audio["covr"] = [MP4Cover(cover_bytes, imageformat=fmt)]
    audio.save()


def _escribir_flac(archivo: Path, nombre, artistas, album, cover_bytes):
    from mutagen.flac import FLAC, Picture
    audio = FLAC(str(archivo))
    audio["title"]  = [nombre]
    audio["artist"] = [artistas]
    audio["album"]  = [album]
    if cover_bytes:
        mime, _ = _imagen_mime_y_formato(cover_bytes)
        pic = Picture()
        pic.type = 3
        pic.mime = mime
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
        mime, _ = _imagen_mime_y_formato(cover_bytes)
        pic = Picture()
        pic.type = 3
        pic.mime = mime
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
        mime, _ = _imagen_mime_y_formato(cover_bytes)
        pic = Picture()
        pic.type = 3
        pic.mime = mime
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
        # "extractor_args": {
        #     "youtube": {
        #         "player_client": ["tv_embedded", "ios"],
        #     }
        # },
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

    archivo_descargado: Optional[Path] = None
    try:
        with yt_dlp.YoutubeDL(ydl_opts) as ydl:
            info = ydl.extract_info(search, download=True)
            entry = (info["entries"][0] if info and info.get("entries") else info) or {}
            titulo = entry.get("title", query)
            # yt-dlp registra el filepath final en requested_downloads
            rds = entry.get("requested_downloads", [])
            if rds:
                fp = rds[0].get("filepath") or rds[0].get("filename", "")
                if fp and Path(fp).exists():
                    archivo_descargado = Path(fp)
    except Exception as e:
        return False, str(e)

    # ── Escribir metadatos si se pasaron ──────────────────────────────────────
    if meta:
        # Fallback: buscar en carpeta si yt-dlp no reportó el filepath
        if archivo_descargado is None or not archivo_descargado.exists():
            EXTS_AUDIO = [formato, "m4a", "mp4", "mp3", "opus", "webm", "flac", "wav", "ogg"]
            archivo_descargado = None
            for ext in EXTS_AUDIO:
                encontrados = [f for f in carpeta.iterdir()
                               if f.is_file() and f.suffix.lower() == f".{ext}"]
                if encontrados:
                    archivo_descargado = max(encontrados, key=lambda f: f.stat().st_mtime)
                    break
            if archivo_descargado is None:
                todos = [f for f in carpeta.iterdir() if f.is_file()]
                archivo_descargado = max(todos, key=lambda f: f.stat().st_mtime) if todos else None

        if archivo_descargado:
            cover_bytes = _descargar_portada(meta.get("cover_url"))
            escribir_metadatos(
                archivo_descargado,
                nombre      = meta.get("nombre", titulo),
                artistas    = meta.get("artistas", ""),
                album       = meta.get("album", ""),
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
        # Usar obtener_info_cancion para rellenar nombre, artista, álbum y portada.
        # Necesario cuando la playlist se obtuvo via OG meta tags (nombre vacío).
        if cancion.get("spotify_url") and (not cancion.get("cover_url") or not cancion.get("nombre")):
            try:
                info = obtener_info_cancion(cancion["spotify_url"])
                if not cancion.get("nombre") and info.get("nombre"):
                    cancion["nombre"] = info["nombre"]
                cancion["cover_url"] = info.get("cover_url") or cancion.get("cover_url")
                if not cancion.get("artistas") and info.get("artistas"):
                    cancion["artistas"] = info["artistas"]
                if not cancion.get("album") and info.get("album"):
                    cancion["album"] = info["album"]
            except Exception:
                pass

        if not cancion.get("nombre"):
            # Si aún no hay nombre (info_cancion falló), usar la URL como fallback
            cancion["nombre"] = cancion.get("spotify_url", "unknown")

        query = f"{cancion['nombre']} {cancion['artistas']}"
        label = f"{cancion['nombre']} — {cancion['artistas']}"
        meta  = {
            "nombre":    cancion["nombre"],
            "artistas":  cancion["artistas"],
            "album":     cancion.get("album", ""),
            "cover_url": cancion.get("cover_url"),
        }
        # Subdirectorio propio para evitar colisiones entre workers concurrentes
        sub = carpeta / str(uuid.uuid4())
        sub.mkdir(exist_ok=True)
        ok, info = descargar_yt(query, sub, formato, meta=meta)
        if ok:
            prefix = f"{cancion['index']} - " if cancion.get("index") else ""
            safe = re.sub(r'[\\/:*?"<>|]', '', cancion["nombre"]).strip()
            for f in sub.iterdir():
                if f.is_file():
                    dest = carpeta / f"{prefix}{safe}{f.suffix}"
                    shutil.move(str(f), str(dest))
        shutil.rmtree(sub, ignore_errors=True)
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
        import traceback; traceback.print_exc()
        raise HTTPException(400, str(e))

    # Inyectar índice con ceros para preservar el orden de la playlist
    n_digits = len(str(len(canciones)))
    for i, c in enumerate(canciones, 1):
        c["index"] = str(i).zfill(n_digits)

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
    safe = re.sub(r'[\\/:*?"<>|]', '', nombre).strip()
    nombre_archivo = f"{safe}{archivo.suffix}"

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


# ─── OAUTH SPOTIFY ────────────────────────────────────────────────────────────

@app.get("/api/spotify/status")
async def spotify_status():
    return {"connected": _spotify_token_valido() is not None}


@app.get("/api/spotify/login")
async def spotify_login():
    """Inicia el flujo OAuth PKCE — redirige al navegador a la página de Spotify."""
    cid = _spotify_client_id()
    if not cid:
        raise HTTPException(400, "Falta CLIENT_ID en spotify_keys.txt")

    verifier   = secrets.token_urlsafe(64)
    challenge  = base64.urlsafe_b64encode(
        hashlib.sha256(verifier.encode()).digest()
    ).rstrip(b"=").decode()

    _spotify_oauth["pkce_verifier"] = verifier

    params = {
        "client_id":             cid,
        "response_type":         "code",
        "redirect_uri":          SPOTIFY_REDIRECT_URI,
        "code_challenge_method": "S256",
        "code_challenge":        challenge,
        "scope":                 SPOTIFY_SCOPES,
    }
    return RedirectResponse(
        "https://accounts.spotify.com/authorize?" + urllib.parse.urlencode(params)
    )


@app.get("/api/spotify/callback")
async def spotify_callback(code: Optional[str] = None, error: Optional[str] = None):
    """Recibe el código OAuth de Spotify, lo intercambia por tokens y los guarda."""
    import time
    if error:
        return HTMLResponse(f"<p>Error de Spotify: {error}</p>")
    if not code:
        raise HTTPException(400, "No se recibió code")

    cid      = _spotify_client_id()
    verifier = _spotify_oauth.get("pkce_verifier")
    if not cid or not verifier:
        raise HTTPException(400, "Estado OAuth inválido — reinicia el flujo")

    r = http_requests.post(
        "https://accounts.spotify.com/api/token",
        data={
            "grant_type":    "authorization_code",
            "code":          code,
            "redirect_uri":  SPOTIFY_REDIRECT_URI,
            "client_id":     cid,
            "code_verifier": verifier,
        },
    )
    r.raise_for_status()
    d = r.json()

    _spotify_oauth["access_token"]  = d["access_token"]
    _spotify_oauth["refresh_token"] = d.get("refresh_token", "")
    _spotify_oauth["expiry"]        = time.time() + d.get("expires_in", 3600)
    _spotify_oauth.pop("pkce_verifier", None)

    return HTMLResponse("""
        <p style="font-family:sans-serif;padding:2rem">
          ✓ Spotify conectado. Puedes cerrar esta pestaña.
        </p>
        <script>window.close();</script>
    """)


# Servir el frontend estático
app.mount("/", StaticFiles(directory="static", html=True), name="static")
