#Requires -Version 7.0
# Milestone 1 acceptance with external oracles (docs/04-roadmap.md, Hito 1,
# criteria 2 and 8): the PowerShell twin of scripts/acceptance.sh. An AI agent
# builds "Forma Demo" through `forma mcp` (the stdio scenario test), then
# independent tools check the font it exported: OTS, HarfBuzz, fontTools,
# fontmake parity and, if it is installed, fontspector.
#
# Usage (from any folder):  pwsh scripts/acceptance.ps1
#
# Python: $env:FORMA_ORACLE_PYTHON if set, else tools/oracle/.venv, else
# python (or python3) on the PATH. It needs tools/oracle/requirements.txt:
#   python -m venv tools/oracle/.venv
#   tools/oracle/.venv/Scripts/pip install -r tools/oracle/requirements.txt
# fontspector is optional: cargo install fontspector --locked
#
# Artefacts go to out/acceptance/, which is recreated on every run.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# Native exit codes are checked explicitly after every call.
$PSNativeCommandUseErrorActionPreference = $false
trap {
    [Console]::Error.WriteLine("ACCEPTANCE FAILED: $_")
    exit 1
}

$Repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Core = Join-Path $Repo 'core'
$Out = Join-Path $Repo 'out' 'acceptance'

function Write-Step([string]$Text) {
    Write-Host ''
    Write-Host "==> $Text"
}

function Stop-Acceptance([string]$Text) {
    [Console]::Error.WriteLine("ACCEPTANCE FAILED: $Text")
    exit 1
}

function Assert-ExitCode([string]$What) {
    if ($LASTEXITCODE -ne 0) {
        Stop-Acceptance "$What (exit code $LASTEXITCODE)"
    }
}

function Find-Python {
    $fromEnv = [Environment]::GetEnvironmentVariable('FORMA_ORACLE_PYTHON')
    if ($fromEnv) {
        return $fromEnv
    }
    $venv = Join-Path $Repo 'tools' 'oracle' '.venv'
    foreach ($candidate in @((Join-Path $venv 'Scripts' 'python.exe'), (Join-Path $venv 'bin' 'python'))) {
        if (Test-Path $candidate -PathType Leaf) {
            return $candidate
        }
    }
    foreach ($name in @('python', 'python3')) {
        $command = Get-Command $name -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($command) {
            return $command.Source
        }
    }
    Stop-Acceptance 'no Python found: set FORMA_ORACLE_PYTHON or create tools/oracle/.venv'
}

$Python = Find-Python

# The same head timestamps from both compilers (fontc and fontmake honour it).
$env:SOURCE_DATE_EPOCH = '0'

Write-Step '1/5 Build the release forma binary'
Push-Location $Core
try {
    cargo build --release -p forma-cli --locked
    Assert-ExitCode 'cargo build failed'
    $metadata = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
    Assert-ExitCode 'cargo metadata failed'
}
finally {
    Pop-Location
}
$Forma = Join-Path $metadata.target_directory 'release' ($IsWindows ? 'forma.exe' : 'forma')
if (-not (Test-Path $Forma -PathType Leaf)) {
    Stop-Acceptance "the release binary is not at $Forma"
}

Write-Step "2/5 Agent scenario over MCP stdio (artefacts in $Out)"
if (Test-Path $Out) {
    Remove-Item -Recurse -Force $Out
}
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$env:FORMA_ACCEPTANCE_OUT = $Out
Push-Location $Core
try {
    cargo test -p forma-cli --locked --test mcp_scenario
    Assert-ExitCode 'the agent scenario failed'
}
finally {
    Pop-Location
    Remove-Item Env:FORMA_ACCEPTANCE_OUT -ErrorAction SilentlyContinue
}
foreach ($file in @('forma-demo.ttf', 'forma-demo.woff2', 'forma-demo.forma', 'expected.json')) {
    $path = Join-Path $Out $file
    if (-not (Test-Path $path -PathType Leaf) -or (Get-Item $path).Length -eq 0) {
        Stop-Acceptance "the scenario did not write $path"
    }
}

Write-Step '3/5 Export the scenario document as a UFO'
& $Forma export (Join-Path $Out 'forma-demo.forma') --format ufo -o (Join-Path $Out 'forma-demo.ufo')
Assert-ExitCode 'forma export --format ufo failed'

Write-Step '4/5 External oracles: OTS, HarfBuzz, fontTools, fontmake parity'
# Run from the artefact folder, never from inside the Python environment.
Push-Location $Out
try {
    & $Python -I (Join-Path $Repo 'tools' 'oracle' 'verify_font.py') `
        --ttf (Join-Path $Out 'forma-demo.ttf') `
        --expected (Join-Path $Out 'expected.json') `
        --woff2 (Join-Path $Out 'forma-demo.woff2') `
        --ufo (Join-Path $Out 'forma-demo.ufo') --fontmake-parity `
        --work-dir (Join-Path $Out 'fontmake')
    Assert-ExitCode 'the external oracles rejected the font (see above)'
}
finally {
    Pop-Location
}

Write-Step '5/5 fontspector (opentype profile)'
$Fontspector = Get-Command fontspector -CommandType Application -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($Fontspector) {
    Write-Host @(& $Fontspector.Source --version)[0]
    $report = Join-Path $Out 'fontspector.json'
    Push-Location $Out
    try {
        # Exit code 1 if any check ends in FAIL or worse (--error-code-on fail).
        & $Fontspector.Source --profile opentype --error-code-on fail --json $report (Join-Path $Out 'forma-demo.ttf')
        Assert-ExitCode "fontspector reported a FAIL (report: $report)"
    }
    finally {
        Pop-Location
    }
}
else {
    Write-Host 'fontspector: SKIPPED (not on the PATH; install it with: cargo install fontspector --locked)'
}

Write-Host ''
Write-Host 'ACCEPTANCE OK'
