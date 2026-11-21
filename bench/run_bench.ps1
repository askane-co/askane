param(
    [string[]]$Binaries = @(),
    [int]$Runs = 3,
    [switch]$ShowRaw,
    [switch]$SkipLanguages
)

$ErrorActionPreference = "Stop"
$BenchDir = $PSScriptRoot
$RepoRoot = Split-Path -Parent $BenchDir
$Culture = [System.Globalization.CultureInfo]::InvariantCulture

# TargetMs calibrated to ~0.82 * Python 3.12 median so Python scores ~82/100 and fast JITs approach 100/100.
$Tests = @(
    @{ Id = "01_game";        Label = "Game";        Weight = 15; TargetMs = 95.0;  Checksum = "236268343" },
    @{ Id = "02_infographic"; Label = "Infographic"; Weight = 15; TargetMs = 132.0; Checksum = "234330233" },
    @{ Id = "03_math";        Label = "Math";        Weight = 20; TargetMs = 83.0;  Checksum = "150000" },
    @{ Id = "04_visual";      Label = "Visual";      Weight = 15; TargetMs = 100.0; Checksum = "516587666" },
    @{ Id = "05_scripting";   Label = "Scripting";   Weight = 35; TargetMs = 138.0; Checksum = "389426226" }
)

function Get-VersionSortKey {
    param([string]$VerText, [string]$FallbackName)
    if ($VerText -match '(\d+)\.(\d+)\.(\d+)(?:-(.+))?') {
        $maj = [int]$matches[1]
        $min = [int]$matches[2]
        $pat = [int]$matches[3]
        $pre = if ($matches[4]) { $matches[4] } else { "" }
        # Release without suffix sorts after pre/fix tags only within patch if needed, keep lexical stability
        return ("{0:D4}.{1:D4}.{2:D4}.{3}.{4}" -f $maj, $min, $pat, $pre, $FallbackName)
    }
    return ("9999.9999.9999." + $FallbackName)
}

