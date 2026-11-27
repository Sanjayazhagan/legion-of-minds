param(
    # Which cloud the VM is in. Sets the defaults below and the firewall
    # hints: "azure" (the portfolio's VM) or "oracle" (Oracle Cloud Always
    # Free, Ubuntu image: user "ubuntu", and -VmHost is required).
    [ValidateSet("azure", "oracle")]
    [string]$Cloud = "azure",
    [string]$VmUser = "",
    [string]$VmHost = "",
    [string]$SshKey = "",
    # Where the server lives on the VM. Kept apart from the portfolio
    # (/home/azureuser/projects/portfolio-site), which this never touches.
    [string]$VmPath = "",
    [int]$Port = 8080,
    # A domain pointed at the VM (for example game.example.com) turns on
    # HTTPS, which Google sign-in needs. Without one the server answers on
    # http://<VmHost>:<Port>.
    [string]$Domain = "",
    # Deploy without deploy.env (no sign-in keys, no admin page). Only for a
    # quick test: players can't sign in with Google or Discord without it.
    [switch]$NoSecrets,
    # The server is normally built by GitHub (Actions > "Server build") and
    # downloaded with the GitHub CLI (gh), so the VM never compiles.
    # -BuildOnVm skips that and builds on the VM like before (slow, needs swap).
    [switch]$BuildOnVm,
    # A server build you downloaded yourself instead of using gh: the
    # legion-server-linux.zip artifact from a "Server build" run on GitHub,
    # or the folder it unzips to.
    [string]$Binary = "",
    # How much the server takes on: auto picks from the VM's memory, cores
    # and disk (small = 1-2 GB, brains up to 256; medium = 4-8 GB, 512;
    # large = 12 GB+ and 3+ cores, like Oracle's 4-core/24 GB Ampere, 1024).
    # keep leaves the sizing settings on the VM as they are. Values in
    # deploy.env (MAX_ONLINE_BRAIN, DB_POOL, KEEP_RAIDS_DAYS) always win.
    [ValidateSet("auto", "small", "medium", "large", "keep")]
    [string]$Size = "auto"
)

$ErrorActionPreference = "Stop"

# Defaults for each cloud. Anything passed on the command line wins.
if ($Cloud -eq "oracle") {
    if (-not $VmUser) { $VmUser = "ubuntu" }
    if (-not $VmHost) {
        Write-Host "Give the Oracle VM's public IP address: .\deploy.ps1 -Cloud oracle -VmHost <ip>" -ForegroundColor Red
        Write-Host "(Oracle console > Compute > Instances > your instance > Public IP address.)" -ForegroundColor Yellow
        exit 1
    }
    # Oracle hands out the private key as a download (ssh-key-<date>.key);
    # save it as ~\.ssh\oracle.key or pass -SshKey.
    if (-not $SshKey) { $SshKey = "$HOME\.ssh\oracle.key" }
} else {
    if (-not $VmUser) { $VmUser = "azureuser" }
    if (-not $VmHost) { $VmHost = "40.81.20.114" }
    if (-not $SshKey) { $SshKey = "$HOME\.ssh\id_ed25519" }
}
if (-not $VmPath) { $VmPath = "/home/$VmUser/legion-server" }
$Portal = if ($Cloud -eq "oracle") {
    "the Oracle console (Networking > Virtual cloud networks > your VCN > Security Lists > Default Security List > Add Ingress Rules, source 0.0.0.0/0)"
} else {
    "the VM's Azure network settings (Networking > Add inbound port rule)"
}

