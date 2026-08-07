<#
.SYNOPSIS
    Genera los ficheros de audio de prueba del motor.

.DESCRIPTION
    Un tono de 440 Hz, un segundo, estereo, en cada formato que Localify debe
    saber reproducir. Se generan una vez y se versionan: la suite de tests NO
    debe depender de que FFmpeg este instalado, igual que no depende de yt-dlp
    ni de la red.

    Son deliberadamente pequenos (unas decenas de KB en total) y sinteticos, sin
    ninguna grabacion con derechos.

    Cada formato cubre algo distinto:
      opus  contenedor Ogg + libopus. Es lo que llega de YouTube.
      m4a   AAC en ISO/MP4. La alternativa cuando Opus no esta disponible.
      mp3   flujo de tramas. Lo mas comun en una biblioteca heredada.
      flac  sin perdida, a 44.1 kHz: obliga a remuestrear.
      ogg   Vorbis.
      wav   PCM sin comprimir, la referencia contra la que comparar.

.PARAMETER Ffmpeg
    Ruta a ffmpeg.exe. Por defecto, el sidecar de Localify.

.EXAMPLE
    .\scripts\gen-fixtures-audio.ps1
#>
[CmdletBinding()]
param(
    [string] $Ffmpeg = (Join-Path $env:APPDATA 'Localify\bin\ffmpeg.exe')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not (Test-Path $Ffmpeg)) {
    throw "No se encuentra ffmpeg en '$Ffmpeg'. Ejecuta antes .\scripts\fetch-sidecars.ps1"
}

$destino = Join-Path $PSScriptRoot '..\crates\localify-audio\tests\fixtures'
$destino = [IO.Path]::GetFullPath($destino)
if (-not (Test-Path $destino)) { New-Item -ItemType Directory -Path $destino | Out-Null }

# 440 Hz (la del diapason) durante un segundo, en los dos canales.
#
# El filtro `sine` de FFmpeg no genera a fondo de escala: sale a unos -21 dBFS.
# Se sube explicitamente a ~-6 dBFS (amplitud 0.5), que es un nivel realista de
# musica y deja margen para que el limitador no tenga que actuar en los tests.
$fuente = @(
    '-f', 'lavfi', '-i', 'sine=frequency=440:duration=1:sample_rate=48000',
    '-af', 'volume=15dB', '-ac', '2'
)

# Formato -> argumentos de codificacion.
$formatos = [ordered]@{
    'tono.wav'  = @('-c:a', 'pcm_s16le')
    'tono.flac' = @('-c:a', 'flac', '-ar', '44100')   # a 44.1 kHz: obliga a remuestrear
    'tono.mp3'  = @('-c:a', 'libmp3lame', '-b:a', '128k')
    'tono.m4a'  = @('-c:a', 'aac', '-b:a', '128k')
    'tono.ogg'  = @('-c:a', 'libvorbis', '-b:a', '128k')
    'tono.opus' = @('-c:a', 'libopus', '-b:a', '128k')
}

# `-loglevel error` en vez de redirigir con `2>&1`: en Windows PowerShell 5.1,
# redirigir la salida de error de un ejecutable nativo la envuelve en
# ErrorRecords y hace fallar el script aunque ffmpeg devuelva 0.
$silencio = @('-hide_banner', '-loglevel', 'error')

foreach ($nombre in $formatos.Keys) {
    $ruta = Join-Path $destino $nombre
    Write-Host "==> $nombre" -ForegroundColor Cyan
    $parametros = $silencio + $fuente + $formatos[$nombre] + @('-y', $ruta)
    & $Ffmpeg @parametros
    if ($LASTEXITCODE -ne 0) { throw "ffmpeg fallo generando $nombre" }
    $kb = [math]::Round((Get-Item $ruta).Length / 1KB, 1)
    Write-Host "    $kb KB" -ForegroundColor Green
}

# Un fichero multicanal: comprueba que la voz del canal central no se pierde.
Write-Host '==> tono-5.1.flac' -ForegroundColor Cyan
$ruta = Join-Path $destino 'tono-5.1.flac'
$parametros = $silencio + @(
    '-f', 'lavfi', '-i', 'sine=frequency=440:duration=1:sample_rate=48000',
    '-af', 'volume=15dB,pan=5.1|FC=c0', '-c:a', 'flac', '-y', $ruta
)
& $Ffmpeg @parametros
if ($LASTEXITCODE -ne 0) { throw 'ffmpeg fallo generando tono-5.1.flac' }
Write-Host "    $([math]::Round((Get-Item $ruta).Length / 1KB, 1)) KB" -ForegroundColor Green

Write-Host ''
Write-Host "Listo. Ficheros en $destino" -ForegroundColor Green
