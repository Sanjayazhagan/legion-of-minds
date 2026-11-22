param(
    [string]$VmUser = "azureuser",
    [string]$VmHost = "40.81.20.114",
    [string]$SshKey = "$HOME\.ssh\id_ed25519",
    # Where the server lives on the VM. Kept apart from the portfolio
    # (/home/azureuser/projects/portfolio-site), which this never touches.
    [string]$VmPath = "/home/azureuser/legion-server",
    [int]$Port = 8080,
    # A domain pointed at the VM (for example game.example.com) turns on
    # HTTPS, which Google sign-in needs. Without one the server answers on
    # http://<VmHost>:<Port>.
    [string]$Domain = ""
)

$ErrorActionPreference = "Stop"

# Same flow as the portfolio's deploy.ps1: pack a tarball, upload it over scp
# with keepalives, then run the VM side as one base64 script over ssh. The VM
# builds the server itself, because a build made on Windows can't run on Linux.
$Backend = $PSScriptRoot
$Root = Split-Path -Parent $Backend
$Archive = Join-Path $Backend "deploy.tar.gz"

# Keepalive options so slow transfers aren't killed by an idle/NAT timeout
# (same as the portfolio). Applied to every scp/ssh call below.
$SshOpts = @(
    "-o", "ServerAliveInterval=15",
    "-o", "ServerAliveCountMax=8",
    "-o", "TCPKeepAlive=yes"
)

# These go into the VM script as arguments, so only allow plain values.
if ($VmPath -notmatch '^/[A-Za-z0-9._/-]+$') {
    Write-Host "VmPath must be an absolute Linux path (letters, digits, . _ - /)." -ForegroundColor Red
    exit 1
}
if ($Domain -and $Domain -notmatch '^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$') {
    Write-Host "Domain must be a plain host name like game.example.com (no https://, no slash)." -ForegroundColor Red
    exit 1
}
if ($Port -lt 1024 -or $Port -gt 65535) {
    Write-Host "Port must be between 1024 and 65535." -ForegroundColor Red
    exit 1
}
if (-not (Test-Path $SshKey)) {
    Write-Host "SSH key not found: $SshKey" -ForegroundColor Red
    Write-Host "Use the same key as the portfolio deploy, for example: .\deploy.ps1 -SshKey C:\path\to\key.pem" -ForegroundColor Yellow
    exit 1
}

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
scp -i $SshKey @SshOpts $Archive "${VmUser}@${VmHost}:/tmp/legion-deploy.tar.gz"
if ($LASTEXITCODE -ne 0) {
    Write-Host "Upload failed (scp exit $LASTEXITCODE). Re-run .\deploy.ps1 to retry." -ForegroundColor Red
    Remove-Item -Force $Archive
    exit 1
}
Write-Host "  Upload complete." -ForegroundColor Green

# Server secrets (OAuth keys, ADMIN_KEY), merged into the VM's env file:
# only the keys in deploy.env are replaced or added.
$DeployEnv = Join-Path $Backend "deploy.env"
if (Test-Path $DeployEnv) {
    scp -i $SshKey @SshOpts $DeployEnv "${VmUser}@${VmHost}:/tmp/legion-deploy.env"
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Secrets upload failed (scp exit $LASTEXITCODE). Re-run .\deploy.ps1 to retry." -ForegroundColor Red
        Remove-Item -Force $Archive
        exit 1
    }
    Write-Host "  Server secrets uploaded (deploy.env)." -ForegroundColor Green
} else {
    Write-Host "  No deploy.env found - skipping server secrets (see deploy.env.example)." -ForegroundColor DarkYellow
}

# ─── Step 3: Back up, build and restart on the VM ────────────────
Write-Host "[3/4] Backing up the database, building and restarting on the VM (the first deploy takes several minutes)..." -ForegroundColor Yellow
# The VM side is deploy/server.sh with its arguments set at the top. Like the
# portfolio, it travels as one base64 token, so Windows can't mangle its
# quotes, and with LF line endings, so bash doesn't choke on `\r`.
$ServerScript = Get-Content -Raw -Encoding UTF8 (Join-Path $Backend "deploy\server.sh")
$RemoteScript = "set -- '$VmPath' '$Port' '$VmHost' '$Domain'`n" + $ServerScript
$RemoteScript = $RemoteScript -replace "`r`n", "`n" -replace "`r", ""
$B64 = [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($RemoteScript))
# Decoded to a file and run from there (not piped into bash), so sudo and apt
# can't eat the script from stdin. -t lets sudo ask for a password if it must.
ssh -t -i $SshKey @SshOpts "${VmUser}@${VmHost}" "echo $B64 | base64 -d > /tmp/legion-server.run.sh && bash /tmp/legion-server.run.sh; s=`$?; rm -f /tmp/legion-server.run.sh; exit `$s"
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "Build or restart on the VM failed (ssh exit $LASTEXITCODE)." -ForegroundColor Red
    Write-Host "If the build failed, the server that was running before keeps running." -ForegroundColor Yellow
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
Write-Host "  Admin page: $Url/admin" -ForegroundColor Green
Write-Host "  Set the GitHub repo variable BAG_SERVER to $Url" -ForegroundColor Green
Write-Host "  so new releases connect to this server." -ForegroundColor Green
Write-Host "=====================================" -ForegroundColor Green