# Same flow as the portfolio's deploy.ps1: pack a tarball, upload it over scp
# with keepalives, then run the VM side as one base64 script over ssh. The
# server itself comes ready-made from GitHub Actions (.github/workflows/
# server.yml builds it for the VM's ARM processor), so the VM doesn't compile.
# If that build can't be had, the VM builds it from the uploaded source.
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
if ($VmUser -notmatch '^[a-z_][a-z0-9_-]*$') {
    Write-Host "VmUser must be a plain Linux user name like ubuntu or azureuser." -ForegroundColor Red
    exit 1
}
if ($VmHost -notmatch '^[A-Za-z0-9]([A-Za-z0-9.:-]*[A-Za-z0-9])?$') {
    Write-Host "VmHost must be a plain IP address or host name (no ssh://, no user@)." -ForegroundColor Red
    exit 1
}
if ($Port -lt 1024 -or $Port -gt 65535) {
    Write-Host "Port must be between 1024 and 65535." -ForegroundColor Red
    exit 1
}
# deploy.env holds the sign-in keys and ADMIN_KEY. Stop before doing
# anything if it's missing, instead of deploying a server nobody can sign in to.
$DeployEnv = Join-Path $Backend "deploy.env"
if (-not (Test-Path $DeployEnv) -and -not $NoSecrets) {
    Write-Host "deploy.env not found in $Backend" -ForegroundColor Red
    Write-Host "It holds the Google/Discord sign-in keys and ADMIN_KEY. Make it with:" -ForegroundColor Yellow
    Write-Host "    copy deploy.env.example deploy.env" -ForegroundColor Yellow
    Write-Host "    notepad deploy.env" -ForegroundColor Yellow
    Write-Host "then run .\deploy.ps1 again. To deploy without it anyway: .\deploy.ps1 -NoSecrets" -ForegroundColor Yellow
    exit 1
}
if (-not (Test-Path $SshKey)) {
    Write-Host "SSH key not found: $SshKey" -ForegroundColor Red
    if ($Cloud -eq "oracle") {
        Write-Host "Use the private key you downloaded when creating the instance, for example:" -ForegroundColor Yellow
        Write-Host "    .\deploy.ps1 -Cloud oracle -VmHost <ip> -SshKey $HOME\Downloads\ssh-key-2026-12-01.key" -ForegroundColor Yellow
    } else {
        Write-Host "Use the same key as the portfolio deploy, for example: .\deploy.ps1 -SshKey C:\path\to\key.pem" -ForegroundColor Yellow
    }
    exit 1
}
# Windows' ssh refuses a key file other accounts can read ("UNPROTECTED
# PRIVATE KEY FILE" / "bad permissions"), which a downloaded key usually is.
# Limit the downloaded Oracle key to this account (the Azure key already works).
if ($Cloud -eq "oracle") {
    try {
        icacls $SshKey /inheritance:r /grant:r "$($env:USERNAME):(R)" *> $null
    } catch { }
}

Write-Host ""
Write-Host "=====================================" -ForegroundColor Cyan
Write-Host "  Legion of Minds server deploy" -ForegroundColor Cyan
Write-Host "  to $VmUser@$VmHost ($Cloud)" -ForegroundColor Cyan
Write-Host "=====================================" -ForegroundColor Cyan
Write-Host ""

# ─── Step 1: Get the server GitHub built ─────────────────────────
# The ready-made servers go in backend/deploy-bin (gitignored), which is
# packed with the source; the VM uses the one for its processor.
$BinDir = Join-Path $Backend "deploy-bin"
if (Test-Path $BinDir) { Remove-Item -Recurse -Force $BinDir }
# Everything the server is built from. A GitHub build of a commit with the
# same files here is the same server.
$ServerPaths = @("backend", "game/crates/godcore", "game/Cargo.toml", "game/Cargo.lock")

function Clear-DeployFiles {
    if (Test-Path $Archive) { Remove-Item -Force $Archive }
    if (Test-Path $BinDir) { Remove-Item -Recurse -Force $BinDir }
}

# Checks a downloaded build against its SHA256SUMS. Returns what's wrong, or
# nothing when it's complete.
function Test-ServerBuild([string]$Dir) {
    $sums = Join-Path $Dir "SHA256SUMS"
    if (-not (Test-Path $sums)) { return "SHA256SUMS is missing" }
    foreach ($line in Get-Content $sums) {
        if ($line -match '^([0-9a-fA-F]{64})\s+\*?(\S+)$') {
            $want = $Matches[1].ToLower(); $name = $Matches[2]
            $f = Join-Path $Dir $name
            if (-not (Test-Path $f)) { return "$name is missing" }
            if ((Get-FileHash -Algorithm SHA256 $f).Hash.ToLower() -ne $want) { return "$name is damaged (checksum differs)" }
        }
    }
    if (-not (Test-Path (Join-Path $Dir "bag-server-aarch64"))) { return "bag-server-aarch64 is missing" }
    return $null
}

