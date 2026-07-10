!macro NSIS_HOOK_PREUNINSTALL
  IfFileExists "$INSTDIR\AgentIsOK.exe" 0 done
    ExecWait '"$INSTDIR\AgentIsOK.exe" --uninstall-hooks'
  done:
!macroend
