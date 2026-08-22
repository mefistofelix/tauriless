@echo off
setlocal
cd /d "%~dp0"

set "CARGO_PROFILE_ARGS="
if /I "%~1"=="release" (
    set "CARGO_PROFILE_ARGS=--release"
) else if not "%~1"=="" if /I not "%~1"=="debug" (
    echo Usage: %~nx0 [debug^|release] 1>&2
    exit /b 2
)

call :ensure_msvc
if errorlevel 1 exit /b 1

echo [tauriless] cargo build %CARGO_PROFILE_ARGS%
cargo build --manifest-path tauriless\Cargo.toml --locked %CARGO_PROFILE_ARGS%
exit /b %errorlevel%

:ensure_msvc
rem Reuse an x64 Visual C++ environment that is already active.
if /I "%VSCMD_ARG_TGT_ARCH%"=="x64" if defined VCToolsInstallDir (
    echo [tauriless] using active x64 MSVC environment
    exit /b 0
)

rem Reuse the project-local portable toolchain when it already exists.
if exist "msvc\vcvars64.bat" (
    echo [tauriless] using project-local MSVC environment
    call "msvc\vcvars64.bat" >nul
    if errorlevel 1 exit /b 1
    exit /b 0
)

rem Some configured runners expose vcvars64.bat directly on PATH.
set "VCVARS_ON_PATH="
for /f "delims=" %%I in ('where vcvars64.bat 2^>nul') do if not defined VCVARS_ON_PATH set "VCVARS_ON_PATH=%%I"
if defined VCVARS_ON_PATH (
    echo [tauriless] using vcvars64.bat from PATH
    call "%VCVARS_ON_PATH%" >nul
    if errorlevel 1 exit /b 1
    exit /b 0
)

rem GitHub Windows runners normally have Visual Studio installed even when its
rem developer environment is not active. Discover and enter the x64 vcvars.
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if exist "%VSWHERE%" (
    set "VSINSTALL="
    for /f "usebackq delims=" %%I in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSINSTALL=%%I"
    if defined VSINSTALL if exist "%VSINSTALL%\VC\Auxiliary\Build\vcvars64.bat" (
        echo [tauriless] using Visual Studio x64 environment
        call "%VSINSTALL%\VC\Auxiliary\Build\vcvars64.bat" >nul
        if errorlevel 1 exit /b 1
        exit /b 0
    )
)

rem Last resort for a clean machine: bootstrap the portable MSVC layout once.
if not exist "msvcup.exe" (
    echo [tauriless] downloading msvcup bootstrap
    curl.exe -fL "https://github.com/mefistofelix/msvcup/releases/latest/download/msvcup.exe" -o msvcup.exe
    if errorlevel 1 exit /b 1
)

echo [tauriless] bootstrapping project-local MSVC environment
msvcup.exe install "msvc sdk" msvc
if errorlevel 1 exit /b 1
call "msvc\vcvars64.bat" >nul
if errorlevel 1 exit /b 1
exit /b 0
