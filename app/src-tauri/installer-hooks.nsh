; Tauri NSIS installer hooks: install the VST3 bundle to the system-wide VST3
; folder (C:\Program Files\Common Files\VST3\) and remove it on uninstall.
; The bundle is shipped as a Tauri resource (see tauri.vst3.conf.json), which
; lands in $INSTDIR\resources\SetupCoachVoice.vst3. If it was not packed (local
; builds without a VST3), the copy is skipped.

!macro NSIS_HOOK_POSTINSTALL
  SetRegView 64
  IfFileExists "$INSTDIR\resources\SetupCoachVoice.vst3\*.*" 0 sc_vst3_skip
    RMDir /r "$COMMONFILES64\VST3\SetupCoachVoice.vst3"
    CreateDirectory "$COMMONFILES64\VST3"
    CreateDirectory "$COMMONFILES64\VST3\SetupCoachVoice.vst3"
    CopyFiles /SILENT "$INSTDIR\resources\SetupCoachVoice.vst3\*.*" "$COMMONFILES64\VST3\SetupCoachVoice.vst3"
    RMDir /r "$INSTDIR\resources\SetupCoachVoice.vst3"
  sc_vst3_skip:
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  SetRegView 64
  RMDir /r "$COMMONFILES64\VST3\SetupCoachVoice.vst3"
!macroend
