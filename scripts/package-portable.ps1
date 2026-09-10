param(
  [ValidateSet('x64', 'arm64')]
  [string]$Architecture = 'x64'
)

$ErrorActionPreference = 'Stop'
$version = (Get-Content -Raw -Encoding UTF8 (Join-Path $PSScriptRoot '..\package.json') | ConvertFrom-Json).version
$target = if ($Architecture -eq 'arm64') { 'aarch64-pc-windows-msvc' } else { 'x86_64-pc-windows-msvc' }
$binary = Join-Path $PSScriptRoot "..\src-tauri\target\$target\release\witch-clipboard.exe"
$output = Join-Path $PSScriptRoot "..\release\Witch-Clipboard-$version-$Architecture-portable.zip"
$stage = Join-Path ([IO.Path]::GetTempPath()) ("witch-clipboard-portable-" + [guid]::NewGuid())

if (-not (Test-Path -LiteralPath $binary)) {
  throw "Release binary not found: $binary"
}

try {
  New-Item -ItemType Directory -Path $stage -Force | Out-Null
  Copy-Item -LiteralPath $binary -Destination (Join-Path $stage 'Witch Clipboard.exe')
$instructions = @(
  "Witch Clipboard $version portable ($Architecture)"
  ""
  "Extract the archive and run Witch Clipboard.exe."
  "Requires Windows 10/11 and the WebView2 runtime."
  "No shortcuts are created; user data stays in %APPDATA%\\WitchCat-Clipboard."
) -join [Environment]::NewLine
  $instructions | Set-Content -LiteralPath (Join-Path $stage '使用说明.txt') -Encoding UTF8

  $outputDir = Split-Path -Parent $output
  New-Item -ItemType Directory -Path $outputDir -Force | Out-Null
  if (Test-Path -LiteralPath $output) {
    Remove-Item -LiteralPath $output -Force
  }
  Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $output -CompressionLevel Optimal
  Write-Output $output
}
finally {
  if (Test-Path -LiteralPath $stage) {
    Remove-Item -LiteralPath $stage -Recurse -Force
  }
}
