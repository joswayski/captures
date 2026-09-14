Unicode true
RequestExecutionLevel user
SetCompressor /SOLID lzma

!define APP_NAME "Captures Native Preview"
!define APP_ID "CapturesNativePreview"

Name "${APP_NAME}"
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\Captures Native Preview"
InstallDirRegKey HKCU "Software\Captures\Native Preview" "InstallLocation"

VIProductVersion "${PRODUCT_VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "${APP_NAME}"
VIAddVersionKey /LANG=1033 "FileDescription" "Captures experimental native Windows Preview installer"
VIAddVersionKey /LANG=1033 "FileVersion" "${PRODUCT_VERSION}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${PRODUCT_VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Captures contributors"

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "Install"
  SetShellVarContext current
  SetOutPath "$INSTDIR"
  File /oname=captures-windows-native.exe "${APP_BINARY}"
  File /oname=ffmpeg.exe "${FFMPEG_BINARY}"
  File /oname=ffprobe.exe "${FFPROBE_BINARY}"
  File "${REPO_ROOT}\LICENSE"
  SetOutPath "$INSTDIR\licenses\openh264"
  File "${REPO_ROOT}\apps\desktop\src-tauri\openh264\*"
  SetOutPath "$INSTDIR\licenses\ffmpeg"
  File "${FFMPEG_NOTICES}\NOTICE.md"
  File "${FFMPEG_NOTICES}\BUILD_CONFIG.txt"
  File "${FFMPEG_NOTICES}\COPYING.LGPLv2.1"
  File "${FFMPEG_DIST}\*"

  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\Captures\Native Preview" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "NoRepair" 1

  CreateDirectory "$SMPROGRAMS\Captures Native Preview"
  CreateShortcut "$SMPROGRAMS\Captures Native Preview\Captures Native Preview.lnk" "$INSTDIR\captures-windows-native.exe"
  CreateShortcut "$SMPROGRAMS\Captures Native Preview\Uninstall.lnk" "$INSTDIR\Uninstall.exe"
  CreateShortcut "$DESKTOP\Captures Native Preview.lnk" "$INSTDIR\captures-windows-native.exe"
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  Delete "$DESKTOP\Captures Native Preview.lnk"
  RMDir /r "$SMPROGRAMS\Captures Native Preview"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}"
  DeleteRegKey HKCU "Software\Captures\Native Preview"
  Delete "$INSTDIR\captures-windows-native.exe"
  Delete "$INSTDIR\ffmpeg.exe"
  Delete "$INSTDIR\ffprobe.exe"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir /r "$INSTDIR\licenses"
  RMDir "$INSTDIR"
  ; The profile under LocalAppData\Captures Windows Native Experiment is deliberately preserved.
SectionEnd
