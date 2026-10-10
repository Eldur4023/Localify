#!/usr/bin/env python3
"""De extremo a extremo: backend LuxScript + motor de audio nativo, sin ventana
y sin sonido (miniaudio en modo `null`). Todo en un directorio temporal: no toca
la biblioteca ni la base de datos de nadie.

    python3 tests/e2e_audio.py [build/app-headless]

Necesita ffmpeg en el PATH.
"""
import http.client
import json
import math
import os
import shutil
import signal
import sqlite3
import struct
import subprocess
import sys
import tempfile
import threading
import time
import wave

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "build", "app-headless")
fallos = []
ACTIVOS = []   # servidores en marcha: se paran siempre, falle lo que falle


def check(cond, msg):
    if not cond:
        fallos.append(msg)
        print("  FALLO:", msg)


def wav(path, seconds, hz):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    w = wave.open(path, "wb")
    w.setnchannels(2); w.setsampwidth(2); w.setframerate(48000)
    w.writeframes(b"".join(struct.pack("<hh", s, s) for s in
                           (int(math.sin(2 * math.pi * hz * i / 48000) * 12000) for i in range(int(seconds * 48000)))))
    w.close()


class Server:
    def __init__(self, tmp):
        self.tmp = tmp
        env = dict(os.environ, HOME=tmp)
        env.setdefault("LUX_AUDIO_NULL", "1")   # LUX_AUDIO_NULL=0 desde fuera: dispositivo real
        self.proc = subprocess.Popen([BIN, os.path.join(tmp, "app")], env=env, stdout=subprocess.PIPE,
                                     stderr=subprocess.STDOUT, text=True)
        ACTIVOS.append(self)
        self.log = []
        self.port = None
        ready = threading.Event()

        def leer():
            for linea in self.proc.stdout:
                self.log.append(linea.rstrip())
                if linea.startswith("port "):
                    self.port = int(linea.split()[1]); ready.set()
        threading.Thread(target=leer, daemon=True).start()
        if not ready.wait(20):
            raise SystemExit("el servidor no arranco:\n" + "\n".join(self.log))
        for _ in range(100):                      # `on start:` responde 503 hasta que acaba
            try:
                if self.get("/api/ready")[0] == 200:
                    return
            except OSError:
                pass
            time.sleep(0.1)
        raise SystemExit("el servidor nunca estuvo listo:\n" + "\n".join(self.log))

    def get(self, ruta):
        c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        c.request("GET", ruta)
        r = c.getresponse()
        return r.status, r.read()

    def invoke(self, cmd, args=None):
        c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        c.request("POST", "/api/invoke", json.dumps({"cmd": cmd, "args": json.dumps(args or {})}),
                  {"Content-Type": "application/json"})
        r = c.getresponse()
        cuerpo = r.read()
        if r.status not in (200, 204):
            raise RuntimeError(f"{cmd}: {r.status} {cuerpo[:200]}")
        return json.loads(cuerpo) if cuerpo else None

    def stop(self):
        self.proc.send_signal(signal.SIGTERM)
        try:
            self.proc.wait(10)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def esperar(cond, segundos, msg):
    fin = time.time() + segundos
    while time.time() < fin:
        v = cond()
        if v:
            return v
        time.sleep(0.05)
    check(False, "tiempo agotado: " + msg)
    return None


