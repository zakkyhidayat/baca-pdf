# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds the program and packs the portable zip: target\portable\Baca-PDF-<version>-portable-x64.zip.
# The zip holds the program, pdfium.dll, the licenses, a data folder (which makes it portable) and
# Install.cmd, all at the top level.

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$name = "Baca-PDF-$version-portable-x64"
$out = "$root\target\portable"
$stage = "$out\$name"

Push-Location $root
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}
finally { Pop-Location }

if (Test-Path $stage) { Remove-Item $stage -Recurse -Confirm:$false }
New-Item -ItemType Directory -Force "$stage\data" | Out-Null
Copy-Item "$root\target\release\baca-pdf.exe" "$stage\Baca PDF.exe"
Copy-Item "$root\pdfium\bin\pdfium.dll" $stage
Copy-Item "$root\LICENSE" $stage
Copy-Item "$root\pdfium\licenses" "$stage\pdfium-licenses" -Recurse
Copy-Item "$PSScriptRoot\Install.cmd" $stage
Copy-Item "$PSScriptRoot\data\README.txt" "$stage\data"

# The files sit at the top of the zip, with no folder around them: Explorer's Extract All already makes
# a folder named after the zip. The entries are written one by one because both Compress-Archive and
# CreateFromDirectory in Windows PowerShell 5.1 store "\" instead of the standard "/" in the paths.
$zip = "$out\$name.zip"
if (Test-Path $zip) { Remove-Item $zip -Confirm:$false }
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$archive = [IO.Compression.ZipFile]::Open($zip, [IO.Compression.ZipArchiveMode]::Create)
try {
    Get-ChildItem $stage -Recurse -File | ForEach-Object {
        $entry = $_.FullName.Substring($stage.Length + 1).Replace('\', '/')
        [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $_.FullName, $entry, [IO.Compression.CompressionLevel]::Optimal)
    }
}
finally { $archive.Dispose() }
Write-Host "Wrote $zip"
