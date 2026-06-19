!macro NSIS_HOOK_PREUNINSTALL
  IfFileExists "$INSTDIR\ThatIsOK.exe" 0 done
    ExecWait '"$INSTDIR\ThatIsOK.exe" --uninstall-hooks'
  done:
!macroend
