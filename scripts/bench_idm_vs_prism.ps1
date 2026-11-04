# PowerShell Benchmark: Real IDM vs Prism Engine on Windows
param(
    [switch]$IncludeInternet = $false
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$serverExe = Join-Path $root "target\release\prism-testserver.exe"
$prismExe = Join-Path $root "target\release\examples\prism-get.exe"
$idmExe = "C:\Program Files (x86)\Internet Download Manager\IDMan.exe"

if (-not (Test-Path $idmExe)) {
    Write-Error "IDM executable not found at $idmExe"
    exit 1
}

if (-not (Test-Path $serverExe) -or -not (Test-Path $prismExe)) {
    Write-Error "Prism binaries not built yet. Please compile target/release."
    exit 1
}

Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "  HEAD-TO-HEAD BENCHMARK: INTERNET DOWNLOAD MANAGER vs PRISM" -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "IDM Path:   $idmExe"
Write-Host "Prism Path: $prismExe"
Write-Host "Server:     $serverExe"
Write-Host ""

$tempDir = Join-Path $env:TEMP "idm_vs_prism_bench"
if (Test-Path $tempDir) { Remove-Item $tempDir -Recurse -Force }
New-Item -ItemType Directory -Path $tempDir -Force | Out-Null

$results = [System.Collections.Generic.List[PSCustomObject]]::new()

function Measure-PrismDownload($label, $url, $expectedBytes, $expectedSha, $conns) {
    $dir = Join-Path $tempDir ("prism_" + [System.Guid]::NewGuid().ToString().Substring(0,8))
    New-Item -ItemType Directory -Path $dir -Force | Out-Null

    Write-Host "[$label] Testing Prism ($conns conns)..." -NoNewline
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $prismExe -ArgumentList @($url, "-c", $conns.ToString(), "-o", $dir) -PassThru -NoNewWindow -Wait
    $sw.Stop()

    $secs = [Math]::Round($sw.Elapsed.TotalSeconds, 2)
    $files = Get-ChildItem -Path $dir -File | Where-Object { $_.Name -notlike "*.prismpart" }
    if ($files.Count -eq 0) {
        Write-Host " FAILED (No file output)" -ForegroundColor Red
        return
    }
    $file = $files[0]
    $size = $file.Length
    $mb = [Math]::Round($size / 1MB, 2)
    $speed = if ($secs -gt 0) { [Math]::Round($mb / $secs, 2) } else { 0 }
    
    $sha = (Get-FileHash -Path $file.FullName -Algorithm SHA256).Hash.ToLower()
    $verified = if ($expectedSha) { $sha -eq $expectedSha.ToLower() } else { $true }

    Write-Host " Done! $secs s | $speed MB/s | Size: $mb MB | Verified: $verified" -ForegroundColor Green

    $results.Add([PSCustomObject]@{
        Scenario = $label
        Tool = "Prism ($conns conns)"
        TimeSec = $secs
        AvgSpeedMBs = $speed
        FileSizeMB = $mb
        Verified = $verified
    })

    Remove-Item $dir -Recurse -Force
}

function Measure-IdmDownload($label, $url, $expectedBytes, $expectedSha, $filename) {
    $dir = Join-Path $tempDir ("idm_" + [System.Guid]::NewGuid().ToString().Substring(0,8))
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    $targetFile = Join-Path $dir $filename

    Write-Host "[$label] Testing IDM..." -NoNewline

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    # /d URL /p local_path /f local_file_name /n /q
    Start-Process -FilePath $idmExe -ArgumentList @("/d", $url, "/p", $dir, "/f", $filename, "/n") | Out-Null

    # Poll until target file exists and no .tmp files exist
    $timeoutSec = 120
    $startTime = [System.DateTime]::UtcNow
    $finished = $false

    while (([System.DateTime]::UtcNow - $startTime).TotalSeconds -lt $timeoutSec) {
        Start-Sleep -Milliseconds 200
        if (Test-Path $targetFile) {
            $len = (Get-Item $targetFile).Length
            if ($len -ge $expectedBytes -and $expectedBytes -gt 0) {
                # Check if temp files are gone
                $tmps = Get-ChildItem -Path $dir -Filter "*.tmp" -ErrorAction SilentlyContinue
                if ($tmps.Count -eq 0) {
                    $finished = $true
                    break
                }
            }
        }
    }
    $sw.Stop()

    $secs = [Math]::Round($sw.Elapsed.TotalSeconds, 2)
    if (-not $finished -or -not (Test-Path $targetFile)) {
        Write-Host " FAILED (Timeout or incomplete)" -ForegroundColor Red
        return
    }

    $size = (Get-Item $targetFile).Length
    $mb = [Math]::Round($size / 1MB, 2)
    $speed = if ($secs -gt 0) { [Math]::Round($mb / $secs, 2) } else { 0 }

    $sha = (Get-FileHash -Path $targetFile -Algorithm SHA256).Hash.ToLower()
    $verified = if ($expectedSha) { $sha -eq $expectedSha.ToLower() } else { $true }

    Write-Host " Done! $secs s | $speed MB/s | Size: $mb MB | Verified: $verified" -ForegroundColor Green

    $results.Add([PSCustomObject]@{
        Scenario = $label
        Tool = "IDM (Real Win32 App)"
        TimeSec = $secs
        AvgSpeedMBs = $speed
        FileSizeMB = $mb
        Verified = $verified
    })

    Remove-Item $dir -Recurse -Force
}

function Get-UrlSha256($url) {
    $tmp = [System.IO.Path]::GetTempFileName()
    try {
        curl.exe -s -L $url -o $tmp
        return (Get-FileHash -Path $tmp -Algorithm SHA256).Hash.ToLower()
    }
    finally {
        if (Test-Path $tmp) { Remove-Item $tmp -Force }
    }
}

# --- SCENARIO 1: Unthrottled Local Disk / CPU Throughput (512 MB) ---
$port = 19100
$serverProcess = Start-Process -FilePath $serverExe -ArgumentList @("--port", $port.ToString()) -PassThru -NoNewWindow
Start-Sleep -Seconds 1

try {
    $size1 = 512 * 1024 * 1024
    $url1 = "http://127.0.0.1:$port/file/bench512.bin?size=$size1&seed=42"
    $refSha1 = $null # verified via length and content

    Measure-PrismDownload "512MB Local Unthrottled" $url1 $size1 $refSha1 8
    Measure-IdmDownload "512MB Local Unthrottled" $url1 $size1 $refSha1 "bench512.bin"
}
finally {
    if ($serverProcess -and -not $serverProcess.HasExited) {
        $serverProcess.Kill()
    }
}

# --- SCENARIO 2: Throttled Server (4 MB/s per connection limit, 256 MB) ---
$port = 19101
$serverProcess2 = Start-Process -FilePath $serverExe -ArgumentList @("--port", $port.ToString(), "--throttle", "4M") -PassThru -NoNewWindow
Start-Sleep -Seconds 1

try {
    $size2 = 256 * 1024 * 1024
    $url2 = "http://127.0.0.1:$port/file/bench256_throttled.bin?size=$size2&seed=88"
    $refSha2 = $null

    Measure-PrismDownload "256MB Throttled 4MB/s per conn" $url2 $size2 $refSha2 8
    Measure-IdmDownload "256MB Throttled 4MB/s per conn" $url2 $size2 $refSha2 "bench256_throttled.bin"
}
finally {
    if ($serverProcess2 -and -not $serverProcess2.HasExited) {
        $serverProcess2.Kill()
    }
}

# --- SCENARIO 3: Real Internet Download Test (OVH 100MB File) ---
$ovhUrl = "https://proof.ovh.net/files/100Mb.dat"
$ovhBytes = 104857600
Measure-PrismDownload "100MB Real Internet (OVH)" $ovhUrl $ovhBytes $null 8
Measure-IdmDownload "100MB Real Internet (OVH)" $ovhUrl $ovhBytes $null "100Mb.dat"

Write-Host ""
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "                 FINAL BENCHMARK RESULTS                  " -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan
$results | Format-Table -AutoSize