# Downloads the "Server build" artifact made from the same server source as
# this checkout. Returns $true when $BinDir holds a checked build.
function Get-GitHubBuild {
    # git and gh report "not found" on stderr; that's an answer here, not an
    # error that should stop the deploy (Windows PowerShell 5.1 would stop).
    $ErrorActionPreference = "Continue"
    if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
        Write-Host "  The GitHub CLI (gh) isn't installed, so the server can't be downloaded." -ForegroundColor DarkYellow
        Write-Host "  Install it once with:  winget install --id GitHub.cli   then:  gh auth login" -ForegroundColor DarkYellow
        return $false
    }
    gh auth status *> $null
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  gh isn't signed in to GitHub. Run once:  gh auth login" -ForegroundColor DarkYellow
        return $false
    }
    $head = (git rev-parse HEAD 2>$null)
    if ($LASTEXITCODE -ne 0 -or -not $head) {
        Write-Host "  This folder isn't a git checkout, so the matching GitHub build can't be found." -ForegroundColor DarkYellow
        return $false
    }
    $head = $head.Trim()
    $changed = git status --porcelain -- @ServerPaths
    if ($changed) {
        Write-Host "  You have uncommitted changes to the server code, which GitHub's build doesn't have:" -ForegroundColor DarkYellow
        $changed | Select-Object -First 5 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkYellow }
        Write-Host "  Commit and push them (GitHub then builds them), or deploy with -BuildOnVm." -ForegroundColor DarkYellow
        return $false
    }

    # Newest first. Pull request runs are skipped: they build the PR merged
    # into its target branch, which isn't exactly any commit here.
    $runs = gh api "repos/{owner}/{repo}/actions/workflows/server.yml/runs?per_page=50" `
        --jq '.workflow_runs[] | [.id, .head_sha, .status, .conclusion, .event] | @tsv' 2>$null
    if ($LASTEXITCODE -ne 0) { $runs = @() }
    foreach ($line in @($runs)) {
        if (-not $line) { continue }
        $id, $sha, $status, $conclusion, $event = "$line" -split "`t"
        if ($event -eq "pull_request") { continue }
        # A run of a commit this checkout doesn't have yet can't be compared.
        git cat-file -e "$sha^{commit}" 2>$null
        if ($LASTEXITCODE -ne 0) { continue }
        git diff --quiet $sha $head -- @ServerPaths
        if ($LASTEXITCODE -ne 0) { continue }
        $short = $sha.Substring(0, 7)
        if ($status -ne "completed") {
            Write-Host "  GitHub is still building the server for commit $short. Waiting for it (a few minutes)..." -ForegroundColor Yellow
            gh run watch $id --exit-status --interval 15 | Out-Host
            if ($LASTEXITCODE -ne 0) { Write-Host "  That build failed on GitHub." -ForegroundColor DarkYellow; continue }
        } elseif ($conclusion -ne "success") {
            continue
        }
        Write-Host "  Downloading the server GitHub built for commit $short..." -ForegroundColor Yellow
        if (Test-Path $BinDir) { Remove-Item -Recurse -Force $BinDir }
        gh run download $id -n legion-server-linux -D $BinDir | Out-Host
        if ($LASTEXITCODE -ne 0) {
            Write-Host "  Download failed (GitHub keeps these builds for 30 days)." -ForegroundColor DarkYellow
            continue
        }
        $bad = Test-ServerBuild $BinDir
        if ($bad) { Write-Host "  The download isn't usable: $bad" -ForegroundColor DarkYellow; continue }
        return $true
    }

    Write-Host "  GitHub has no server build for this code yet." -ForegroundColor DarkYellow
    $branch = (git rev-parse --abbrev-ref HEAD 2>$null)
    if ($branch) { $branch = $branch.Trim() }
    $pushed = $false
    if ($branch -and $branch -ne "HEAD") {
        git fetch -q origin $branch 2>$null
        git merge-base --is-ancestor $head "origin/$branch" 2>$null
        $pushed = ($LASTEXITCODE -eq 0)
    }
    if ($pushed) {
        Write-Host "  Asking GitHub to build $branch now..." -ForegroundColor Yellow
        gh workflow run server.yml --ref $branch 2>$null | Out-Host
        if ($LASTEXITCODE -eq 0) {
            for ($i = 0; $i -lt 12; $i++) {
                Start-Sleep -Seconds 5
                $id = $null
                $new = gh api "repos/{owner}/{repo}/actions/workflows/server.yml/runs?event=workflow_dispatch&per_page=5" `
                    --jq '.workflow_runs[] | [.id, .head_sha] | @tsv' 2>$null
                foreach ($r in @($new)) {
                    $rid, $rsha = "$r" -split "`t"
                    if ($rsha -eq $head) { $id = $rid; break }
                }
                if ($id) {
                    Write-Host "  Waiting for GitHub's build (a few minutes)..." -ForegroundColor Yellow
                    gh run watch $id --exit-status --interval 15 | Out-Host
                    if ($LASTEXITCODE -ne 0) { Write-Host "  That build failed on GitHub." -ForegroundColor DarkYellow; return $false }
                    gh run download $id -n legion-server-linux -D $BinDir | Out-Host
                    if ($LASTEXITCODE -ne 0) { return $false }
                    $bad = Test-ServerBuild $BinDir
                    if ($bad) { Write-Host "  The download isn't usable: $bad" -ForegroundColor DarkYellow; return $false }
                    return $true
                }
            }
        }
        Write-Host "  Couldn't start a build. On GitHub: Actions > Server build > Run workflow ($branch)." -ForegroundColor DarkYellow
    } else {
        Write-Host "  Push your commits so GitHub builds them, then run .\deploy.ps1 again." -ForegroundColor DarkYellow
    }
    return $false
}