def main():
    if not os.path.exists(BIN):
        raise SystemExit(f"no existe {BIN} (cmake --build build --target app-headless)")
    tmp = tempfile.mkdtemp(prefix="localify-e2e-")
    try:
        shutil.copytree(os.path.join(ROOT, "app"), os.path.join(tmp, "app"), ignore=shutil.ignore_patterns("data"))
        lib = os.path.join(tmp, "lib")
        for tid, hz, seg in (("AAAAAAAAAAA", 440, 3.0), ("BBBBBBBBBBB", 660, 3.0), ("CCCCCCCCCCC", 880, 3.0),
                             ("DDDDDDDDDDD", 523, 6.0)):
            wav(os.path.join(lib, "audio", tid[:2], tid + ".wav"), seg, hz)

        s = Server(tmp)
        db = sqlite3.connect(os.path.join(tmp, "app", "data", "localify.db"), timeout=10)
        db.execute("insert into settings(key, value) values('library_path', ?)", (lib,))
        db.execute("insert into albums(id, title, title_norm) values('alb', 'Album', 'album')")
        for n, tid in enumerate(("AAAAAAAAAAA", "BBBBBBBBBBB", "CCCCCCCCCCC", "DDDDDDDDDDD"), 1):
            db.execute("insert into tracks(id, title, title_norm, album_id, duration_ms, track_number, artist_display, artist_norm)"
                       " values(?, ?, ?, ?, ?, ?, 'Artista', 'artista')",
                       (tid, "Pista " + tid[0], "pista", None if tid[0] == "D" else "alb", 6000 if tid[0] == "D" else 3000, n))
            db.execute("insert into audio_files(track_id, rel_path, format) values(?, ?, 'wav')",
                       (tid, f"audio/{tid[:2]}/{tid}.wav"))
        db.commit()
        A, B, C, D = "AAAAAAAAAAA", "BBBBBBBBBBB", "CCCCCCCCCCC", "DDDDDDDDDDD"

        print("SSE: la primera foto llega sola y trae el estado")
        fotos = []
        def sse():
            c = http.client.HTTPConnection("127.0.0.1", s.port, timeout=30)
            c.request("GET", "/api/events/stream")
            r = c.getresponse()
            evento = None
            for raw in r:
                linea = raw.decode().rstrip("\n")
                if linea.startswith("event: "): evento = linea[7:]
                elif linea.startswith("data: ") and evento == "snapshot":
                    fotos.append(json.loads(linea[6:]))
        threading.Thread(target=sse, daemon=True).start()
        esperar(lambda: fotos, 5, "primera foto SSE")
        check(fotos and "estado" in fotos[0] and "downloads" in fotos[0], "la foto no trae estado/descargas")

        print("reproducir una pista de un album")
        st = s.invoke("player_play_track", {"trackId": A, "context": {"kind": "album", "id": "alb"}})
        check(st["track"]["id"] == A and st["status"] == "playing", f"estado tras play: {st['status']}")
        pos = esperar(lambda: (lambda p: p if p and p > 600 else None)(s.invoke("player_get_state")["positionMs"]), 6,
                      "la posicion avanza (el motor suena)")
        print("  posicion tras unos instantes:", pos)

        print("el SSE empuja la posicion mientras suena")
        n0 = len(fotos); time.sleep(1.2)
        check(len(fotos) - n0 >= 3, f"solo {len(fotos) - n0} fotos en 1,2 s")
        ultima = fotos[-1]["estado"]
        check(ultima["status"] == "playing" and ultima["positionMs"] > 0, "la foto no refleja que suena")

        print("pausa: la posicion se queda quieta; reanudar sigue")
        st = s.invoke("player_pause")
        check(st["status"] == "paused", "no quedo en pausa")
        time.sleep(0.4)
        p1 = s.invoke("player_get_state")["positionMs"]; time.sleep(0.6)
        p2 = s.invoke("player_get_state")["positionMs"]
        check(abs(p2 - p1) < 40, f"se movio en pausa: {p1} -> {p2}")
        s.invoke("player_resume")
        esperar(lambda: s.invoke("player_get_state")["positionMs"] > p2 + 300, 4, "reanuda")

        print("seek")
        st = s.invoke("player_seek", {"positionMs": 2000})
        check(1900 <= st["positionMs"] <= 2300, f"posicion tras seek: {st['positionMs']}")

        print("relevo natural A -> B sin que nadie toque nada")
        esperar(lambda: s.invoke("player_get_state")["track"]["id"] == B, 6, "avanza a B por si solo")
        st = s.invoke("player_get_state")
        check(st["status"] == "playing", "tras el relevo no suena")
        check(st["positionMs"] < 2500, f"B no empezo desde el principio: {st['positionMs']}")
        check(any("handoff " + B in l for l in s.log), "A -> B no fue un relevo del motor (gapless), sino una recarga")
        # un aviso Ended/handoff nunca debe avanzar dos veces
        time.sleep(0.8)
        check(s.invoke("player_get_state")["track"]["id"] == B, "avanzo de mas")

        print("siguiente / anterior")
        st = s.invoke("player_next")
        check(st["track"]["id"] == C, "next no llego a C")
        time.sleep(0.5)
        st = s.invoke("player_previous")           # <3 s: pista anterior, no reiniciar
        check(st["track"]["id"] == B, f"previous: {st['track']['id']}")
        s.invoke("player_next")

        print("fin de la cola: se para y la ultima se queda al principio")
        esperar(lambda: s.invoke("player_get_state")["status"] == "stopped", 6, "se para al acabar C")
        check(any("ended " + C in l for l in s.log), "el fin de C no llego como `ended` del motor")
        st = s.invoke("player_get_state")
        check(st["track"]["id"] == C and st["positionMs"] < 300, f"C deberia quedarse al principio: {st['track']['id']} {st['positionMs']}")

        print("ajustes de audio (EQ, crossfade) se aplican sin error")
        s.invoke("settings_patch", {"patch": {"audio": {"crossfadeMs": 500, "gapless": True, "outputDeviceId": None,
                 "eqProfile": {"id": "x", "nameKey": "x", "gainsDb": [3, 2, 1, 0, 0, 0, 0, 1, 2, 3]}}}})
        s.invoke("settings_preview_eq", {"profile": {"gainsDb": [0] * 10}})
        devs = s.invoke("settings_audio_devices")
        check(isinstance(devs, list) and all("id" in d and "name" in d for d in devs), f"dispositivos: {devs}")

        print("estadisticas de escucha (cuenta desde 5 s, y solo lo que sono de verdad)")
        s.invoke("player_play_track", {"trackId": D, "context": {"kind": "single"}})
        time.sleep(5.6)
        s.invoke("player_pause")
        time.sleep(0.5)
        filas = {r[0]: r[1] for r in db.execute("select track_id, max(ms_played) from play_history group by track_id")}
        check(D in filas and 5000 <= filas[D] <= 6300, f"escuchado de D: {filas.get(D)}")
        check(A not in filas and B not in filas, f"pistas de 3 s no deben contar: {sorted(filas)}")

        print("la sesion se restaura en pausa tras reiniciar")
        s.invoke("player_play_track", {"trackId": A, "context": {"kind": "album", "id": "alb"}})
        esperar(lambda: s.invoke("player_get_state")["positionMs"] > 800, 5, "suena A")
        s.invoke("player_pause")
        pos_antes = s.invoke("player_get_state")["positionMs"]
        s.stop()
        s = Server(tmp)
        st = s.invoke("player_get_state")
        check(st["track"]["id"] == A and st["status"] == "paused", f"restauracion: {st['track']['id']} {st['status']}")
        check(abs(st["positionMs"] - pos_antes) < 400, f"posicion restaurada {st['positionMs']} vs {pos_antes}")

        malos = [l for l in s.log if "failed" in l.lower() or "error" in l.lower()]
        check(not malos, "el log del servidor trae errores:\n    " + "\n    ".join(malos[:8]))
        s.stop()
    finally:
        for srv in ACTIVOS:
            if srv.proc.poll() is None:
                srv.stop()
        shutil.rmtree(tmp, ignore_errors=True)

    prueba_biblioteca()
    print("\n%s" % (f"{len(fallos)} FALLO(S)" if fallos else "todo bien"))
    sys.exit(1 if fallos else 0)


