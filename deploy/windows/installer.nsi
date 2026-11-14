; Instalador de Localify para Windows (NSIS). Lo genera tools/package-windows.sh:
;   makensis -DSRC=<carpeta con los ficheros> -DOUT=<instalador.exe> -DVERSION=x.y.z installer.nsi
; Instala la app y sus dependencias (ffmpeg, ffprobe, yt-dlp) dentro de su carpeta, y el runtime de
; WebView2 si el sistema no lo tiene. Admite instalación silenciosa: installer.exe /S
Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"

!ifndef SRC
  !error "pasa -DSRC=<carpeta con localify.exe, ffmpeg.exe, ffprobe.exe, yt-dlp.exe, ...>"
!endif
!ifndef OUT
  !define OUT "installer.exe"
!endif
!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!define NAME "Localify"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${NAME}"
!define WV2_KEY "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

Name "${NAME}"
OutFile "${OUT}"
InstallDir "$PROGRAMFILES64\${NAME}"
InstallDirRegKey HKLM "Software\${NAME}" "InstallDir"
RequestExecutionLevel admin
SetCompressor /SOLID lzma
SetCompressorDictSize 64
Icon "${SRC}\localify.ico"
UninstallIcon "${SRC}\localify.ico"
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${NAME}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "Instalador de ${NAME}"

!define MUI_ICON "${SRC}\localify.ico"
!define MUI_UNICON "${SRC}\localify.ico"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\localify.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Abrir Localify"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "Spanish"

Section "Localify"
  ; Instalación para todos los usuarios y registro de 64 bits (el instalador es de 32 bits).
  SetShellVarContext all
  SetRegView 64
  SetOutPath "$INSTDIR"
  ; Cierra una instancia abierta antes de sobrescribir (si no, File falla con el .exe en uso).
  nsExec::Exec 'taskkill /F /IM localify.exe'
  File "${SRC}\localify.exe"
  File "${SRC}\libwinpthread-1.dll"
  File "${SRC}\localify.ico"
  File "${SRC}\LICENSE.txt"
  ; Dependencias dentro de su carpeta: Windows busca los ejecutables hijos junto al de la app.
  File "${SRC}\ffmpeg.exe"
  File "${SRC}\ffprobe.exe"
  File "${SRC}\yt-dlp.exe"
  ; yt-dlp se actualiza solo (Ajustes > Actualizar): que un usuario sin permisos de administrador pueda reescribirlo.
  nsExec::Exec 'icacls "$INSTDIR\yt-dlp.exe" /grant *S-1-5-32-545:M'
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateDirectory "$SMPROGRAMS\${NAME}"
  CreateShortcut "$SMPROGRAMS\${NAME}\${NAME}.lnk" "$INSTDIR\localify.exe" "" "$INSTDIR\localify.ico"
  CreateShortcut "$SMPROGRAMS\${NAME}\Desinstalar ${NAME}.lnk" "$INSTDIR\Uninstall.exe"
  CreateShortcut "$DESKTOP\${NAME}.lnk" "$INSTDIR\localify.exe" "" "$INSTDIR\localify.ico"

  WriteRegStr HKLM "Software\${NAME}" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayName" "${NAME}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\localify.ico"
  WriteRegStr HKLM "${UNINST_KEY}" "Publisher" "Eldur4023"
  WriteRegStr HKLM "${UNINST_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1

  ; La ventana usa WebView2 (viene con Windows 11 y con Windows 10 actualizado): si falta, se instala.
  ReadRegStr $0 HKLM "${WV2_KEY}" "pv"
  ${If} $0 == ""
    ReadRegStr $0 HKCU "Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  ${EndIf}
  ${If} $0 == ""
  ${OrIf} $0 == "0.0.0.0"
    DetailPrint "Instalando el runtime de WebView2..."
    SetOutPath "$PLUGINSDIR"
    File "/oname=$PLUGINSDIR\MicrosoftEdgeWebview2Setup.exe" "${SRC}\MicrosoftEdgeWebview2Setup.exe"
    ; El instalador oficial descarga el runtime: necesita conexión a internet.
    ExecWait '"$PLUGINSDIR\MicrosoftEdgeWebview2Setup.exe" /silent /install' $1
    ${If} $1 != 0
      MessageBox MB_OK|MB_ICONEXCLAMATION "No se pudo instalar el runtime de WebView2 (código $1).$\nLocalify lo necesita para mostrar su ventana: instálalo desde https://developer.microsoft.com/microsoft-edge/webview2/"
    ${EndIf}
  ${EndIf}
SectionEnd

Section "Uninstall"
  SetShellVarContext all
  SetRegView 64
  nsExec::Exec 'taskkill /F /IM localify.exe'
  Delete "$INSTDIR\localify.exe"
  Delete "$INSTDIR\libwinpthread-1.dll"
  Delete "$INSTDIR\localify.ico"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\ffmpeg.exe"
  Delete "$INSTDIR\ffprobe.exe"
  Delete "$INSTDIR\yt-dlp.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${NAME}\${NAME}.lnk"
  Delete "$SMPROGRAMS\${NAME}\Desinstalar ${NAME}.lnk"
  RMDir "$SMPROGRAMS\${NAME}"
  Delete "$DESKTOP\${NAME}.lnk"
  DeleteRegKey HKLM "${UNINST_KEY}"
  DeleteRegKey HKLM "Software\${NAME}"
  ; Los datos del usuario (biblioteca, base de datos, ajustes) no se tocan.
SectionEnd