Write-Host "[1/5] Getting the server GitHub built..." -ForegroundColor Yellow
$HaveBuild = $false
if ($BuildOnVm) {
    Write-Host "  -BuildOnVm: the VM builds the server from source." -ForegroundColor DarkYellow
} elseif ($Binary) {
    if (-not (Test-Path $Binary)) {
        Write-Host "Not found: $Binary" -ForegroundColor Red
        exit 1
    }
    New-Item -ItemType Directory -Force $BinDir | Out-Null
    if ((Get-Item $Binary).PSIsContainer) {
        Copy-Item -Force (Join-Path $Binary "*") $BinDir
    } else {
        Expand-Archive -Force $Binary $BinDir
    }
    $bad = Test-ServerBuild $BinDir
    if ($bad) {
        Write-Host "$Binary isn't a usable server build: $bad" -ForegroundColor Red
        Write-Host "Use the legion-server-linux artifact of a 'Server build' run on GitHub." -ForegroundColor Yellow
        Clear-DeployFiles
        exit 1
    }
    $HaveBuild = $true
} else {
    Push-Location $Root
    # Only the last value counts: anything else a function writes out would
    # also land in its result.
    try { $HaveBuild = ((Get-GitHubBuild) | Select-Object -Last 1) -eq $true } finally { Pop-Location }
}
if ($HaveBuild) {
    $Commit = (Get-Content (Join-Path $BinDir "COMMIT") -ErrorAction SilentlyContinue | Select-Object -First 1)
    $Short = if ($Commit) { $Commit.Substring(0, [Math]::Min(7, $Commit.Length)) } else { "unknown" }
    Write-Host "  Got the server built by GitHub (commit $Short)." -ForegroundColor Green
} elseif (-not $BuildOnVm) {
    if (Test-Path $BinDir) { Remove-Item -Recurse -Force $BinDir }
    Write-Host "  Falling back to building on the VM (slow on a small VM)." -ForegroundColor DarkYellow
    Write-Host "  Press Ctrl+C now to stop instead; continuing in 10 seconds..." -ForegroundColor DarkYellow
    Start-Sleep -Seconds 10
}

