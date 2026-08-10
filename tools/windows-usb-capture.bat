@echo off
setlocal enabledelayedexpansion
title Logitech wheel USB capture

rem  Capture a Logitech wheel's USB traffic on Windows, so the commands a
rem  Windows driver sends can be decoded and reproduced on Linux.
rem
rem  Exists because the alternative is a page of Wireshark GUI instructions,
rem  and every step in it is a place to go wrong: not running elevated,
rem  picking the wrong USBPcap interface, or saving in a format that loses
rem  the USB layer. This asks for none of those decisions.
rem
rem  Right-click the file and choose "Run as administrator".

echo.
echo   Logitech wheel USB capture
echo   =========================
echo.

rem  USBPcap cannot attach without elevation, and the failure it gives
rem  otherwise is an empty capture rather than an error.
net session >nul 2>&1
if errorlevel 1 (
  echo   This has to run as administrator.
  echo.
  echo   Close this window, right-click the file, and choose
  echo   "Run as administrator".
  echo.
  pause
  exit /b 1
)

rem  dumpcap ships with Wireshark and is what actually records; using it
rem  directly avoids the GUI entirely.
rem  %ProgramFiles(x86)% is avoided deliberately: the parentheses in the
rem  variable's own name terminate a block early under cmd's parser, which
rem  is a bug that only appears on the machine you cannot test on.
set "DUMPCAP="
if exist "%ProgramFiles%\Wireshark\dumpcap.exe" set "DUMPCAP=%ProgramFiles%\Wireshark\dumpcap.exe"
if not defined DUMPCAP if exist "C:\Program Files\Wireshark\dumpcap.exe" set "DUMPCAP=C:\Program Files\Wireshark\dumpcap.exe"
if not defined DUMPCAP if exist "C:\Program Files (x86)\Wireshark\dumpcap.exe" set "DUMPCAP=C:\Program Files (x86)\Wireshark\dumpcap.exe"
if not defined DUMPCAP for /f "delims=" %%P in ('where dumpcap 2^>nul') do if not defined DUMPCAP set "DUMPCAP=%%P"
if not defined DUMPCAP (
  echo   Wireshark is not installed, or not in the usual place.
  echo.
  echo   Get it from https://www.wireshark.org/download.html
  echo   During setup, make sure USBPcap is ticked. Reboot afterwards.
  echo.
  pause
  exit /b 1
)

rem  Record every USB interface at once rather than asking which one the
rem  wheel is on. The extra traffic costs a few megabytes and removes the
rem  one question whose wrong answer produces a capture with no wheel in it.
set "ARGS="
set "COUNT=0"
rem  usebackq with backticks, so a path containing spaces needs no nested
rem  quoting. The doubled-quote form this replaced is the classic way to
rem  get "The system cannot find the file specified" from a path that is
rem  perfectly correct.
for /f "usebackq tokens=2" %%A in (`"%DUMPCAP%" -D 2^>nul ^| findstr /i USBPcap`) do (
  set "ARGS=!ARGS! -i %%A"
  set /a COUNT+=1
)

if "%COUNT%"=="0" (
  echo   No USB capture interfaces found, which means USBPcap is missing.
  echo.
  echo   Re-run the Wireshark installer and tick USBPcap, then reboot.
  echo.
  pause
  exit /b 1
)

echo   Found %COUNT% USB interface(s) to record.
echo.
echo   WHAT TO DO WHEN RECORDING STARTS
echo   --------------------------------
echo   Recording runs for 40 seconds. During it:
echo.
echo     1. Sit still, engine off or idling, for about 5 seconds.
echo     2. Rev the engine so the lights sweep up. Do it two or
echo        three times, letting them fall back down in between.
echo.
echo   Have a racing game already running before you start this.
echo   The rev lights follow engine RPM, so a game has to be feeding
echo   them; G HUB on its own will usually leave them dark.
echo.
echo   Slowly is better than quickly. The gaps between revs are what
echo   make the light commands stand out from everything else.
echo.
pause

set "OUT=%USERPROFILE%\Desktop\wheel-usb-capture.pcapng"
if exist "%OUT%" del /q "%OUT%"

echo.
echo   Recording for 40 seconds. Go.
echo.
"%DUMPCAP%" %ARGS% -a duration:40 -w "%OUT%" >nul 2>&1

if not exist "%OUT%" (
  echo   Something went wrong: no capture file was written.
  echo   Please say so on the issue and paste anything shown above.
  echo.
  pause
  exit /b 1
)

echo.
echo   Done. The capture is on your Desktop:
echo.
echo     %OUT%
echo.
echo   Zip it and attach it to the issue, and say in one line what you
echo   did to make the lights come on.
echo.
echo   Note: a USB capture includes the wheel's serial number. If you would
echo   rather not post that publicly, say so on the issue instead of
echo   attaching it, and we will find another way.
echo.
pause
