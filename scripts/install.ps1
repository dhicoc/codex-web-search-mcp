# codex-web-search-mcp — one-line installer (Windows / PowerShell)
#
# Downloads the correct prebuilt binary for your platform from GitHub Releases,
# verifies its SHA-256 against checksums.txt, installs it, and prints the MCP
# config snippet. No Rust / Node required.
#
# Usage (PowerShell):
#   irm https://raw.githubusercontent.com/dhicoc/codex-web-search-mcp/main/scripts/install.ps1 | iex
#   .\scripts\install.ps1 [-Version v2.1.0] [-InstallDir ~\codex-web-search-mcp] [-WriteConfig] [-Repo owner/name]
#
# Params:
#   -Version <tag>      Release tag to fetch (default: latest)
#   -InstallDir <dir>   Where to put the binary (default: ~/codex-web-search-mcp)
#   -WriteConfig        Write a .mcp.json (Claude Code project config) into the CWD if none exists
#   -Repo <owner/name>  Override the GitHub repo (default: dhicoc/codex-web-search-mcp)

param(
  [string]$Version = "",
  [string]$InstallDir = "",
  [switch]$WriteConfig,
  [string]$Repo = "dhicoc/codex-web-search-mcp"
)

$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

# ---- detect platform -------------------------------------------------------
$arch = $env:PROCESSOR_ARCHITECTURE
switch ($arch) {
  "AMD64" { $PLATFORM = "win32-x64" }
  "ARM64" { $PLATFORM = "win32-arm64" }
  default {
    if ($arch -eq "x86") { $PLATFORM = "win32-x64" }
    else { Write-Error "Unsupported architecture: $arch"; exit 1 }
  }
}
$EXE = ".exe"

# ---- resolve version -------------------------------------------------------
if (-not $Version) {
  Write-Host "Querying latest version..."
  $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" `
    -Headers @{ "User-Agent" = "install-script" }
  $Version = $rel.tag_name
  if (-not $Version) {
    Write-Error "Could not resolve latest version; pass -Version vX.Y.Z"
    exit 1
  }
}
Write-Host "Platform: $PLATFORM   Version: $Version"

# ---- install dir -----------------------------------------------------------
if (-not $InstallDir) { $InstallDir = Join-Path $HOME "codex-web-search-mcp" }
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

$ASSET = "codex-web-search-mcp-$PLATFORM$EXE"
$URL = "https://github.com/$Repo/releases/download/$Version/$ASSET"
$TMP = Join-Path $env:TEMP ("cwsmcp-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $TMP | Out-Null
try {
  Write-Host "Downloading $URL"
  Invoke-WebRequest -Uri $URL -OutFile (Join-Path $TMP $ASSET) -Headers @{ "User-Agent" = "install-script" }

  # ---- checksum verify (best-effort) --------------------------------------
  $cksUrl = "https://github.com/$Repo/releases/download/$Version/checksums.txt"
  $cksPath = Join-Path $TMP "checksums.txt"
  try {
    Invoke-WebRequest -Uri $cksUrl -OutFile $cksPath -Headers @{ "User-Agent" = "install-script" } -ErrorAction Stop
    $lines = Get-Content $cksPath
    $expected = ($lines | Where-Object { $_ -match [regex]::Escape($ASSET) + '$' }) -split '\s+' | Select-Object -First 1
    if ($expected) {
      $actual = (Get-FileHash -Algorithm SHA256 -Path (Join-Path $TMP $ASSET)).Hash.ToLower()
      if ($actual -eq $expected.ToLower()) {
        Write-Host "✓ SHA-256 verification passed"
      } else {
        Write-Error "✗ SHA-256 mismatch! expected $expected got $actual"
        exit 1
      }
    }
  } catch {
    Write-Host "(checksums.txt not found, skipping verification)"
  }

  # ---- install -------------------------------------------------------------
  $DEST = Join-Path $InstallDir ("codex-web-search-mcp" + $EXE)
  Copy-Item -Force (Join-Path $TMP $ASSET) $DEST
  Write-Host "✓ Installed to $DEST"
} finally {
  Remove-Item -Recurse -Force $TMP -ErrorAction SilentlyContinue
}

# ---- MCP config ------------------------------------------------------------
$DEST = Join-Path $InstallDir ("codex-web-search-mcp" + $EXE)
if ($WriteConfig -and -not (Test-Path (Join-Path (Get-Location) ".mcp.json"))) {
  $cfg = @{
    mcpServers = @{
      "codex-web-search" = @{ command = $DEST }
    }
  } | ConvertTo-Json -Depth 3
  Set-Content -Path (Join-Path (Get-Location) ".mcp.json") -Value $cfg
  Write-Host "✓ Wrote .mcp.json (current dir); restart Claude Code to apply"
}

Write-Host ""
Write-Host "MCP config snippet (add to your client's mcpServers):"
Write-Host '{'
Write-Host '  "mcpServers": {'
Write-Host '    "codex-web-search": {'
Write-Host "      `"command`": `"$DEST`""
Write-Host '    }'
Write-Host '  }'
Write-Host '}'
Write-Host ""
Write-Host "Tip: this tool needs a Codex login — run 'codex login' first (or set CODEX_ACCESS_TOKEN)."
