# Builds the Windows release of Legion of Minds:
#   dist\LegionOfMinds-<version>-windows.zip   (unzip and run)
#   dist\LegionOfMinds-<version>-setup.exe     (installer; needs Inno Setup 6)
#
#   cd game\installer
#   .\build-installer.ps1 -Server https://play.example.com
#
# -Server is the online server the game talks to (baked into the exe; players
# can still override it with the BAG_SERVER environment variable).
param(
    [string]$Server = $env:BAG_SERVER,
    [string]$Version
)
$ErrorActionPreference = "Stop"
$game = Split-Path $PSScriptRoot -Parent
Set-Location $game

if (-not $Version) {
    $Version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
}
if ($Server) {
    $env:BAG_SERVER = $Server
    Write-Host "Online server: $Server"
} else {
    Write-Warning "No -Server given: this build looks for a server on localhost:8080."
}

cargo build --release -p legion-of-minds
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

# Everything the game needs, laid out as it is installed: the exe with the
# assets folder next to it.
$dist = Join-Path $game "dist"
$stage = Join-Path $dist "stage"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\legion-of-minds.exe $stage
Copy-Item assets $stage -Recurse
Copy-Item CREDITS.md $stage

$zip = Join-Path $dist "LegionOfMinds-$Version-windows.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip
Write-Host "Zip:       $zip"

$iscc = @(
    (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source,
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if ($iscc) {
    & $iscc "/DAppVersion=$Version" "/DStage=$stage" (Join-Path $PSScriptRoot "legion-of-minds.iss")
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed" }
    Write-Host "Installer: $(Join-Path $dist "LegionOfMinds-$Version-setup.exe")"
} else {
    Write-Warning "Inno Setup 6 not found (https://jrsoftware.org/isdl.php): made the zip only."
}