def prueba_biblioteca():
    """Duplicados, biblioteca e Inicio, sobre una biblioteca sembrada a mano."""
    print("\n== biblioteca: duplicados, Canciones e Inicio ==")
    tmp = tempfile.mkdtemp(prefix="localify-e2e-bib-")
    try:
        shutil.copytree(os.path.join(ROOT, "app"), os.path.join(tmp, "app"), ignore=shutil.ignore_patterns("data"))
        lib = os.path.join(tmp, "lib")
        s = Server(tmp)
        db = sqlite3.connect(os.path.join(tmp, "app", "data", "localify.db"), timeout=10)
        x = db.execute
        x("insert into settings(key, value) values('library_path', ?)", (lib,))
        x("insert into artists(id, name, name_norm) values('UCartista000000000000000', 'Artista', 'artista')")
        x("insert into albums(id, title, title_norm) values('alb', 'Album', 'album')")
        def pista(tid, titulo, dur, album=None):
            x("insert into tracks(id, title, title_norm, album_id, duration_ms, artist_display, artist_norm) values(?, ?, lower(?), ?, ?, 'Artista', 'artista')",
              (tid, titulo, titulo, album, dur))
            x("insert into track_artists(track_id, artist_id, position) values(?, 'UCartista000000000000000', 0)", (tid,))
        # La misma canción dos veces: una importada (id largo, con fichero) y otra de YouTube.
        pista("SPOTIFYID00000000000001", "Repetida", 200000)
        pista("ytREPETIDA1", "Repetida", 200500)
        x("insert into audio_files(track_id, rel_path, format) values('SPOTIFYID00000000000001', 'audio/SP/x.opus', 'opus')")
        x("insert into favorites(track_id) values('ytREPETIDA1')")
        x("insert into playlists(id, name, name_norm, created_at, updated_at) values('pl', 'Lista', 'lista', 0, 0)")
        x("insert into playlist_items(id, playlist_id, track_id, position, added_at) values('i1', 'pl', 'ytREPETIDA1', 1, 0)")
        for tid in ("SPOTIFYID00000000000001", "ytREPETIDA1"):
            for _ in range(3):
                x("insert into play_history(id, track_id, played_at, ms_played, completed, context) values(hex(randomblob(8)), ?, unixepoch(), 200000, 1, 'x')", (tid,))
        # Título que YouTube Music manda dos veces.
        pista("ytDOBLETITU", "TITULO (“EN DIRECTO”) - TITULO (\"EN DIRECTO\")", 180000)
        # Del catálogo, sin escuchar: candidatos a Descubre (y una versión en directo que no debe salir).
        for n in range(1, 6):
            pista(f"ytNUEVA{n:04d}", f"Nueva {n}", 190000 + n, "alb")
        pista("ytNUEVALIVE", "Nueva 1 (Live)", 250000, "alb")
        db.commit()
        s.stop()

        s = Server(tmp)   # al arrancar: fusión de duplicados y reparación de títulos
        ids = {r[0] for r in db.execute("select id from tracks")}
        check("SPOTIFYID00000000000001" in ids and "ytREPETIDA1" not in ids, f"no se fusionó la repetida: {sorted(ids)}")
        check(db.execute("select count(*) from play_history where track_id = 'SPOTIFYID00000000000001'").fetchone()[0] == 6,
              "el historial de las dos copias no quedó sumado")
        check(db.execute("select count(*) from favorites where track_id = 'SPOTIFYID00000000000001'").fetchone()[0] == 1, "el me gusta no pasó a la que queda")
        check(db.execute("select track_id from playlist_items where id = 'i1'").fetchone()[0] == "SPOTIFYID00000000000001", "la playlist sigue apuntando a la borrada")
        check(os.path.exists(os.path.join(tmp, "app", "data", "localify.antes-de-fusionar.db")), "no se hizo la copia de seguridad")
        titulo = db.execute("select title from tracks where id = 'ytDOBLETITU'").fetchone()[0]
        check(titulo == "TITULO (“EN DIRECTO”)", f"título repetido sin reparar: {titulo!r}")

        print("Canciones = tu biblioteca, no el caché del catálogo")
        r = s.invoke("library_tracks", {"filter": {"favoritesOnly": False, "localOnly": False}, "sort": "addedDesc", "page": {"offset": 0, "limit": 100}})
        titulos = [i["title"] for i in r["items"]]
        check(titulos == ["Repetida"] and r["total"] == 1, f"Canciones: {titulos} total={r['total']}")
        r = s.invoke("library_tracks", {"filter": {"albumId": "alb"}, "sort": "addedDesc", "page": {"offset": 0, "limit": 100}})
        check(len(r["items"]) == 6, f"la vista del álbum debe enseñar todo el álbum: {len(r['items'])}")

        print("Inicio empieza por descubrir: nada escuchado, sin directos")
        secciones = s.invoke("home_sections")
        claves = [x["key"] for x in secciones]
        check(claves and claves[0] == "home.discover", f"orden de Inicio: {claves}")
        descubre = next((x for x in secciones if x["key"] == "home.discover"), None)
        nombres = [i["title"] for i in descubre["items"]["items"]] if descubre else []
        check(set(nombres) == {f"Nueva {n}" for n in range(1, 6)}, f"Descubre: {nombres}")
        s.stop()
    finally:
        for srv in ACTIVOS:
            if srv.proc.poll() is None:
                srv.stop()
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