# ─── Step 2: Pack the source ─────────────────────────────────────
Write-Host "[2/5] Packing the server..." -ForegroundColor Yellow
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
    Clear-DeployFiles
    exit 1
}
$SizeMb = [math]::Round((Get-Item $Archive).Length / 1MB, 1)
Write-Host "  Archive created ($SizeMb MB)." -ForegroundColor Green

# ─── Step 3: Upload ──────────────────────────────────────────────
Write-Host "[3/5] Uploading to the VM..." -ForegroundColor Yellow
scp -i $SshKey @SshOpts $Archive "${VmUser}@${VmHost}:/tmp/legion-deploy.tar.gz"
if ($LASTEXITCODE -ne 0) {
    Write-Host "Upload failed (scp exit $LASTEXITCODE). Re-run .\deploy.ps1 to retry." -ForegroundColor Red
    Clear-DeployFiles
    exit 1
}
Write-Host "  Upload complete." -ForegroundColor Green

# Server secrets (OAuth keys, ADMIN_KEY), merged into the VM's env file:
# only the keys in deploy.env are replaced or added.
if (Test-Path $DeployEnv) {
    scp -i $SshKey @SshOpts $DeployEnv "${VmUser}@${VmHost}:/tmp/legion-deploy.env"
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Secrets upload failed (scp exit $LASTEXITCODE). Re-run .\deploy.ps1 to retry." -ForegroundColor Red
        Clear-DeployFiles
        exit 1
    }
    Write-Host "  Server secrets uploaded (deploy.env)." -ForegroundColor Green
} else {
    Write-Host "  -NoSecrets: deploying without deploy.env (no sign-in keys, no admin page)." -ForegroundColor DarkYellow
}

# ─── Step 4: Back up, install and restart on the VM ────────────────
$What = if ($HaveBuild) { "installing the new server" } else { "building the server (several minutes)" }
Write-Host "[4/5] Backing up the database, $What and restarting on the VM..." -ForegroundColor Yellow
# The VM side is deploy/server.sh with its arguments set at the top. Like the
# portfolio, it travels as one base64 token, so Windows can't mangle its
# quotes, and with LF line endings, so bash doesn't choke on `\r`.
$ServerScript = Get-Content -Raw -Encoding UTF8 (Join-Path $Backend "deploy\server.sh")
$RemoteScript = "set -- '$VmPath' '$Port' '$VmHost' '$Domain' '$Size'`n" + $ServerScript
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
    Clear-DeployFiles
    exit 1
}
Clear-DeployFiles

# ─── Step 5: Check from here ─────────────────────────────────────
Write-Host "[5/5] Checking the server from this computer..." -ForegroundColor Yellow
$Url = if ($Domain) { "https://$Domain" } else { "http://${VmHost}:$Port" }
try {
    Invoke-WebRequest -UseBasicParsing -TimeoutSec 15 "$Url/health" | Out-Null
    Write-Host "  $Url answers." -ForegroundColor Green
} catch {
    Write-Host "  The server runs on the VM, but $Url doesn't answer from here." -ForegroundColor DarkYellow
    if ($Domain) {
        Write-Host "  Check that $Domain points at $VmHost and that TCP ports 80 and 443 are open in" -ForegroundColor DarkYellow
        Write-Host "  $Portal." -ForegroundColor DarkYellow
    } else {
        Write-Host "  Open TCP port $Port (inbound) in" -ForegroundColor DarkYellow
        Write-Host "  $Portal." -ForegroundColor DarkYellow
    }
    Write-Host "  (The VM's own firewall was opened by the deploy; this one is in the cloud's settings.)" -ForegroundColor DarkYellow
}

Write-Host ""
Write-Host "=====================================" -ForegroundColor Green
Write-Host "  Deploy complete!" -ForegroundColor Green
Write-Host "  Admin page: $Url/admin" -ForegroundColor Green
Write-Host "  Set the GitHub repo variable BAG_SERVER to $Url" -ForegroundColor Green
Write-Host "  so new releases connect to this server." -ForegroundColor Green
Write-Host "=====================================" -ForegroundColor Green
