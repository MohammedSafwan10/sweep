param(
    [Parameter(Mandatory)][string]$Baseline,
    [Parameter(Mandatory)][string]$Candidate,
    [Parameter(Mandatory)][string[]]$Paths,
    [Parameter(Mandatory)][string]$Output,
    [ValidateRange(1,10)][int]$Rounds = 3
)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $Output) { throw 'Output exists; choose a fresh results file.' }
$results = @()
foreach ($round in 1..$Rounds) {
    # Reverse the order each round to reduce systematic warm-cache advantage.
    $versions = if ($round % 2) { @('baseline','candidate') } else { @('candidate','baseline') }
    foreach ($version in $versions) {
        $binary = if ($version -eq 'baseline') { $Baseline } else { $Candidate }
        foreach ($path in $Paths) {
            $lines = @(& $binary $path)
            if ($LASTEXITCODE -ne 0) { throw "Profiler failed: $version $path" }
            foreach ($line in $lines) {
                $row = $line | ConvertFrom-Json
                $results += [pscustomobject]@{
                    round = $round; version = $version; path = $row.path
                    seconds = $row.seconds; bytes = $row.snapshot.bytes
                    entries = $row.snapshot.entries
                }
            }
        }
    }
}
$results | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $Output -Encoding utf8
$results | Format-Table -AutoSize