function Get-RelativeLabel {
    param([string]$FullPath)
    if ($FullPath.StartsWith($RepoRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        return $FullPath.Substring($RepoRoot.Length).TrimStart('\', '/')
    }
    return [System.IO.Path]::GetFileName($FullPath)
}

function Resolve-AskaneTargets {
    param([string[]]$InputBins)
    $seen = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    $paths = New-Object System.Collections.Generic.List[string]

    $addPath = {
        param([string]$PathToAdd)
        if (Test-Path $PathToAdd) {
            $resolved = (Resolve-Path $PathToAdd).Path
            if ($seen.Add($resolved)) {
                $paths.Add($resolved)
            }
        }
    }

    if ($InputBins.Count -gt 0) {
        foreach ($b in $InputBins) {
            if (Test-Path $b) {
                & $addPath $b
            } else {
                Write-Warning "Binary not found: $b"
            }
        }
    } else {
        Get-ChildItem $BenchDir -Filter "*.exe" -File | ForEach-Object { & $addPath $_.FullName }
        $versionsDir = Join-Path $BenchDir "versions"
        if (Test-Path $versionsDir) {
            Get-ChildItem $versionsDir -Filter "*.exe" -File | ForEach-Object { & $addPath $_.FullName }
        }
        $rootExe = Join-Path $RepoRoot "askane.exe"
        $releaseExe = Join-Path $RepoRoot "target\release\askane.exe"
        if (Test-Path $rootExe) {
            & $addPath $rootExe
        } elseif (Test-Path $releaseExe) {
            & $addPath $releaseExe
        }
    }

    $items = New-Object System.Collections.Generic.List[hashtable]
    foreach ($bin in $paths) {
        $verRaw = (& $bin -v 2>$null | Select-Object -First 1)
        $rel = Get-RelativeLabel -FullPath $bin
        $label = if ([string]::IsNullOrWhiteSpace($verRaw)) { $rel } else { "$verRaw ($rel)" }
        $items.Add(@{
            Kind      = "Askane"
            Label     = $label
            Exe       = $bin
            PreArgs   = @()
            ScriptDir = $BenchDir
            Ext       = ".ask"
            SortKey   = Get-VersionSortKey -VerText $verRaw -FallbackName $rel
        })
    }

    return @($items | Sort-Object { $_.SortKey })
}

function Resolve-LanguageTargets {
    $items = New-Object System.Collections.Generic.List[hashtable]

    $pyDir = Join-Path $BenchDir "langs\py"
    $pyCmd = Get-Command python -ErrorAction SilentlyContinue
    if ($null -ne $pyCmd -and (Test-Path $pyDir)) {
        $pyVer = (& $pyCmd.Source --version 2>&1 | Select-Object -First 1).ToString().Trim()
        if ([string]::IsNullOrWhiteSpace($pyVer)) { $pyVer = "Python" }
        $items.Add(@{
            Kind      = "Lang"
            Label     = $pyVer
            Exe       = $pyCmd.Source
            PreArgs   = @()
            ScriptDir = $pyDir
            Ext       = ".py"
        })
    }

    $jsDir = Join-Path $BenchDir "langs\js"
    if (Test-Path $jsDir) {
        $nodeCmd = Get-Command node -ErrorAction SilentlyContinue
        if ($null -ne $nodeCmd) {
            $nodeVer = (& $nodeCmd.Source -v 2>&1 | Select-Object -First 1).ToString().Trim()
            $items.Add(@{
                Kind      = "Lang"
                Label     = "Node.js $nodeVer"
                Exe       = $nodeCmd.Source
                PreArgs   = @()
                ScriptDir = $jsDir
                Ext       = ".js"
            })
        }
        $bunCmd = Get-Command bun -ErrorAction SilentlyContinue
        if ($null -ne $bunCmd) {
            $bunVer = (& $bunCmd.Source -v 2>&1 | Select-Object -First 1).ToString().Trim()
            $items.Add(@{
                Kind      = "Lang"
                Label     = "Bun $bunVer"
                Exe       = $bunCmd.Source
                PreArgs   = @()
                ScriptDir = $jsDir
                Ext       = ".js"
            })
        }
        $denoCmd = Get-Command deno -ErrorAction SilentlyContinue
        if ($null -ne $denoCmd) {
            $denoVer = ((& $denoCmd.Source --version 2>&1 | Select-Object -First 1).ToString().Trim())
            $items.Add(@{
                Kind      = "Lang"
                Label     = $denoVer
                Exe       = $denoCmd.Source
                PreArgs   = @("run")
                ScriptDir = $jsDir
                Ext       = ".js"
            })
        }
    }

    return $items.ToArray()
}

function Invoke-BenchRun {
    param(
        [hashtable]$Target,
        [string]$ScriptPath
    )
    if ($Target.Kind -eq "Askane") {
        $out = & $Target.Exe --unsafe $ScriptPath 2>&1
        if ($LASTEXITCODE -ne 0) {
            $out = & $Target.Exe $ScriptPath 2>&1
            if ($LASTEXITCODE -ne 0) {
                return $null
            }
        }
    } else {
        $pre = $Target.PreArgs
        $out = & $Target.Exe @pre $ScriptPath 2>&1
        if ($LASTEXITCODE -ne 0) {
            return $null
        }
    }

    foreach ($line in ($out -split "`r?`n")) {
        if ($line -match '^BENCH_RESULT\s+(\S+)\s+([0-9.]+)\s+(\S+)$') {
            return @{
                Id       = $matches[1]
                Ms       = [double]::Parse($matches[2], $Culture)
                Checksum = $matches[3]
            }
        }
    }
    return $null
}

function Get-Median {
    param([double[]]$Values)
    $sorted = $Values | Sort-Object
    $n = $sorted.Count
    if ($n -eq 0) { return 0.0 }
    if ($n % 2 -eq 1) {
        return $sorted[[int][math]::Floor($n / 2)]
    }
    return ($sorted[$n / 2 - 1] + $sorted[$n / 2]) / 2.0
}

$Targets = New-Object System.Collections.Generic.List[hashtable]
foreach ($t in (Resolve-AskaneTargets -InputBins $Binaries)) { $Targets.Add($t) }
if (-not $SkipLanguages) {
    foreach ($t in (Resolve-LanguageTargets)) { $Targets.Add($t) }
}

if ($Targets.Count -eq 0) {
    Write-Error "No askane binaries or language runtimes found."
    exit 1
}

Write-Host "Askane Benchmark Suite (0-100 strict scale, ~0.82 * Python 3.12 baseline)" -ForegroundColor Cyan
Write-Host "Weights: Scripting 35%, Math 20%, Game 15%, Infographic 15%, Visual 15%"
Write-Host "Runs per test: $Runs (after 1 warmup run; using median)"
Write-Host ""

$Summary = @()

foreach ($target in $Targets) {
    Write-Host "=== $($target.Label) ===" -ForegroundColor Yellow
    $weightedTotal = 0.0
    $rowScores = @{}

    foreach ($t in $Tests) {
        $scriptPath = Join-Path $target.ScriptDir ($t.Id + $target.Ext)
        if (-not (Test-Path $scriptPath)) {
            Write-Host ("  {0,-12} ({1,2}%) : SKIP (missing {2})" -f $t.Label, $t.Weight, $scriptPath) -ForegroundColor DarkGray
            $rowScores[$t.Id] = "SKIP"
            continue
        }

        # Warmup + checksum check
        $warm = Invoke-BenchRun -Target $target -ScriptPath $scriptPath
        if ($null -eq $warm -or $warm.Checksum -ne $t.Checksum) {
            $gotSum = if ($null -eq $warm) { "ERROR" } else { $warm.Checksum }
            Write-Host ("  {0,-12} ({1,2}%) : FAIL (checksum expected {2}, got {3})" -f $t.Label, $t.Weight, $t.Checksum, $gotSum) -ForegroundColor Red
            $rowScores[$t.Id] = "FAIL"
            continue
        }

        $samples = New-Object System.Collections.Generic.List[double]
        $failed = $false
        for ($r = 0; $r -lt $Runs; $r++) {
            $res = Invoke-BenchRun -Target $target -ScriptPath $scriptPath
            if ($null -eq $res -or $res.Checksum -ne $t.Checksum) {
                $failed = $true
                break
            }
            $samples.Add($res.Ms)
        }

        if ($failed -or $samples.Count -eq 0) {
            Write-Host ("  {0,-12} ({1,2}%) : FAIL" -f $t.Label, $t.Weight) -ForegroundColor Red
            $rowScores[$t.Id] = "FAIL"
            continue
        }

        $medianMs = Get-Median -Values $samples.ToArray()
        $score = [math]::Min(100.0, 100.0 * ($t.TargetMs / [math]::Max(0.01, $medianMs)))
        $weightedTotal += $score * ($t.Weight / 100.0)
        $rowScores[$t.Id] = ("{0,5:N1}" -f $score)

        $rawInfo = if ($ShowRaw) { " [" + (($samples | ForEach-Object { "{0:N1}" -f $_ }) -join ", ") + "]" } else { "" }
        Write-Host ("  {0,-12} ({1,2}%) : {2,7:N1} ms | Score: {3,5:N1} / 100{4}" -f $t.Label, $t.Weight, $medianMs, $score, $rawInfo)
    }

    Write-Host ("  TOTAL SCORE        : {0,5:N2} / 100" -f $weightedTotal) -ForegroundColor Green
    Write-Host ""

    $Summary += [PSCustomObject]@{
        Runtime     = $target.Label
        Game        = $rowScores["01_game"]
        Infographic = $rowScores["02_infographic"]
        Math        = $rowScores["03_math"]
        Visual      = $rowScores["04_visual"]
        Scripting   = $rowScores["05_scripting"]
        TotalScore  = [math]::Round($weightedTotal, 2)
    }
}

Write-Host "=== Comparison Summary ===" -ForegroundColor Cyan
$Summary | Format-Table -AutoSize
