param(
    [string]$VmUser = "azureuser",
    [string]$VmHost = "40.81.20.114",
    [string]$SshKey = "$HOME\.ssh\id_ed25519",
    # Where the server lives on the VM.
    [string]$VmPath = "/home/azureuser/legion-server",
    [int]$Port = 8080,
    # A domain pointed at the VM (for example game.example.com) turns on
    # HTTPS, which Google sign-in needs. Without one the server answers on
    # http://<VmHost>:<Port>.
    [string]$Domain = ""
)

$ErrorActionPreference = "Stop"

# Same flow as the portfolio's deploy.ps1: pack, upload over scp, then build
# and restart on the VM over ssh. The VM builds the server itself, because a
# build made on Windows can't run on Linux.
$Backend = $PSScriptRoot
$Root = Split-Path -Parent $Backend
$Archive = Join-Path $Backend "deploy.tar.gz"

Write-Host ""
Write-Host "=====================================" -ForegroundColor Cyan
Write-Host "  Legion of Minds server deploy" -ForegroundColor Cyan
Write-Host "  to $VmUser@$VmHost" -ForegroundColor Cyan
Write-Host "=====================================" -ForegroundColor Cyan
Write-Host ""

# ─── Step 1: Pack the source ─────────────────────────────────────
Write-Host "[1/4] Packing the server source..." -ForegroundColor Yellow
if (Test-Path $Archive) { Remove-Item -Force $Archive }
# The server needs backend/ and the game's battle code (game/crates/godcore,
# plus game/Cargo.toml, which godcore takes its version from).
# Windows 10/11 has native tar support.
tar -czf $Archive -C $Root `
    --exclude "backend/target" --exclude "backend/videos" `
    --exclude "backend/deploy.env" --exclude "backend/deploy.tar.gz" `
    --exclude "backend/.env" --exclude "*.token" `
    --exclude "game/crates/godcore/target" `
    backend game/Cargo.toml game/Cargo.lock game/crates/godcore
if ($LASTEXITCODE -ne 0) {
    Write-Host "Packing failed (tar exit $LASTEXITCODE)." -ForegroundColor Red
    exit 1
}
$SizeMb = [math]::Round((Get-Item $Archive).Length / 1MB, 1)
Write-Host "  Archive created ($SizeMb MB)." -ForegroundColor Green

# ─── Step 2: Upload ──────────────────────────────────────────────
Write-Host "[2/4] Uploading to the VM..." -ForegroundColor Yellow
scp -i $SshKey $Archive "${VmUser}@${VmHost}:/tmp/legion-deploy.tar.gz"
if ($LASTEXITCODE -ne 0) {
    Write-Host "Upload failed (scp exit $LASTEXITCODE). Aborting." -ForegroundColor Red
    Remove-Item -Force $Archive
    exit 1
}
scp -i $SshKey (Join-Path $Backend "deploy\server.sh") "${VmUser}@${VmHost}:/tmp/legion-server.sh"
if ($LASTEXITCODE -ne 0) {
    Write-Host "Upload failed (scp exit $LASTEXITCODE). Aborting." -ForegroundColor Red
    Remove-Item -Force $Archive
    exit 1
}
Write-Host "  Upload complete." -ForegroundColor Green

# Server secrets (OAuth keys and so on), merged into the VM's env file:
# only the keys in deploy.env are replaced or added.
$DeployEnv = Join-Path $Backend "deploy.env"
if (Test-Path $DeployEnv) {
    scp -i $SshKey $DeployEnv "${VmUser}@${VmHost}:/tmp/legion-deploy.env"
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Secrets upload failed (scp exit $LASTEXITCODE). Aborting." -ForegroundColor Red
        Remove-Item -Force $Archive
        exit 1
    }
    Write-Host "  Server secrets uploaded (deploy.env)." -ForegroundColor Green
} else {
    Write-Host "  No deploy.env found - skipping server secrets (see deploy.env.example)." -ForegroundColor DarkYellow
}

# ─── Step 3: Build and restart on the VM ─────────────────────────
Write-Host "[3/4] Building and restarting on the VM (the first deploy takes several minutes)..." -ForegroundColor Yellow
# tr strips Windows line endings in case git checked server.sh out with them.
$RemoteCommand = "tr -d '\r' < /tmp/legion-server.sh > /tmp/legion-server.run.sh && bash /tmp/legion-server.run.sh '$VmPath' '$Port' '$VmHost' '$Domain'; s=`$?; rm -f /tmp/legion-server.sh /tmp/legion-server.run.sh; exit `$s"
ssh -t -i $SshKey "${VmUser}@${VmHost}" $RemoteCommand
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "Build or restart on the VM failed (ssh exit $LASTEXITCODE)." -ForegroundColor Red
    Write-Host "Re-run .\deploy.ps1 to retry." -ForegroundColor Yellow
    Remove-Item -Force $Archive
    exit 1
}
Remove-Item -Force $Archive

# ─── Step 4: Check from here ─────────────────────────────────────
Write-Host "[4/4] Checking the server from this computer..." -ForegroundColor Yellow
$Url = if ($Domain) { "https://$Domain" } else { "http://${VmHost}:$Port" }
try {
    Invoke-WebRequest -UseBasicParsing -TimeoutSec 15 "$Url/health" | Out-Null
    Write-Host "  $Url answers." -ForegroundColor Green
} catch {
    Write-Host "  The server runs on the VM, but $Url doesn't answer from here." -ForegroundColor DarkYellow
    if ($Domain) {
        Write-Host "  Check that $Domain points at $VmHost and that ports 80 and 443 are open in the VM's Azure network settings." -ForegroundColor DarkYellow
    } else {
        Write-Host "  Open port $Port (TCP, inbound) in the VM's Azure network settings." -ForegroundColor DarkYellow
    }
}

Write-Host ""
Write-Host "=====================================" -ForegroundColor Green
Write-Host "  Deploy complete!" -ForegroundColor Green
Write-Host "  Build the game for this server (in the game folder):" -ForegroundColor Green
Write-Host "    `$env:BAG_SERVER=`"$Url`"; cargo build --release" -ForegroundColor Green
Write-Host "=====================================" -ForegroundColor Green
