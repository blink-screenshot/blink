# Makes makensis available to the next workflow steps (GitHub Actions, Windows runner).
# Uses an installed NSIS, else Chocolatey, else the official zip from SourceForge
# (the Chocolatey feed is not always reachable).

$version = "3.10"
$dirs = @("C:\Program Files (x86)\NSIS", "C:\Program Files\NSIS")

function Find-Nsis {
    foreach ($d in $dirs) {
        if (Test-Path "$d\makensis.exe") { return $d }
    }
    $cmd = Get-Command makensis -ErrorAction SilentlyContinue
    if ($cmd) { return Split-Path $cmd.Source }
    return $null
}

$found = Find-Nsis
if (-not $found) {
    choco install nsis -y --no-progress
    if ($LASTEXITCODE -ne 0) { Write-Warning "Chocolatey could not install NSIS (exit $LASTEXITCODE)" }
    $found = Find-Nsis
}
if (-not $found) {
    $zip = Join-Path $env:RUNNER_TEMP "nsis.zip"
    $url = "https://downloads.sourceforge.net/project/nsis/NSIS%203/$version/nsis-$version.zip"
    curl.exe -fsSL --retry 4 --retry-delay 5 -o $zip $url
    if ($LASTEXITCODE -ne 0) { throw "Could not download NSIS from $url" }
    Expand-Archive $zip -DestinationPath (Join-Path $env:RUNNER_TEMP "nsis") -Force
    $found = Join-Path $env:RUNNER_TEMP "nsis\nsis-$version"
}
if (-not (Test-Path "$found\makensis.exe")) { throw "makensis.exe not found in $found" }
Add-Content -Path $env:GITHUB_PATH -Value $found
& "$found\makensis.exe" /VERSION
