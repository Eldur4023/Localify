#!/usr/bin/env python3
# migrar_a_mp3.py — Convierte a MP3 (320k, ID3v2.3 + portada) todo el audio
# existente de Localify: la biblioteca local (audio_files en localify.db) y,
# opcionalmente, el audio del servidor (tracks.ext en serverify.db).
#
# Uso:
#   ./tools/migrar_a_mp3.py                                 # solo local
#   ./tools/migrar_a_mp3.py --server-db srv/data/serverify.db \
#                          --server-audio srv/data/audio     # también servidor
#
# Rutas por defecto (se pueden sobreescribir con --db / --library):
#   db      = ~/.local/share/lux-desktop/localify/data/localify.db
#   library = raiz de biblioteca (setting library_path, o ~/Music | ~/Música)
#
# IMPORTANTE: cierra la app antes de migrar (la base de datos queda bloqueada
# en WAL mientras corre). El script es idempotente: lo que ya es mp3 se salta.

import argparse
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile

BITRATE = "320k"


def ffprobe_ok(path):
    try:
        r = subprocess.run(
            ["ffprobe", "-v", "error", "-show_entries", "format=format_name",
             "-of", "default=nw=1:nk=1", path],
            capture_output=True, text=True, timeout=30)
        return r.returncode == 0 and r.stdout.strip() != ""
    except Exception:
        return False


def ffmpeg_tagged(src, dst, tags, cover=None):
    """Transcode src → dst (mp3 320k) with ID3 tags and optional cover."""
    args = ["ffmpeg", "-y", "-v", "error", "-i", src]
    if cover:
        args += ["-i", cover, "-map", "0:a", "-map", "1:v", "-c:v", "copy",
                 "-metadata:s:v", "title=Album cover",
                 "-metadata:s:v", "comment=Cover (front)"]
    else:
        args += ["-vn"]
    args += ["-c:a", "libmp3lame", "-b:a", BITRATE,
             "-id3v2_version", "3", "-write_id3v1", "1", "-map_metadata", "-1"]
    for k, v in tags.items():
        if v:
            args += ["-metadata", f"{k}={v}"]
    args += [dst]
    r = subprocess.run(args, capture_output=True, text=True, timeout=600)
    return r.returncode == 0


def default_library():
    home = os.path.expanduser("~")
    for cand in (os.path.join(home, "Music"), os.path.join(home, "Música"),
                 os.path.join(home, "Musica")):
        if os.path.isdir(cand):
            return cand
    return os.path.join(home, "Music")


def default_db(library):
    # el setting library_path manda; la db vive junto a data/ de la app
    return os.path.expanduser(
        "~/.local/share/lux-desktop/localify/data/localify.db")


def cover_for(covers_dir, album_id, track_id):
    candidates = []
    if album_id:
        candidates.append(os.path.join(covers_dir, f"album-{album_id}.jpg"))
    candidates.append(os.path.join(covers_dir, f"track2-{track_id}.jpg"))
    candidates.append(os.path.join(covers_dir, f"track-{track_id}.jpg"))
    for p in candidates:
        if os.path.isfile(p):
            return p
    return None


