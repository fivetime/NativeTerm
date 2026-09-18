@echo off
rem Builds ntplink.exe, NativeTerm's client for non-SSH sessions, from a
rem PuTTY source tree carrying NativeTerm's additions (its nativeterm\
rem folder; see ARCHITECTURE.md, "ntplink"), and puts it next to the shim
rem in target\debug and target\release (the ones that exist).
rem
rem   tools\build-ntplink.cmd [putty-source-dir]
rem
rem The source defaults to ..\putty-win-src beside this repository. Needs
rem CMake and Visual Studio's C++ tools; the static C runtime makes the
rem program self-contained.
setlocal
set REPO=%~dp0..
set SRC=%~1
if "%SRC%"=="" set SRC=%REPO%\..\putty-win-src
if not exist "%SRC%\nativeterm\ntplink.c" (echo %SRC% is not a PuTTY source tree with nativeterm\ntplink.c & exit /b 1)

set VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe
for /f "usebackq delims=" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set VS=%%i
if not exist "%VS%\VC\Auxiliary\Build\vcvars64.bat" (echo Visual Studio's C++ tools weren't found & exit /b 1)
call "%VS%\VC\Auxiliary\Build\vcvars64.bat" >nul

set BUILD=%REPO%\target\ntplink-build
cmake -S "%SRC%" -B "%BUILD%" -G "NMake Makefiles" -DCMAKE_BUILD_TYPE=Release ^
    -DCMAKE_POLICY_DEFAULT_CMP0091=NEW -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded > "%BUILD%-configure.log" 2>&1
if errorlevel 1 (type "%BUILD%-configure.log" & exit /b 1)
cmake --build "%BUILD%" --target ntplink
if errorlevel 1 exit /b 1

for %%d in (debug release) do (
    if exist "%REPO%\target\%%d" (
        copy /y "%BUILD%\nativeterm\ntplink.exe" "%REPO%\target\%%d\" >nul && echo ntplink.exe -^> target\%%d
    )
)
