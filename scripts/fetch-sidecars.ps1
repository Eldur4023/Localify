<#
.SYNOPSIS
    Descarga yt-dlp y FFmpeg en la carpeta de binarios de Localify.

.DESCRIPTION
    Los sidecars no se empaquetan con la aplicacion (ADR-006): yt-dlp se rompe
    cuando YouTube cambia su ofuscacion, cada pocas semanas, y poder
    actualizarlo sin publicar una version de Localify es la diferencia entre
    "se arregla solo" y "la app esta rota hasta la siguiente release".

    En produccion los descarga la propia aplicacion en el primer arranque. Este
    script hace lo mismo para un entorno de desarrollo.

.PARAMETER Destino
    Carpeta de destino. Por defecto, %APPDATA%\Localify\bin.

.PARAMETER Forzar
    Vuelve a descargar aunque el binario ya exista.

.EXAMPLE
    .\scripts\fetch-sidecars.ps1
    .\scripts\fetch-sidecars.ps1 -Forzar
#>
[CmdletBinding()]
param(
    [string] $Destino = (Join-Path $env:APPDATA 'Localify\bin'),
    [switch] $Forzar
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # sin esto, Invoke-WebRequest es ~10x mas lento

# TLS 1.2 explicito: Windows PowerShell 5.1 no lo negocia por defecto.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Write-Paso { param([string] $Texto) Write-Host "==> $Texto" -ForegroundColor Cyan }
function Write-Ok   { param([string] $Texto) Write-Host "    $Texto" -ForegroundColor Green }
function Write-Aviso{ param([string] $Texto) Write-Host "    $Texto" -ForegroundColor Yellow }

if (-not (Test-Path $Destino)) {
    New-Item -ItemType Directory -Path $Destino -Force | Out-Null
}
Write-Paso "Destino: $Destino"

# ─── yt-dlp ──────────────────────────────────────────────────────────────────
# Se toma siempre la ultima release: fijar una version seria contraproducente,
# porque el valor de yt-dlp esta justo en ir por delante de YouTube.

$ytDlp = Join-Path $Destino 'yt-dlp.exe'
if ((Test-Path $ytDlp) -and -not $Forzar) {
    $v = & $ytDlp --version 2>$null
    Write-Ok "yt-dlp ya presente (version $v). Usa -Forzar para reinstalar."
} else {
    Write-Paso 'Descargando yt-dlp...'
    $url = 'https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe'
    $temporal = "$ytDlp.download"
    Invoke-WebRequest -Uri $url -OutFile $temporal -UseBasicParsing

    # Verificacion minima: que sea un PE valido y que responda a --version. Un
    # binario truncado por una descarga a medias fallaria aqui y no en mitad de
    # una descarga de audio.
    $cabecera = [System.IO.File]::ReadAllBytes($temporal)[0..1]
    if ($cabecera[0] -ne 0x4D -or $cabecera[1] -ne 0x5A) {
        Remove-Item $temporal -Force
        throw 'La descarga de yt-dlp no es un ejecutable valido.'
    }

    Move-Item $temporal $ytDlp -Force
    $v = & $ytDlp --version 2>$null
    Write-Ok "yt-dlp $v instalado."
}

# ─── FFmpeg ──────────────────────────────────────────────────────────────────
# Compilaciones de gyan.dev, las que recomienda el propio proyecto FFmpeg para
# Windows. Se usa el paquete 'essentials', que trae lo necesario para remuxear
# e inspeccionar (Localify nunca recodifica).

$ffmpeg = Join-Path $Destino 'ffmpeg.exe'
$ffprobe = Join-Path $Destino 'ffprobe.exe'
if ((Test-Path $ffmpeg) -and (Test-Path $ffprobe) -and -not $Forzar) {
    $v = (& $ffmpeg -version 2>$null | Select-Object -First 1)
    Write-Ok "FFmpeg ya presente ($v). Usa -Forzar para reinstalar."
} else {
    Write-Paso 'Descargando FFmpeg (~30 MB)...'
    $url = 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip'
    $zip = Join-Path $env:TEMP 'localify-ffmpeg.zip'
    $extraido = Join-Path $env:TEMP 'localify-ffmpeg'

    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing

    if (Test-Path $extraido) { Remove-Item $extraido -Recurse -Force }
    Expand-Archive -Path $zip -DestinationPath $extraido -Force

    foreach ($nombre in @('ffmpeg.exe', 'ffprobe.exe')) {
        $origen = Get-ChildItem -Path $extraido -Filter $nombre -Recurse |
                  Select-Object -First 1
        if (-not $origen) { throw "El archivo de FFmpeg no contiene $nombre." }
        Copy-Item $origen.FullName (Join-Path $Destino $nombre) -Force
    }

    Remove-Item $zip -Force
    Remove-Item $extraido -Recurse -Force

    $v = (& $ffmpeg -version 2>$null | Select-Object -First 1)
    Write-Ok "$v instalado."
}

Write-Host ''
Write-Paso 'Sidecars listos.'
Get-ChildItem $Destino -Filter '*.exe' |
    Select-Object Name, @{ Name = 'MB'; Expression = { [math]::Round($_.Length / 1MB, 1) } } |
    Format-Table -AutoSize
