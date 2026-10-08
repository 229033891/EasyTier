@echo off
setlocal EnableExtensions
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" || exit /b 1

set "MSVC=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207"
set "KIT=C:\Program Files (x86)\Windows Kits\10"
set "INCLUDE=%MSVC%\include;%KIT%\Include\10.0.26100.0\ucrt"
set "LIB=%MSVC%\lib\x64;%KIT%\Lib\10.0.26100.0\ucrt\x64;%KIT%\Lib\10.0.26100.0\um\x64"
set "PATH=%MSVC%\bin\Hostx64\x64;C:\Users\Administrator\.cargo\bin;C:\Users\Administrator\.rustup\toolchains\1.95-x86_64-pc-windows-msvc\bin;C:\Windows\system32;C:\Windows"
set "CARGO_TARGET_DIR=C:\et"
set "CARGO_BUILD_JOBS=1"
set "TEMP=C:\et\tmp"
set "TMP=C:\et\tmp"
if not exist C:\et\tmp mkdir C:\et\tmp
set "PROTOC=C:\Users\Administrator\AppData\Local\Microsoft\WinGet\Packages\Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe\bin\protoc.exe"

set "VCINSTALLDIR=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\"
set "VSCMD_ARG_TGT_ARCH=x64"
set VSINSTALLDIR=
set WindowsSdkDir=
set WindowsSdkVerBinPath=
set WindowsLibPath=
set UniversalCRTSdkDir=
set UCRTVersion=
set WindowsSDKLibVersion=
set WindowsSDKVersion=
set WindowsSDK_ExecutablePath_x64=
set WindowsSDK_ExecutablePath_x86=
set FrameworkDir=
set FrameworkDir64=
set FrameworkVersion=
set FrameworkVersion64=
set NETFXSDKDir=
set VSCMD_VER=
set VSCMD_ARG_HOST_ARCH=
set VSCMD_ARG_app_plat=
set __VSCMD_PREINIT_PATH=
set __DOTNET_ADD_64BIT=
set __DOTNET_PREFERRED_BITNESS=
set DevEnvDir=
set ExtensionSdkDir=
set EXTERNAL_INCLUDE=
set FSHARPINSTALLDIR=
set HTMLHelpDir=
set IFCPATH=
set VCIDEInstallDir=
set VCToolsInstallDir=
set VCToolsRedistDir=
set VCToolsVersion=
set VS170COMNTOOLS=
set VSSDK150INSTALL=
set VSSDKINSTALL=

cd /d D:\EasyTier
if "%1"=="" (
  cargo check -p easytier-core
) else (
  cargo test -p easytier-core --lib %*
)
exit /b %ERRORLEVEL%
