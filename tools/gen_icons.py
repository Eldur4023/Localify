#!/usr/bin/env python3
"""Genera los iconos de Localify (escritorio y Android): el diseño original —círculo
negro con un punto en el centro— con el punto en naranja de la familia Lux (#D97757).

    python3 tools/gen_icons.py
Escribe app/icon.png y android/src/main/res/mipmap-*/ic_launcher*.png
"""
from PIL import Image, ImageDraw
from pathlib import Path

RAIZ = Path(__file__).resolve().parent.parent
NEGRO, NARANJA = (17, 17, 17, 255), (217, 119, 87, 255)
S = 2048  # lienzo de trabajo (se reduce al final)
PUNTO = 0.43  # diámetro del punto respecto al círculo completo


def circulo(color, fraccion, lienzo=S):
    capa = Image.new("RGBA", (lienzo, lienzo), (0, 0, 0, 0))
    r = int(lienzo * fraccion) // 2
    c = lienzo // 2
    ImageDraw.Draw(capa).ellipse((c - r, c - r, c + r, c + r), fill=color)
    return capa


def guardar(img, ruta, lado):
    ruta.parent.mkdir(parents=True, exist_ok=True)
    img.resize((lado, lado), Image.LANCZOS).save(ruta)


icono = circulo(NEGRO, 1.0)
icono.alpha_composite(circulo(NARANJA, PUNTO))
guardar(icono, RAIZ / "app/icon.png", 256)

res = RAIZ / "android/src/main/res"
# Capas adaptables de 108 dp: la máscara deja ~72 dp visibles, así que el punto mide
# PUNTO * 72/108 del lienzo para quedar igual que en el círculo completo.
punto = circulo(NARANJA, PUNTO * 72 / 108)
mono = circulo((0, 0, 0, 255), PUNTO * 72 / 108)
# densidad: (lado del icono clásico, lado de las capas adaptables)
for dens, (clasico, capa) in {"mdpi": (48, 108), "hdpi": (72, 162), "xhdpi": (96, 216),
                              "xxhdpi": (144, 324), "xxxhdpi": (192, 432)}.items():
    d = res / f"mipmap-{dens}"
    guardar(icono, d / "ic_launcher.png", clasico)
    guardar(punto, d / "ic_launcher_foreground.png", capa)
    guardar(Image.new("RGBA", (S, S), NEGRO), d / "ic_launcher_background.png", capa)
    guardar(mono, d / "ic_launcher_monochrome.png", capa)
