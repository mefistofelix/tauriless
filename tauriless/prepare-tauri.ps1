param()

$ErrorActionPreference = "Stop"

$crateRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$patchRoot = Join-Path $crateRoot "patches\tauri-2.11.5"
$vendorRoot = Join-Path $crateRoot "vendor\tauri"
$originalMod = Join-Path $patchRoot "webview-mod.original.rs"
$patchedMod = Join-Path $patchRoot "webview-mod.patched.rs"
$headless = Join-Path $patchRoot "headless.rs"
$modPatch = Join-Path $patchRoot "webview-mod.patch"
$vendorMod = Join-Path $vendorRoot "src\webview\mod.rs"
$vendorHeadless = Join-Path $vendorRoot "src\webview\headless.rs"

function Test-SameFile([string]$Left, [string]$Right) {
  if (-not (Test-Path -LiteralPath $Left) -or -not (Test-Path -LiteralPath $Right)) {
    return $false
  }

  (Get-FileHash -Algorithm SHA256 -LiteralPath $Left).Hash -eq
    (Get-FileHash -Algorithm SHA256 -LiteralPath $Right).Hash
}

if ((Test-SameFile $vendorMod $patchedMod) -and
    (Test-SameFile $vendorHeadless $headless)) {
  Write-Host "Tauri 2.11.5 working copy is already patched."
  exit 0
}

if (-not (Test-Path -LiteralPath (Join-Path $vendorRoot "Cargo.toml"))) {
  $cargoRoot = if ($env:CARGO_HOME) {
    $env:CARGO_HOME
  } else {
    Join-Path $env:USERPROFILE ".cargo"
  }
  $registryPattern = Join-Path $cargoRoot "registry\src\*\tauri-2.11.5"
  $registrySource = Get-ChildItem -Path $registryPattern -Directory -ErrorAction SilentlyContinue |
    Select-Object -First 1

  if (-not $registrySource) {
    & cargo info "tauri@2.11.5" | Out-Null
    if ($LASTEXITCODE -ne 0) {
      throw "cargo could not fetch tauri 2.11.5"
    }
    $registrySource = Get-ChildItem -Path $registryPattern -Directory |
      Select-Object -First 1
  }

  if (-not $registrySource) {
    throw "tauri 2.11.5 was not found in the Cargo registry"
  }

  New-Item -ItemType Directory -Path (Split-Path -Parent $vendorRoot) -Force | Out-Null
  Copy-Item -LiteralPath $registrySource.FullName -Destination $vendorRoot -Recurse
}

if (-not (Test-SameFile $vendorMod $originalMod)) {
  throw "vendor/tauri is neither pristine Tauri 2.11.5 nor the expected patched copy"
}

Push-Location $vendorRoot
try {
  & git -c core.autocrlf=false apply --unidiff-zero --check $modPatch
  if ($LASTEXITCODE -ne 0) {
    throw "the Tauriless mod.rs patch does not apply cleanly"
  }
  & git -c core.autocrlf=false apply --unidiff-zero $modPatch
  if ($LASTEXITCODE -ne 0) {
    throw "the Tauriless mod.rs patch failed"
  }
} finally {
  Pop-Location
}

Copy-Item -LiteralPath $headless -Destination $vendorHeadless

if (-not (Test-SameFile $vendorMod $patchedMod) -or
    -not (Test-SameFile $vendorHeadless $headless)) {
  throw "the prepared Tauri source does not match the committed patch artifacts"
}

Write-Host "Prepared ignored vendor/tauri working copy from the committed patch kit."
