@echo off
rem SPDX-License-Identifier: GPL-3.0-or-later
rem
rem Moves Baca PDF out of the unzipped folder into %LOCALAPPDATA%\Programs\Baca PDF, adds a shortcut to
rem the Start Menu and deletes itself. It writes nothing to the registry and needs no administrator
rem rights. Running it again from a newer zip updates the program and keeps the data folder.
setlocal
title Install Baca PDF
cd /d "%~dp0"

set "SELF=%~f0"
set "BACA_DEST=%LOCALAPPDATA%\Programs\Baca PDF"
set "BACA_EXE=%BACA_DEST%\Baca PDF.exe"
set "BACA_LNK=%APPDATA%\Microsoft\Windows\Start Menu\Programs\Baca PDF.lnk"

if not exist "Baca PDF.exe" goto missing
if not exist "pdfium.dll" goto missing
if /i "%CD%"=="%BACA_DEST%" goto already
tasklist /fi "imagename eq Baca PDF.exe" /nh 2>nul | find /i "Baca PDF.exe" >nul
if not errorlevel 1 goto running

echo Installing Baca PDF to:
echo   %BACA_DEST%
echo.

robocopy "%~dp0." "%BACA_DEST%" "Baca PDF.exe" pdfium.dll LICENSE /njh /njs /ndl /np >nul
if errorlevel 8 goto copyfail
if exist "%~dp0pdfium-licenses" (
    robocopy "%~dp0pdfium-licenses" "%BACA_DEST%\pdfium-licenses" /e /njh /njs /ndl /np >nul
    if errorlevel 8 goto copyfail
)

rem Settings are never overwritten: a data folder that is already there stays as it is.
set "KEEPDATA="
if exist "%BACA_DEST%\data" set "KEEPDATA=1"
if not defined KEEPDATA if exist "%~dp0data" (
    robocopy "%~dp0data" "%BACA_DEST%\data" /e /njh /njs /ndl /np >nul
    if errorlevel 8 goto copyfail
)
if not exist "%BACA_EXE%" goto copyfail
if not exist "%BACA_DEST%\pdfium.dll" goto copyfail

rem Everything arrived, so the originals can go.
del /q "Baca PDF.exe" "pdfium.dll" "LICENSE" 2>nul
rmdir /s /q "pdfium-licenses" 2>nul
if not defined KEEPDATA rmdir /s /q "data" 2>nul

powershell -NoProfile -ExecutionPolicy Bypass -Command "$t=[IO.File]::ReadAllText($env:SELF); iex $t.Substring($t.IndexOf([string][char]10+'#PS-BEGIN'))"
if errorlevel 1 (
    echo The Start Menu shortcut could not be created. Start the program from:
    echo   %BACA_EXE%
) else (
    echo A shortcut named Baca PDF is now in the Start Menu.
)
echo.
echo Baca PDF is installed. You can delete this folder now:
echo   %~dp0
if defined KEEPDATA (
    echo Its data folder was left alone, because Baca PDF already has settings in %BACA_DEST%\data.
)
echo.
pause
(goto) 2>nul & del "%SELF%"

:missing
echo Baca PDF.exe and pdfium.dll must be in the same folder as this file.
echo Unzip the whole zip first, then run Install.cmd from the unzipped folder.
echo.
pause
exit /b 1

:already
echo Baca PDF is already running from this folder, which is where it installs to.
echo.
pause
exit /b 1

:running
echo Baca PDF is open. Close it, then run Install.cmd again.
echo.
pause
exit /b 1

:copyfail
echo Some files could not be copied to %BACA_DEST%.
echo Nothing was removed from this folder. Close Baca PDF if it is open, then try again.
echo.
pause
exit /b 1

#PS-BEGIN
$ErrorActionPreference = 'Stop'

# Puts the program's AppUserModelID on the shortcut, so a button pinned from it and the open window
# share one place on the taskbar.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class ShortcutId {
    [StructLayout(LayoutKind.Sequential)]
    struct Key { public Guid fmtid; public uint pid; }

    [StructLayout(LayoutKind.Explicit, Size = 24)]
    struct Value { [FieldOffset(0)] public ushort vt; [FieldOffset(8)] public IntPtr p; }

    [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IPropertyStore {
        void GetCount(out uint count);
        void GetAt(uint index, out Key key);
        void GetValue(ref Key key, out Value value);
        void SetValue(ref Key key, ref Value value);
        void Commit();
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    static extern int SHGetPropertyStoreFromParsingName(string path, IntPtr bind, int flags, ref Guid iid, out IPropertyStore store);

    public static void Set(string path, string id) {
        Guid iid = new Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99");
        IPropertyStore store;
        Marshal.ThrowExceptionForHR(SHGetPropertyStoreFromParsingName(path, IntPtr.Zero, 2, ref iid, out store));
        Key key = new Key { fmtid = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), pid = 5 };
        Value value = new Value { vt = 31, p = Marshal.StringToCoTaskMemUni(id) };
        store.SetValue(ref key, ref value);
        store.Commit();
        Marshal.FreeCoTaskMem(value.p);
        Marshal.ReleaseComObject(store);
    }
}
'@

$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut($env:BACA_LNK)
$link.TargetPath = $env:BACA_EXE
$link.WorkingDirectory = $env:BACA_DEST
$link.Description = 'Baca PDF'
$link.Save()
[ShortcutId]::Set($env:BACA_LNK, 'BacaPDF.Reader')