def migrate_local(db_path, library):
    con = sqlite3.connect(db_path)
    con.row_factory = sqlite3.Row
    covers = os.path.join(library, "covers")
    rows = con.execute(
        """select af.track_id, af.rel_path, t.title, t.artist_display,
                  t.isrc, t.track_number, t.album_id,
                  al.title as album, al.release_date
           from audio_files af
           join tracks t on t.id = af.track_id
           left join albums al on al.id = t.album_id""").fetchall()

    ok = skip = fail = 0
    for r in rows:
        src = os.path.join(library, r["rel_path"])
        if not os.path.isfile(src):
            print(f"  · falta el fichero, se salta: {r['rel_path']}")
            skip += 1
            continue
        if src.lower().endswith(".mp3"):
            con.execute("update audio_files set format='mp3', codec='mp3' "
                        "where track_id=?", (r["track_id"],))
            skip += 1
            continue
        if not ffprobe_ok(src):
            print(f"  · ilegible (ffprobe), se salta: {r['rel_path']}")
            fail += 1
            continue

        new_rel = r["rel_path"][: r["rel_path"].rfind(".")] + ".mp3"
        dst = os.path.join(library, new_rel)
        tags = {
            "title": r["title"], "artist": r["artist_display"],
            "album": r["album"], "date": r["release_date"],
            "isrc": r["isrc"],
            "track": str(r["track_number"]) if r["track_number"] else "",
        }
        tmp_fd, tmp = tempfile.mkstemp(suffix=".mp3",
                                       dir=os.path.dirname(dst))
        os.close(tmp_fd)
        try:
            if not ffmpeg_tagged(src, tmp, tags,
                                 cover_for(covers, r["album_id"], r["track_id"])):
                print(f"  · ERROR transcodificando: {r['rel_path']}")
                fail += 1
                continue
            os.replace(tmp, dst)
            con.execute(
                """update audio_files set rel_path=?, format='mp3',
                       codec='mp3', size_bytes=? where track_id=?""",
                (new_rel, os.path.getsize(dst), r["track_id"]))
            con.commit()   # la BD apunta al mp3 ANTES de borrar el original
            if dst != src:
                os.remove(src)
            # el servidor ya no necesita esta pista en el formato viejo: se
            # re-subirá en mp3 si hay sesión activa (srv_subir la ignora si el
            # servidor la tiene; bórrala allí a mano si quieres forzar)
            print(f"  ✓ {r['rel_path']} → {new_rel}")
            ok += 1
        finally:
            if os.path.exists(tmp):
                os.remove(tmp)
    con.commit()
    con.close()
    return ok, skip, fail


def migrate_server(db_path, audio_dir):
    con = sqlite3.connect(db_path)
    con.row_factory = sqlite3.Row
    rows = con.execute(
        "select id, ext, title, artist, album from tracks "
        "where ext is not null and ext != 'mp3'").fetchall()
    ok = skip = fail = 0
    for r in rows:
        src = os.path.join(audio_dir, f"{r['id']}.{r['ext']}")
        if not os.path.isfile(src):
            skip += 1
            continue
        dst = os.path.join(audio_dir, f"{r['id']}.mp3")
        tags = {"title": r["title"], "artist": r["artist"],
                "album": r["album"]}
        tmp_fd, tmp = tempfile.mkstemp(suffix=".mp3", dir=audio_dir)
        os.close(tmp_fd)
        try:
            if not ffmpeg_tagged(src, tmp, tags):
                print(f"  · ERROR (servidor): {src}")
                fail += 1
                continue
            os.replace(tmp, dst)
            con.execute("update tracks set ext='mp3' where id=?", (r["id"],))
            con.commit()   # la BD apunta al mp3 ANTES de borrar el original
            os.remove(src)
            print(f"  ✓ servidor: {r['id']}.{r['ext']} → {r['id']}.mp3")
            ok += 1
        finally:
            if os.path.exists(tmp):
                os.remove(tmp)
    con.commit()
    con.close()
    return ok, skip, fail


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=default_db(None))
    ap.add_argument("--library", default=default_library())
    ap.add_argument("--server-only", action="store_true",
                    help="solo migra el servidor (ignora --db/--library)")
    ap.add_argument("--server-db", default=None,
                    help="ruta a serverify.db del servidor")
    ap.add_argument("--server-audio", default=None,
                    help="directorio data/audio del servidor")
    args = ap.parse_args()

    if args.server_only:
        if not (args.server_db and args.server_audio):
            sys.exit("--server-only requiere --server-db y --server-audio")
    else:
        if not os.path.isfile(args.db):
            sys.exit(f"no existe la base de datos: {args.db}")
        if shutil.which("ffmpeg") is None:
            sys.exit("ffmpeg no está en el PATH")

    if not args.server_only:
        print(f"Biblioteca: {args.library}")
        print(f"Base de datos: {args.db}\n")
        ok, skip, fail = migrate_local(args.db, args.library)
        print(f"\nLocal: {ok} convertidas, {skip} saltadas, {fail} con error")

    if args.server_db and args.server_audio:
        print(f"Servidor: {args.server_db}")
        ok, skip, fail = migrate_server(args.server_db, args.server_audio)
        print(f"Servidor: {ok} convertidos, {skip} saltados, {fail} con error")


if __name__ == "__main__":
    main()
