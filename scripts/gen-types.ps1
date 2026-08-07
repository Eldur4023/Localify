<#
.SYNOPSIS
    Regenera los tipos de TypeScript desde los DTOs de Rust.

.DESCRIPTION
    `ts-rs` exporta durante `cargo test`. El destino lo fija `.cargo/config.toml`
    (`TS_RS_EXPORT_DIR`), asi que basta con ejecutar los tests del crate de la
    aplicacion.

    Rust es la unica fuente de verdad de lo que cruza el puente IPC (ADR-014):
    este fichero no se edita a mano.

.PARAMETER Verificar
    No escribe: comprueba que el fichero generado coincide con el versionado.
    Es lo que ejecuta la CI para impedir que backend y frontend se
    desincronicen.

.EXAMPLE
    .\scripts\gen-types.ps1
    .\scripts\gen-types.ps1 -Verificar
#>
[CmdletBinding()]
param(
    [switch] $Verificar
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$raiz = Split-Path -Parent $PSScriptRoot
$destino = Join-Path $raiz 'frontend\src\ipc\types.gen.ts'

if ($Verificar) {
    if (-not (Test-Path $destino)) {
        Write-Host "::error::no existe $destino"
        exit 1
    }
    $antes = Get-FileHash $destino -Algorithm SHA256
}

Write-Host '==> Generando tipos desde los DTOs de Rust...' -ForegroundColor Cyan
Push-Location $raiz
try {
    # `ErrorActionPreference = 'Stop'` convierte cualquier linea de stderr de un
    # ejecutable nativo en un error terminante, y cargo escribe avisos ahi de
    # forma rutinaria. Lo que importa es el codigo de salida.
    $anterior = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    cargo test -p localify-app --quiet
    $codigo = $LASTEXITCODE
    $ErrorActionPreference = $anterior

    if ($codigo -ne 0) { throw "los tests de localify-app fallaron (codigo $codigo)" }
} finally {
    Pop-Location
}

if (-not (Test-Path $destino)) {
    Write-Host "::error::ts-rs no genero $destino"
    exit 1
}

$tipos = ([regex]::Matches(
    [System.IO.File]::ReadAllText($destino), 'export type (\w+)'
)).Count
Write-Host "    $tipos tipos en $destino" -ForegroundColor Green

if ($Verificar) {
    $despues = Get-FileHash $destino -Algorithm SHA256
    if ($antes.Hash -ne $despues.Hash) {
        Write-Host '::error::los tipos generados no coinciden con los versionados.'
        Write-Host 'Ejecuta .\scripts\gen-types.ps1 y confirma el resultado.'
        exit 1
    }
    Write-Host '    Los tipos estan al dia.' -ForegroundColor Green
}
